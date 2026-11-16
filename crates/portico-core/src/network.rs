use crate::config::LicenseTier;
use crate::logging::{DaemonEvent, LogLevel};
use crate::session::{serve_portico_stream_with_stop, serve_reverse_data_stream};
use crate::state::{DaemonState, RelayStatus};
use crate::CoreError;
use portico_proto::discovery::DiscoveryPacket;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, LicenseTier as ProtoTier, CAP_USBIP_V1,
    PROTOCOL_VERSION_V0,
};
use portico_proto::relay_proto::{format_relay_uri, hash_slot_secret, RelayFrame};
use portico_proto::tls::{
    build_client_tls_config, build_server_tls_config, configure_tunnel_tcp, CertCheckPolicy,
};
use portico_proto::token::verify_claim_token;
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn extract_sni_host(addr: &str) -> String {
    let raw = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    raw.trim_start_matches('[').trim_end_matches(']').to_string()
}

#[derive(Default)]
pub struct NetworkSupervisor {
    tcp_task: Mutex<Option<JoinHandle<()>>>,
    discovery_task: Mutex<Option<JoinHandle<()>>>,
    relay_task: Mutex<Option<JoinHandle<()>>>,
    reverse_tasks: Mutex<Vec<JoinHandle<()>>>,
    session_stop: Mutex<Option<Arc<Notify>>>,
    bound_tcp_addr: Mutex<Option<SocketAddr>>,
    bound_udp_addr: Mutex<Option<SocketAddr>>,
}

impl NetworkSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_server(
        &self,
        state: Arc<DaemonState>,
        tcp_bind: SocketAddr,
        udp_bind: Option<SocketAddr>,
    ) -> Result<(SocketAddr, Option<SocketAddr>), CoreError> {
        if let Some(addr) = *self.bound_tcp_addr.lock().await {
            let udp = *self.bound_udp_addr.lock().await;
            return Ok((addr, udp));
        }

        let stop_notify = Arc::new(Notify::new());
        *self.session_stop.lock().await = Some(stop_notify.clone());

        let listener = TcpListener::bind(tcp_bind).await?;
        let local_tcp = listener.local_addr()?;
        *self.bound_tcp_addr.lock().await = Some(local_tcp);

        let local_udp = if let Some(ubind) = udp_bind {
            let sock = UdpSocket::bind(ubind).await?;
            let addr = sock.local_addr()?;
            *self.bound_udp_addr.lock().await = Some(addr);
            let st = state.clone();
            let tcp_port = local_tcp.port();
            let handle = tokio::spawn(async move {
                run_discovery_loop(sock, st, tcp_port).await;
            });
            *self.discovery_task.lock().await = Some(handle);
            Some(addr)
        } else {
            None
        };

        let st = state.clone();
        let tcp_stop = stop_notify.clone();
        let tcp_handle = tokio::spawn(async move {
            run_tcp_accept_loop(listener, st, tcp_stop).await;
        });
        *self.tcp_task.lock().await = Some(tcp_handle);

        state.server_running.store(true, Ordering::SeqCst);
        state.emit(DaemonEvent::ServerState {
            running: true,
            port: local_tcp.port(),
        });
        state.log_and_emit(
            LogLevel::Info,
            "network",
            &format!("Portico server listening on {local_tcp}"),
        );

        self.sync_relay(state.clone()).await;
        self.sync_reverse_clients(state).await;

        Ok((local_tcp, local_udp))
    }

    pub async fn stop_server(&self, state: &Arc<DaemonState>) {
        if let Some(stop) = self.session_stop.lock().await.take() {
            stop.notify_waiters();
        }
        if let Some(h) = self.tcp_task.lock().await.take() {
            h.abort();
        }
        if let Some(h) = self.discovery_task.lock().await.take() {
            h.abort();
        }
        if let Some(h) = self.relay_task.lock().await.take() {
            h.abort();
        }
        for h in self.reverse_tasks.lock().await.drain(..) {
            h.abort();
        }
        *self.bound_tcp_addr.lock().await = None;
        *self.bound_udp_addr.lock().await = None;
        state.server_running.store(false, Ordering::SeqCst);
        *state.relay_status.write().await = RelayStatus::Disabled;
        state.emit(DaemonEvent::RelayState {
            status: "disabled".into(),
            address: String::new(),
        });
        let port = state.config.lock().await.config.port;
        state.emit(DaemonEvent::ServerState {
            running: false,
            port,
        });
        state.log_and_emit(LogLevel::Info, "network", "Portico server stopped");
    }

    pub async fn sync_relay(&self, state: Arc<DaemonState>) {
        if let Some(h) = self.relay_task.lock().await.take() {
            h.abort();
        }
        if !state.server_running.load(Ordering::SeqCst) {
            *state.relay_status.write().await = RelayStatus::Disabled;
            return;
        }
        let relay_cfg = state.config.lock().await.config.relay.clone();
        if !relay_cfg.enabled || relay_cfg.endpoint.trim().is_empty() {
            *state.relay_status.write().await = RelayStatus::Disabled;
            state.emit(DaemonEvent::RelayState {
                status: "disabled".into(),
                address: String::new(),
            });
            return;
        }

        *state.relay_status.write().await = RelayStatus::Initializing;
        state.emit(DaemonEvent::RelayState {
            status: "initializing".into(),
            address: String::new(),
        });

        let stop_notify = self.session_stop.lock().await.clone();
        let handle = tokio::spawn(async move {
            run_relay_loop(state, relay_cfg.endpoint, relay_cfg.slot_secret_hex, stop_notify).await;
        });
        *self.relay_task.lock().await = Some(handle);
    }

    pub async fn sync_reverse_clients(&self, state: Arc<DaemonState>) {
        let mut tasks = self.reverse_tasks.lock().await;
        for h in tasks.drain(..) {
            h.abort();
        }
        if !state.server_running.load(Ordering::SeqCst) {
            return;
        }
        let stop_notify = self.session_stop.lock().await.clone();
        let entries = state.config.lock().await.config.reverse_clients.clone();
        for entry in entries.into_iter().filter(|e| e.enabled) {
            let st = state.clone();
            let stop = stop_notify.clone();
            tasks.push(tokio::spawn(async move {
                run_reverse_client_loop(st, entry.addr, entry.cert_sha256, stop).await;
            }));
        }
    }

    pub async fn bound_tcp_addr(&self) -> Option<SocketAddr> {
        *self.bound_tcp_addr.lock().await
    }

    pub async fn bound_udp_addr(&self) -> Option<SocketAddr> {
        *self.bound_udp_addr.lock().await
    }
}

