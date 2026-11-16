use crate::CliError;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, DeviceSummary, Frame, HelloStatus, CAP_USBIP_V1,
    PROTOCOL_VERSION_V0,
};
use portico_proto::identity::normalize_fingerprint;
use portico_proto::relay_proto::{parse_relay_uri, RelayFrame};
use portico_proto::tls::{build_client_tls_config, is_lan_or_loopback_ip, CertCheckPolicy};
use portico_proto::tofu::{KnownHosts, TofuDecision};
use portico_proto::DEFAULT_PORT;
use rand::RngCore;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetEndpoint {
    Direct(String),
    Relay {
        relay_addr: String,
        node_id: [u8; 16],
        pin_key: String,
    },
}

impl TargetEndpoint {
    pub fn parse(target: &str) -> Result<Self, CliError> {
        let trimmed = target.trim();
        if trimmed.starts_with("prly://") {
            let (relay_addr, node_id) = parse_relay_uri(trimmed)?;
            let pin_key = format!("prly://{}", hex::encode(node_id));
            return Ok(Self::Relay {
                relay_addr,
                node_id,
                pin_key,
            });
        }
        if trimmed.is_empty() {
            return Err(CliError::Remote("empty target".into()));
        }
        let with_port = if let Ok(ip) = trimmed.parse::<IpAddr>() {
            match ip {
                IpAddr::V4(_) => format!("{ip}:{DEFAULT_PORT}"),
                IpAddr::V6(_) => format!("[{ip}]:{DEFAULT_PORT}"),
            }
        } else if trimmed.starts_with('[') {
            if trimmed.contains("]:") {
                trimmed.to_string()
            } else {
                format!("{trimmed}:{DEFAULT_PORT}")
            }
        } else if trimmed.contains(':') {
            trimmed.to_string()
        } else {
            format!("{trimmed}:{DEFAULT_PORT}")
        };
        Ok(Self::Direct(with_port))
    }

    pub fn pin_key(&self) -> &str {
        match self {
            Self::Direct(s) => s.as_str(),
            Self::Relay { pin_key, .. } => pin_key.as_str(),
        }
    }
}

pub trait AsyncRw: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> AsyncRw for T {}

pub type AuthenticatedStream = Box<dyn AsyncRw>;

pub struct ConnectOptions<'a> {
    pub config_dir: &'a Path,
    pub target: &'a str,
    pub token: &'a str,
    pub trust_fp: Option<&'a str>,
    pub insecure_lan_plaintext: bool,
    pub client_name: &'a str,
}

fn known_hosts_path(config_dir: &Path) -> PathBuf {
    config_dir.join("known_hosts.json")
}

fn extract_host_for_sni(addr: &str) -> String {
    let raw = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    raw.trim_start_matches('[').trim_end_matches(']').to_string()
}

async fn resolve_lan_checked(addr: &str) -> Result<SocketAddr, CliError> {
    let host = extract_host_for_sni(addr);
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_lan_or_loopback_ip(ip) {
            return Err(CliError::PlaintextNonLanRefused(addr.to_string()));
        }
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(addr).await?.collect();
    if addrs.is_empty() {
        return Err(CliError::Remote(format!("could not resolve {addr}")));
    }
    for sa in &addrs {
        if !is_lan_or_loopback_ip(sa.ip()) {
            return Err(CliError::PlaintextNonLanRefused(addr.to_string()));
        }
    }
    Ok(addrs[0])
}

async fn open_raw_tcp(ep: &TargetEndpoint) -> Result<TcpStream, CliError> {
    match ep {
        TargetEndpoint::Direct(addr) => Ok(TcpStream::connect(addr).await?),
        TargetEndpoint::Relay {
            relay_addr,
            node_id,
            ..
        } => {
            let mut tcp = TcpStream::connect(relay_addr).await?;
            let mut conn_nonce = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut conn_nonce);
            RelayFrame::ClientConnect {
                node_id: *node_id,
                conn_nonce,
            }
            .write_to(&mut tcp)
            .await?;
            Ok(tcp)
        }
    }
}

