use crate::bridge::bridge_already_open_tunnel;
use crate::transport::{AsyncRw, AuthenticatedStream};
use crate::CliError;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, DeviceSummary, Frame, HelloStatus, TunnelStatus,
    CAP_USBIP_V1, PROTOCOL_VERSION_V0,
};
use portico_proto::identity::{cert_fingerprint_hex, normalize_fingerprint, NodeIdentity};
use portico_proto::tls::{build_server_tls_config, configure_tunnel_tcp, PinAllowlist};
use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

pub struct ReverseControlHandle {
    pub server_fp: String,
    pub devices: Vec<DeviceSummary>,
}

enum ControlRequest {
    List(oneshot::Sender<Result<Vec<DeviceSummary>, CliError>>),
    RequestData {
        conn_nonce: [u8; 16],
        bus_id: String,
    },
}

#[derive(Clone)]
enum ControlSlot {
    Connecting,
    Ready(mpsc::Sender<ControlRequest>),
}

struct ReverseInner {
    controls: Mutex<HashMap<String, ControlSlot>>,
    pending_data: Mutex<HashMap<[u8; 16], oneshot::Sender<Result<AuthenticatedStream, CliError>>>>,
    ready_tx: Mutex<Option<mpsc::Sender<ReverseControlHandle>>>,
    ready_rx: Mutex<mpsc::Receiver<ReverseControlHandle>>,
    claim_token: String,
}

pub struct ReverseListener {
    addr: SocketAddr,
    identity: NodeIdentity,
    inner: Arc<ReverseInner>,
    accept_task: JoinHandle<()>,
}

impl Drop for ReverseListener {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

impl ReverseListener {
    pub fn load_or_create_identity(config_dir: &Path) -> Result<NodeIdentity, CliError> {
        Ok(NodeIdentity::load_or_generate(
            config_dir,
            "client_identity",
            "portico-client",
        )?)
    }

    pub async fn bind(
        bind_addr: SocketAddr,
        identity: NodeIdentity,
        allowed_server_fps: Vec<String>,
        claim_token: String,
    ) -> Result<Self, CliError> {
        if allowed_server_fps.is_empty() {
            return Err(CliError::Remote(
                "at least one --allow-server SHA-256 fingerprint is required".into(),
            ));
        }
        let normalized_fps = allowed_server_fps
            .into_iter()
            .map(|fp| normalize_fingerprint(&fp))
            .collect::<Result<Vec<_>, _>>()?;

        let allowlist = Arc::new(PinAllowlist::new(normalized_fps));
        let tls_cfg = build_server_tls_config(&identity, Some(allowlist))?;
        let acceptor = TlsAcceptor::from(tls_cfg);
        let listener = TcpListener::bind(bind_addr).await?;
        let addr = listener.local_addr()?;

        let (ready_tx, ready_rx) = mpsc::channel(16);
        let inner = Arc::new(ReverseInner {
            controls: Mutex::new(HashMap::new()),
            pending_data: Mutex::new(HashMap::new()),
            ready_tx: Mutex::new(Some(ready_tx)),
            ready_rx: Mutex::new(ready_rx),
            claim_token,
        });

        let inner_clone = inner.clone();
        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((tcp, _peer)) = listener.accept().await else {
                    break;
                };
                configure_tunnel_tcp(&tcp);
                let acc = acceptor.clone();
                let inn = inner_clone.clone();
                tokio::spawn(async move {
                    let Ok(tls) = acc.accept(tcp).await else {
                        return;
                    };
                    let peer_fp = {
                        let (_, conn) = tls.get_ref();
                        conn.peer_certificates()
                            .and_then(|c| c.first())
                            .map(|c| cert_fingerprint_hex(c.as_ref()))
                    };
                    let Some(peer_fp) = peer_fp else {
                        return;
                    };

                    let slot = {
                        let mut guard = inn.controls.lock().await;
                        match guard.get(&peer_fp).cloned() {
                            None => {
                                guard.insert(peer_fp.clone(), ControlSlot::Connecting);
                                None
                            }
                            Some(s) => Some(s),
                        }
                    };
                    match slot {
                        None => {
                            let _ = handle_control_conn(tls, peer_fp, inn).await;
                        }
                        Some(ControlSlot::Ready(_)) => {
                            let _ = handle_data_conn(tls, inn).await;
                        }
                        Some(ControlSlot::Connecting) => {
                            // Duplicate connection before initial control handshake finished
                        }
                    }
                });
            }
        });

        Ok(Self {
            addr,
            identity,
            inner,
            accept_task,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn fingerprint_hex(&self) -> String {
        self.identity.fingerprint_hex()
    }

    pub async fn wait_for_server_ready(&self) -> Result<ReverseControlHandle, CliError> {
        self.inner
            .ready_rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| CliError::Remote("reverse listener closed".into()))
    }

    async fn control_sender(&self, server_fp: &str) -> Result<mpsc::Sender<ControlRequest>, CliError> {
        match self.inner.controls.lock().await.get(server_fp).cloned() {
            Some(ControlSlot::Ready(tx)) => Ok(tx),
            _ => Err(CliError::Remote("reverse server not connected".into())),
        }
    }

    pub async fn list_devices(&self, server_fp: &str) -> Result<Vec<DeviceSummary>, CliError> {
        let tx = self.control_sender(server_fp).await?;
        let (rtx, rrx) = oneshot::channel();
        tx.send(ControlRequest::List(rtx))
            .await
            .map_err(|_| CliError::Remote("control channel closed".into()))?;
        rrx.await
            .map_err(|_| CliError::Remote("control response dropped".into()))?
    }

    pub async fn open_reverse_tunnel(
        &self,
        server_fp: &str,
        bus_id: &str,
    ) -> Result<(SocketAddr, JoinHandle<Result<(), CliError>>), CliError> {
        let tx = self.control_sender(server_fp).await?;

        let mut conn_nonce = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut conn_nonce);
        let (dtx, drx) = oneshot::channel();
        self.inner
            .pending_data
            .lock()
            .await
            .insert(conn_nonce, dtx);

        if tx
            .send(ControlRequest::RequestData {
                conn_nonce,
                bus_id: bus_id.to_string(),
            })
            .await
            .is_err()
        {
            self.inner.pending_data.lock().await.remove(&conn_nonce);
            return Err(CliError::Remote("reverse control channel closed".into()));
        }

        let stream = match tokio::time::timeout(Duration::from_secs(10), drx).await {
            Ok(Ok(Ok(s))) => s,
            Ok(Ok(Err(e))) => return Err(e),
            _ => {
                self.inner.pending_data.lock().await.remove(&conn_nonce);
                return Err(CliError::Remote(
                    "timed out waiting for reverse data connection".into(),
                ));
            }
        };

        bridge_already_open_tunnel(stream).await
    }
}