async fn run_tcp_accept_loop(
    listener: TcpListener,
    state: Arc<DaemonState>,
    stop_notify: Arc<Notify>,
) {
    loop {
        let Ok((socket, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        configure_tunnel_tcp(&socket);
        let st = state.clone();
        let stop = stop_notify.clone();
        tokio::spawn(async move {
            let mut first = [0u8; 1];
            let Ok(Ok(_)) =
                tokio::time::timeout(Duration::from_secs(10), socket.peek(&mut first)).await
            else {
                return;
            };
            if first[0] == 0x16 {
                let id = st.identity.read().await.clone();
                let Ok(tls_cfg) = build_server_tls_config(&id, None) else {
                    return;
                };
                let acceptor = TlsAcceptor::from(tls_cfg);
                match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(socket)).await {
                    Ok(Ok(tls_stream)) => {
                        let _ =
                            serve_portico_stream_with_stop(tls_stream, peer, true, st, Some(stop))
                                .await;
                    }
                    Ok(Err(e)) => {
                        st.log_and_emit(
                            LogLevel::Warn,
                            "tls",
                            &format!("TLS handshake failed from {peer}: {e}"),
                        );
                    }
                    Err(_) => {}
                }
            } else {
                let _ = serve_portico_stream_with_stop(socket, peer, false, st, Some(stop)).await;
            }
        });
    }
}

async fn run_discovery_loop(sock: UdpSocket, state: Arc<DaemonState>, tcp_port: u16) {
    let mut buf = [0u8; 256];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else {
            continue;
        };
        if let Ok(DiscoveryPacket::Request { nonce }) = DiscoveryPacket::decode(&buf[..n]) {
            let (tls_required, plaintext_lan) = {
                let cfg = state.config.lock().await;
                (cfg.config.tls_required, cfg.config.plaintext_lan)
            };
            let id = state.identity.read().await;
            let digest = Sha256::digest(&id.cert_der);
            let mut cert_fp_prefix = [0u8; 8];
            cert_fp_prefix.copy_from_slice(&digest[..8]);
            drop(id);

            let resp = DiscoveryPacket::Response {
                nonce,
                tcp_port,
                tls_required,
                plaintext_lan,
                cert_fp_prefix,
                node_name: "Portico".into(),
            }
            .encode();
            let _ = sock.send_to(&resp, from).await;
        }
    }
}

