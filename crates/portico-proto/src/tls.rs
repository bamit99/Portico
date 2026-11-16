use crate::identity::{cert_fingerprint_hex, normalize_fingerprint, NodeIdentity};
use crate::ProtoError;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error as RustlsError, SignatureScheme};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, Once, RwLock};
use std::time::Duration;

pub fn configure_tunnel_tcp(stream: &tokio::net::TcpStream) {
    configure_tunnel_tcp_with(
        stream,
        Duration::from_secs(2),
        Duration::from_secs(1),
        2,
        Duration::from_secs(4),
    );
}

pub fn configure_tunnel_tcp_with(
    stream: &tokio::net::TcpStream,
    keepalive_idle: Duration,
    keepalive_interval: Duration,
    keepalive_retries: u32,
    user_timeout: Duration,
) {
    let _ = stream.set_nodelay(true);
    let sock_ref = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(keepalive_idle)
        .with_interval(keepalive_interval)
        .with_retries(keepalive_retries);
    let _ = sock_ref.set_tcp_keepalive(&keepalive);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let _ = sock_ref.set_tcp_user_timeout(Some(user_timeout));
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let _ = user_timeout;
}

static CRYPTO_INIT: Once = Once::new();

pub fn init_crypto_provider() {
    CRYPTO_INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub fn is_lan_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return mapped.is_loopback() || mapped.is_private() || mapped.is_link_local();
            }
            let seg0 = v6.segments()[0];
            v6.is_loopback()
                || (seg0 & 0xfe00) == 0xfc00 // fc00::/7 ULA
                || (seg0 & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

#[derive(Debug, Default)]
pub struct PinAllowlist {
    pins: RwLock<Vec<String>>,
}

impl PinAllowlist {
    pub fn new(fps: Vec<String>) -> Self {
        let normalized = fps
            .into_iter()
            .filter_map(|f| normalize_fingerprint(&f).ok())
            .collect();
        Self {
            pins: RwLock::new(normalized),
        }
    }

    pub fn set_pins(&self, fps: Vec<String>) {
        let normalized = fps
            .into_iter()
            .filter_map(|f| normalize_fingerprint(&f).ok())
            .collect();
        *self.pins.write().unwrap() = normalized;
    }

    pub fn contains(&self, fp: &str) -> bool {
        let Ok(norm) = normalize_fingerprint(fp) else {
            return false;
        };
        self.pins.read().unwrap().iter().any(|p| p == &norm)
    }
}

#[derive(Debug, Default)]
pub struct ObservedFingerprint {
    inner: Mutex<Option<String>>,
}

impl ObservedFingerprint {
    pub fn get(&self) -> Option<String> {
        self.inner.lock().unwrap().clone()
    }

    fn set(&self, fp: String) {
        *self.inner.lock().unwrap() = Some(fp);
    }
}

pub enum CertCheckPolicy {
    PinnedOrFirstUse {
        expected_fp: Option<String>,
        trust_first_use: bool,
    },
}

#[derive(Debug)]
struct TofuServerCertVerifier {
    expected_fp: Option<String>,
    trust_first_use: bool,
    observed: Arc<ObservedFingerprint>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for TofuServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let actual_fp = cert_fingerprint_hex(end_entity.as_ref());
        self.observed.set(actual_fp.clone());
        match &self.expected_fp {
            Some(expected) => {
                let norm_expected = normalize_fingerprint(expected)
                    .map_err(|_| RustlsError::General("invalid pinned fingerprint".into()))?;
                if actual_fp != norm_expected {
                    return Err(RustlsError::General(format!(
                        "certificate fingerprint mismatch: expected {norm_expected}, got {actual_fp}"
                    )));
                }
                Ok(ServerCertVerified::assertion())
            }
            None => {
                if self.trust_first_use {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(RustlsError::General(format!(
                        "untrusted server certificate fingerprint: {actual_fp}"
                    )))
                }
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedClientVerifier {
    allowlist: Arc<PinAllowlist>,
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for PinnedClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, RustlsError> {
        let fp = cert_fingerprint_hex(end_entity.as_ref());
        if self.allowlist.contains(&fp) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(RustlsError::General(format!(
                "client certificate fingerprint not in allowlist: {fp}"
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

pub fn build_server_tls_config(
    identity: &NodeIdentity,
    client_allowlist: Option<Arc<PinAllowlist>>,
) -> Result<Arc<rustls::ServerConfig>, ProtoError> {
    init_crypto_provider();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| ProtoError::Tls(format!("tls13 server version: {e}")))?;

    let builder = if let Some(allowlist) = client_allowlist {
        builder.with_client_cert_verifier(Arc::new(PinnedClientVerifier { allowlist, algs }))
    } else {
        builder.with_no_client_auth()
    };

    let cert = CertificateDer::from(identity.cert_der.clone());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(identity.key_der.clone()));
    let cfg = builder
        .with_single_cert(vec![cert], key)
        .map_err(|e| ProtoError::Tls(format!("server cert: {e}")))?;
    Ok(Arc::new(cfg))
}

pub fn build_client_tls_config(
    client_identity: Option<&NodeIdentity>,
    policy: CertCheckPolicy,
) -> Result<(Arc<rustls::ClientConfig>, Arc<ObservedFingerprint>), ProtoError> {
    init_crypto_provider();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let observed = Arc::new(ObservedFingerprint::default());

    let (expected_fp, trust_first_use) = match policy {
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp,
            trust_first_use,
        } => (expected_fp, trust_first_use),
    };

    let verifier = Arc::new(TofuServerCertVerifier {
        expected_fp,
        trust_first_use,
        observed: observed.clone(),
        algs,
    });

    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| ProtoError::Tls(format!("tls13 client version: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier);

    let cfg = if let Some(id) = client_identity {
        let cert = CertificateDer::from(id.cert_der.clone());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der.clone()));
        builder
            .with_client_auth_cert(vec![cert], key)
            .map_err(|e| ProtoError::Tls(format!("client auth cert: {e}")))?
    } else {
        builder.with_no_client_auth()
    };

    Ok((Arc::new(cfg), observed))
}