struct ControlCleanup {
    peer_fp: String,
    inner: Arc<ReverseInner>,
}

impl Drop for ControlCleanup {
    fn drop(&mut self) {
        let fp = self.peer_fp.clone();
        if let Ok(mut g) = self.inner.controls.try_lock() {
            g.remove(&fp);
        } else {
            let inn = self.inner.clone();
            tokio::spawn(async move {
                inn.controls.lock().await.remove(&fp);
            });
        }
    }
}

async fn handle_control_conn<S>(
    mut stream: S,
    peer_fp: String,
    inner: Arc<ReverseInner>,
) -> Result<(), CliError>
where
    S: AsyncRw + 'static,
{
    let _cleanup = ControlCleanup {
        peer_fp: peer_fp.clone(),
        inner: inner.clone(),
    };

    write_frame(
        &mut stream,
        &Frame::ClientHello {
            min_ver: PROTOCOL_VERSION_V0,
            max_ver: PROTOCOL_VERSION_V0,
            auth_kind: AuthKind::ClaimToken,
            credentials: inner.claim_token.as_bytes().to_vec(),
            client_name: "portico-reverse-listener".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await?;

    match read_frame(&mut stream).await? {
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        } => {}
        Frame::ServerHello {
            status, message, ..
        } => return Err(CliError::AuthFailed(format!("{status:?}: {message}"))),
        _ => return Err(CliError::Remote("unexpected ServerHello".into())),
    }

    write_frame(&mut stream, &Frame::ListDevicesReq).await?;
    let initial_devices = match read_frame(&mut stream).await? {
        Frame::ListDevicesResp { devices } => devices,
        _ => Vec::new(),
    };

    let (tx, mut rx) = mpsc::channel::<ControlRequest>(32);
    inner
        .controls
        .lock()
        .await
        .insert(peer_fp.clone(), ControlSlot::Ready(tx));

    if let Some(ref ready_tx) = *inner.ready_tx.lock().await {
        let _ = ready_tx
            .send(ReverseControlHandle {
                server_fp: peer_fp.clone(),
                devices: initial_devices,
            })
            .await;
    }

    loop {
        tokio::select! {
            maybe_req = rx.recv() => {
                let Some(req) = maybe_req else { break; };
                match req {
                    ControlRequest::List(reply) => {
                        if write_frame(&mut stream, &Frame::ListDevicesReq)
                            .await
                            .is_err()
                        {
                            let _ = reply.send(Err(CliError::Remote("control write failed".into())));
                            break;
                        }
                        match read_frame(&mut stream).await {
                            Ok(Frame::ListDevicesResp { devices }) => {
                                let _ = reply.send(Ok(devices));
                            }
                            Ok(other) => {
                                let _ = reply.send(Err(CliError::Remote(format!("unexpected: {other:?}"))));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(CliError::Proto(e)));
                                break;
                            }
                        }
                    }
                    ControlRequest::RequestData { conn_nonce, bus_id } => {
                        if write_frame(
                            &mut stream,
                            &Frame::RequestReverseDataConn { conn_nonce, bus_id },
                        )
                        .await
                        .is_err()
                        {
                            if let Some(dtx) = inner.pending_data.lock().await.remove(&conn_nonce) {
                                let _ = dtx.send(Err(CliError::Remote("reverse control write failed".into())));
                            }
                            break;
                        }
                    }
                }
            }
            incoming = read_frame(&mut stream) => {
                match incoming {
                    Ok(Frame::Ping { nonce }) => {
                        if write_frame(&mut stream, &Frame::Pong { nonce }).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
    }

    Ok(())
}

async fn handle_data_conn<S>(mut stream: S, inner: Arc<ReverseInner>) -> Result<(), CliError>
where
    S: AsyncRw + 'static,
{
    let Frame::ReverseDataHello { conn_nonce } = read_frame(&mut stream).await? else {
        return Ok(());
    };
    let resp = read_frame(&mut stream).await?;
    let Some(tx) = inner.pending_data.lock().await.remove(&conn_nonce) else {
        return Ok(());
    };
    match resp {
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        } => {
            let _ = tx.send(Ok(Box::new(stream)));
        }
        Frame::OpenTunnelResp { status, message } => {
            let _ = tx.send(Err(CliError::Remote(format!(
                "reverse tunnel rejected ({status:?}): {message}"
            ))));
        }
        other => {
            let _ = tx.send(Err(CliError::Remote(format!(
                "unexpected reverse data response: {other:?}"
            ))));
        }
    }
    Ok(())
}