async fn run_relay_loop(
    state: Arc<DaemonState>,
    endpoint: String,
    slot_secret_hex: String,
    stop_notify: Option<Arc<Notify>>,
) {
    let secret_bytes =
        hex::decode(&slot_secret_hex).unwrap_or_else(|_| slot_secret_hex.into_bytes());
    let secret_hash = hash_slot_secret(&secret_bytes);

    let mut backoff = Duration::from_secs(1);
    loop {
        let node_id = state.identity.read().await.node_id();
        let err_msg: String = match TcpStream::connect(&endpoint).await {
            Ok(mut ctrl) => {
                configure_tunnel_tcp(&ctrl);
                let reg = RelayFrame::ControlRegister {
                    node_id,
                    slot_secret_hash: secret_hash,
                };
                if let Err(e) = reg.write_to(&mut ctrl).await {
                    e.to_string()
                } else {
                    match RelayFrame::read_from(&mut ctrl).await {
                        Ok(RelayFrame::Registered) => {
                            backoff = Duration::from_secs(1);
                            let uri = format_relay_uri(&endpoint, &node_id);
                            *state.relay_status.write().await = RelayStatus::Ready(uri.clone());
                            state.emit(DaemonEvent::RelayState {
                                status: "ready".into(),
                                address: uri,
                            });

                            loop {
                                match RelayFrame::read_from(&mut ctrl).await {
                                    Ok(RelayFrame::IncomingClient { conn_nonce }) => {
                                        let ep = endpoint.clone();
                                        let st = state.clone();
                                        let stop = stop_notify.clone();
                                        tokio::spawn(async move {
                                            let Ok(mut data) = TcpStream::connect(&ep).await else {
                                                return;
                                            };
                                            configure_tunnel_tcp(&data);
                                            let peer = data.peer_addr().unwrap_or_else(|_| {
                                                SocketAddr::from(([0, 0, 0, 0], 0))
                                            });
                                            let accept = RelayFrame::ServerAccept {
                                                node_id,
                                                slot_secret_hash: secret_hash,
                                                conn_nonce,
                                            };
                                            if accept.write_to(&mut data).await.is_err() {
                                                return;
                                            }
                                            let id = st.identity.read().await.clone();
                                            let Ok(tls_cfg) = build_server_tls_config(&id, None)
                                            else {
                                                return;
                                            };
                                            let acceptor = TlsAcceptor::from(tls_cfg);
                                            if let Ok(tls_stream) = acceptor.accept(data).await {
                                                let _ = serve_portico_stream_with_stop(
                                                    tls_stream,
                                                    peer,
                                                    true,
                                                    st,
                                                    stop,
                                                )
                                                .await;
                                            }
                                        });
                                    }
                                    Ok(_) => {}
                                    Err(e) => break e.to_string(),
                                }
                            }
                        }
                        Ok(RelayFrame::RegisterRejected) => "relay registration rejected".into(),
                        Ok(_) => "unexpected relay registration response".into(),
                        Err(e) => e.to_string(),
                    }
                }
            }
            Err(e) => e.to_string(),
        };

        *state.relay_status.write().await = RelayStatus::Error(err_msg);
        state.emit(DaemonEvent::RelayState {
            status: "error".into(),
            address: String::new(),
        });

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn run_reverse_client_loop(
    state: Arc<DaemonState>,
    addr: String,
    pinned_fp: String,
    stop_notify: Option<Arc<Notify>>,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let id = state.identity.read().await.clone();
        let Ok((cli_cfg, _)) = build_client_tls_config(
            Some(&id),
            CertCheckPolicy::PinnedOrFirstUse {
                expected_fp: Some(pinned_fp.clone()),
                trust_first_use: false,
            },
        ) else {
            return;
        };
        let connector = TlsConnector::from(cli_cfg);
        let host = extract_sni_host(&addr);
        let Ok(server_name) = host.try_into() else {
            state.log_and_emit(
                LogLevel::Warn,
                "reverse",
                &format!("invalid reverse client SNI host for '{addr}'"),
            );
            return;
        };

        if let Ok(tcp) = TcpStream::connect(&addr).await {
            configure_tunnel_tcp(&tcp);
            let peer = tcp
                .peer_addr()
                .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
            if let Ok(mut tls) = connector.connect(server_name, tcp).await {
                backoff = Duration::from_secs(1);
                let _ = handle_reverse_control_stream(
                    &mut tls,
                    peer,
                    &addr,
                    &pinned_fp,
                    state.clone(),
                    stop_notify.clone(),
                )
                .await;
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

async fn handle_reverse_control_stream<S>(
    stream: &mut S,
    peer: SocketAddr,
    addr: &str,
    pinned_fp: &str,
    state: Arc<DaemonState>,
    stop_notify: Option<Arc<Notify>>,
) -> Result<(), CoreError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let Frame::ClientHello {
        min_ver,
        max_ver,
        auth_kind,
        credentials,
        client_name,
        ..
    } = read_frame(stream).await?
    else {
        return Ok(());
    };

    let (expected_token, tier) = {
        let cfg = state.config.lock().await;
        (cfg.config.claim_token.clone(), cfg.tier())
    };
    let proto_tier = match tier {
        LicenseTier::Free => ProtoTier::Free,
    };

    if min_ver > PROTOCOL_VERSION_V0 || max_ver < PROTOCOL_VERSION_V0 {
        write_frame(
            stream,
            &Frame::ServerHello {
                chosen_ver: PROTOCOL_VERSION_V0,
                status: HelloStatus::UnsupportedVersion,
                server_name: "portico".into(),
                capabilities: CAP_USBIP_V1,
                tier: proto_tier,
                message: "unsupported version".into(),
            },
        )
        .await?;
        return Ok(());
    }

    let authed =
        matches!(auth_kind, AuthKind::ClaimToken) && verify_claim_token(&expected_token, &credentials);
    if !authed {
        write_frame(
            stream,
            &Frame::ServerHello {
                chosen_ver: PROTOCOL_VERSION_V0,
                status: HelloStatus::AuthFailed,
                server_name: "portico".into(),
                capabilities: CAP_USBIP_V1,
                tier: proto_tier,
                message: "invalid claim token".into(),
            },
        )
        .await?;
        return Ok(());
    }

    write_frame(
        stream,
        &Frame::ServerHello {
            chosen_ver: PROTOCOL_VERSION_V0,
            status: HelloStatus::Ok,
            server_name: "portico".into(),
            capabilities: CAP_USBIP_V1,
            tier: proto_tier,
            message: String::new(),
        },
    )
    .await?;

    state.log_and_emit(
        LogLevel::Info,
        "reverse",
        &format!("reverse control session ready with {client_name} ({peer})"),
    );

    loop {
        let frame = read_frame(stream).await?;
        match frame {
            Frame::ListDevicesReq => {
                let devices = state.registry.list_shareable_summaries().await;
                write_frame(stream, &Frame::ListDevicesResp { devices }).await?;
            }
            Frame::Ping { nonce } => {
                write_frame(stream, &Frame::Pong { nonce }).await?;
            }
            Frame::RequestReverseDataConn { conn_nonce, bus_id } => {
                let st = state.clone();
                let addr_owned = addr.to_string();
                let fp_owned = pinned_fp.to_string();
                let stop = stop_notify.clone();
                tokio::spawn(async move {
                    let id = st.identity.read().await.clone();
                    let Ok((cli_cfg, _)) = build_client_tls_config(
                        Some(&id),
                        CertCheckPolicy::PinnedOrFirstUse {
                            expected_fp: Some(fp_owned),
                            trust_first_use: false,
                        },
                    ) else {
                        return;
                    };
                    let host = extract_sni_host(&addr_owned);
                    let Ok(sni) = host.try_into() else {
                        return;
                    };
                    let Ok(tcp) = TcpStream::connect(&addr_owned).await else {
                        return;
                    };
                    configure_tunnel_tcp(&tcp);
                    let peer = tcp
                        .peer_addr()
                        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
                    if let Ok(tls) = TlsConnector::from(cli_cfg).connect(sni, tcp).await {
                        let _ = serve_reverse_data_stream(
                            tls,
                            peer,
                            &bus_id,
                            conn_nonce,
                            st,
                            stop,
                        )
                        .await;
                    }
                });
            }
            _ => {}
        }
    }
}