pub async fn connect_and_auth(opts: ConnectOptions<'_>) -> Result<AuthenticatedStream, CliError> {
    let ep = TargetEndpoint::parse(opts.target)?;

    let mut stream: AuthenticatedStream = if opts.insecure_lan_plaintext {
        match &ep {
            TargetEndpoint::Direct(addr) => {
                let sa = resolve_lan_checked(addr).await?;
                Box::new(TcpStream::connect(sa).await?)
            }
            TargetEndpoint::Relay { .. } => {
                return Err(CliError::PlaintextNonLanRefused(opts.target.to_string()));
            }
        }
    } else {
        let kh_path = known_hosts_path(opts.config_dir);
        let mut kh = KnownHosts::load(&kh_path)?;
        let pin_key = ep.pin_key().to_string();

        let explicit_trust = opts.trust_fp.map(normalize_fingerprint).transpose()?;
        let pinned_fp = if let Some(ref tfp) = explicit_trust {
            Some(tfp.clone())
        } else {
            kh.entries
                .get(&pin_key)
                .map(|raw| normalize_fingerprint(raw).unwrap_or_else(|_| raw.to_lowercase()))
        };

        let (tls_cfg, observed) = build_client_tls_config(
            None,
            CertCheckPolicy::PinnedOrFirstUse {
                expected_fp: pinned_fp.clone(),
                trust_first_use: pinned_fp.is_none(),
            },
        )?;

        let tcp = open_raw_tcp(&ep).await?;
        let sni_host = match &ep {
            TargetEndpoint::Direct(addr) => extract_host_for_sni(addr),
            TargetEndpoint::Relay { .. } => "portico-node".to_string(),
        };
        let server_name = sni_host
            .try_into()
            .map_err(|_| CliError::Remote("invalid SNI hostname".into()))?;
        let connector = TlsConnector::from(tls_cfg);

        match connector.connect(server_name, tcp).await {
            Ok(tls) => {
                let actual = observed
                    .get()
                    .ok_or_else(|| CliError::Remote("missing peer certificate".into()))?;
                if let Some(ref tfp) = explicit_trust {
                    if &actual != tfp {
                        return Err(CliError::CertPinMismatch {
                            endpoint: pin_key,
                            expected: tfp.clone(),
                            actual,
                        });
                    }
                    kh.pin(&pin_key, &actual)?;
                    kh.save_atomic(&kh_path)?;
                } else {
                    match kh.check(&pin_key, &actual) {
                        TofuDecision::Trusted => {}
                        TofuDecision::FirstUse(fp) => {
                            // Never send ClientHello / claim token until user explicitly pins cert
                            return Err(CliError::UntrustedCert {
                                endpoint: pin_key,
                                fingerprint: fp,
                            });
                        }
                        TofuDecision::Mismatch { expected, actual } => {
                            return Err(CliError::CertPinMismatch {
                                endpoint: pin_key,
                                expected,
                                actual,
                            });
                        }
                    }
                }
                Box::new(tls)
            }
            Err(e) => {
                if let (Some(expected), Some(actual)) = (pinned_fp, observed.get()) {
                    if expected != actual {
                        return Err(CliError::CertPinMismatch {
                            endpoint: pin_key,
                            expected,
                            actual,
                        });
                    }
                }
                return Err(CliError::Io(e));
            }
        }
    };

    write_frame(
        &mut stream,
        &Frame::ClientHello {
            min_ver: PROTOCOL_VERSION_V0,
            max_ver: PROTOCOL_VERSION_V0,
            auth_kind: AuthKind::ClaimToken,
            credentials: opts.token.as_bytes().to_vec(),
            client_name: opts.client_name.to_string(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await?;

    match read_frame(&mut stream).await? {
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        } => Ok(stream),
        Frame::ServerHello {
            status: HelloStatus::AuthFailed,
            message,
            ..
        } => Err(CliError::AuthFailed(message)),
        Frame::ServerHello {
            status, message, ..
        } => Err(CliError::Remote(format!("{status:?}: {message}"))),
        other => Err(CliError::Remote(format!("unexpected hello response: {other:?}"))),
    }
}

pub async fn cmd_list(opts: ConnectOptions<'_>) -> Result<Vec<DeviceSummary>, CliError> {
    let mut stream = connect_and_auth(opts).await?;
    write_frame(&mut stream, &Frame::ListDevicesReq).await?;
    match read_frame(&mut stream).await? {
        Frame::ListDevicesResp { devices } => Ok(devices),
        Frame::Error { code, message } => {
            Err(CliError::Remote(format!("error {code}: {message}")))
        }
        other => Err(CliError::Remote(format!("unexpected response: {other:?}"))),
    }
}

pub fn cmd_trust(config_dir: &Path, target: &str, fp_hex: &str) -> Result<(), CliError> {
    let ep = TargetEndpoint::parse(target)?;
    let path = known_hosts_path(config_dir);
    let mut kh = KnownHosts::load(&path)?;
    kh.pin(ep.pin_key(), fp_hex)?;
    kh.save_atomic(&path)?;
    Ok(())
}
