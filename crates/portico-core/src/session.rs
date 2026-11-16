use crate::config::LicenseTier;
use crate::logging::LogLevel;
use crate::state::DaemonState;
use crate::CoreError;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, LicenseTier as ProtoTier, TunnelStatus,
    CAP_USBIP_V1, PROTOCOL_VERSION_V0,
};
use portico_proto::tls::is_lan_or_loopback_ip;
use portico_proto::token::verify_claim_token;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Notify;

struct PeerGuard {
    state: Arc<DaemonState>,
}

impl PeerGuard {
    fn enter(state: Arc<DaemonState>) -> Self {
        state.active_peers.fetch_add(1, Ordering::Relaxed);
        Self { state }
    }
}

impl Drop for PeerGuard {
    fn drop(&mut self) {
        self.state.active_peers.fetch_sub(1, Ordering::Relaxed);
    }
}

fn map_tier(tier: LicenseTier) -> ProtoTier {
    match tier {
        LicenseTier::Free => ProtoTier::Free,
    }
}

pub async fn serve_portico_stream<S>(
    stream: S,
    peer_addr: SocketAddr,
    is_tls: bool,
    state: Arc<DaemonState>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    serve_portico_stream_with_stop(stream, peer_addr, is_tls, state, None).await
}

pub async fn serve_portico_stream_with_stop<S>(
    mut stream: S,
    peer_addr: SocketAddr,
    is_tls: bool,
    state: Arc<DaemonState>,
    stop_notify: Option<Arc<Notify>>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _guard = PeerGuard::enter(state.clone());

    let hello = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        read_frame(&mut stream),
    )
    .await
    {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Ok(()),
    };

    let Frame::ClientHello {
        min_ver,
        max_ver,
        auth_kind,
        credentials,
        client_name,
        capabilities: _,
    } = hello
    else {
        let _ = write_frame(
            &mut stream,
            &Frame::Error {
                code: 1,
                message: "expected ClientHello".into(),
            },
        )
        .await;
        return Ok(());
    };

    let (plaintext_lan, expected_token, tier) = {
        let cfg = state.config.lock().await;
        (
            cfg.config.plaintext_lan,
            cfg.config.claim_token.clone(),
            cfg.tier(),
        )
    };

    if !is_tls && (!plaintext_lan || !is_lan_or_loopback_ip(peer_addr.ip())) {
        state.log_and_emit(
            LogLevel::Warn,
            "session",
            &format!("rejected plaintext connection from {peer_addr}"),
        );
        let _ = write_frame(
            &mut stream,
            &Frame::ServerHello {
                chosen_ver: PROTOCOL_VERSION_V0,
                status: HelloStatus::PolicyRejected,
                server_name: "portico".into(),
                capabilities: CAP_USBIP_V1,
                tier: map_tier(tier),
                message: "TLS required for this route".into(),
            },
        )
        .await;
        return Ok(());
    }

    if min_ver > PROTOCOL_VERSION_V0 || max_ver < PROTOCOL_VERSION_V0 {
        let _ = write_frame(
            &mut stream,
            &Frame::ServerHello {
                chosen_ver: PROTOCOL_VERSION_V0,
                status: HelloStatus::UnsupportedVersion,
                server_name: "portico".into(),
                capabilities: CAP_USBIP_V1,
                tier: map_tier(tier),
                message: "unsupported protocol version".into(),
            },
        )
        .await;
        return Ok(());
    }

    let authed = match auth_kind {
        AuthKind::ClaimToken => verify_claim_token(&expected_token, &credentials),
        AuthKind::AccountCredentials => false,
    };

    if !authed {
        state.log_and_emit(
            LogLevel::Warn,
            "session",
            &format!("authentication failed for peer {peer_addr} ({client_name})"),
        );
        let _ = write_frame(
            &mut stream,
            &Frame::ServerHello {
                chosen_ver: PROTOCOL_VERSION_V0,
                status: HelloStatus::AuthFailed,
                server_name: "portico".into(),
                capabilities: CAP_USBIP_V1,
                tier: map_tier(tier),
                message: "invalid claim token".into(),
            },
        )
        .await;
        return Ok(());
    }

    state.log_and_emit(
        LogLevel::Info,
        "session",
        &format!("peer {peer_addr} ({client_name}) authenticated"),
    );

    write_frame(
        &mut stream,
        &Frame::ServerHello {
            chosen_ver: PROTOCOL_VERSION_V0,
            status: HelloStatus::Ok,
            server_name: "portico".into(),
            capabilities: CAP_USBIP_V1,
            tier: map_tier(tier),
            message: String::new(),
        },
    )
    .await?;

    loop {
        let frame = if let Some(ref stop) = stop_notify {
            tokio::select! {
                _ = stop.notified() => return Ok(()),
                r = read_frame(&mut stream) => match r {
                    Ok(f) => f,
                    Err(portico_proto::ProtoError::Io(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        return Ok(());
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        } else {
            match read_frame(&mut stream).await {
                Ok(f) => f,
                Err(portico_proto::ProtoError::Io(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        };

        match frame {
            Frame::ListDevicesReq => {
                let devices = state.registry.list_shareable_summaries().await;
                write_frame(&mut stream, &Frame::ListDevicesResp { devices }).await?;
            }
            Frame::Ping { nonce } => {
                write_frame(&mut stream, &Frame::Pong { nonce }).await?;
            }
            Frame::OpenTunnelReq { bus_id } => {
                let peer_label = format!("{client_name}@{peer_addr}");
                match state.registry.try_claim_device(&bus_id, &peer_label).await {
                    Ok(claim) => {
                        let _done_guard = ClaimDoneGuard(claim.done_tx.clone());
                        if let Err(e) = write_frame(
                            &mut stream,
                            &Frame::OpenTunnelResp {
                                status: TunnelStatus::Ok,
                                message: String::new(),
                            },
                        )
                        .await
                        {
                            drop(claim);
                            let ignored = state.config.lock().await.config.ignored_devices.clone();
                            state.registry.release_claim(&bus_id, &ignored).await;
                            return Err(e.into());
                        }

                        let res = run_tunnel_with_cancel(
                            &mut stream,
                            claim.usbip_server,
                            claim.cancel,
                            stop_notify.clone(),
                        )
                        .await;
                        let ignored = state.config.lock().await.config.ignored_devices.clone();
                        state.registry.release_claim(&bus_id, &ignored).await;
                        return res;
                    }
                    Err(status) => {
                        let msg = match status {
                            TunnelStatus::NotFound => "device not found",
                            TunnelStatus::InUse => "device already in use",
                            TunnelStatus::Ignored => "device is ignored or awaiting permission",
                            TunnelStatus::Ok => "",
                        };
                        write_frame(
                            &mut stream,
                            &Frame::OpenTunnelResp {
                                status,
                                message: msg.into(),
                            },
                        )
                        .await?;
                    }
                }
            }
            _ => {
                write_frame(
                    &mut stream,
                    &Frame::Error {
                        code: 2,
                        message: "unexpected frame".into(),
                    },
                )
                .await?;
            }
        }
    }
}

pub async fn serve_reverse_data_stream<S>(
    stream: S,
    peer_addr: SocketAddr,
    bus_id: &str,
    conn_nonce: [u8; 16],
    state: Arc<DaemonState>,
    stop_notify: Option<Arc<Notify>>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = stream;
    let _guard = PeerGuard::enter(state.clone());
    write_frame(&mut stream, &Frame::ReverseDataHello { conn_nonce }).await?;

    let peer_label = format!("reverse@{peer_addr}");
    match state.registry.try_claim_device(bus_id, &peer_label).await {
        Ok(claim) => {
            let _done_guard = ClaimDoneGuard(claim.done_tx.clone());
            if let Err(e) = write_frame(
                &mut stream,
                &Frame::OpenTunnelResp {
                    status: TunnelStatus::Ok,
                    message: String::new(),
                },
            )
            .await
            {
                drop(claim);
                let ignored = state.config.lock().await.config.ignored_devices.clone();
                state.registry.release_claim(bus_id, &ignored).await;
                return Err(e.into());
            }
            let res = run_tunnel_with_cancel(
                &mut stream,
                claim.usbip_server,
                claim.cancel,
                stop_notify,
            )
            .await;
            let ignored = state.config.lock().await.config.ignored_devices.clone();
            state.registry.release_claim(bus_id, &ignored).await;
            res
        }
        Err(status) => {
            write_frame(
                &mut stream,
                &Frame::OpenTunnelResp {
                    status,
                    message: "device unavailable for reverse tunnel".into(),
                },
            )
            .await?;
            Ok(())
        }
    }
}

struct ClaimDoneGuard(Arc<tokio::sync::watch::Sender<bool>>);

impl Drop for ClaimDoneGuard {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

async fn run_tunnel_with_cancel<S>(
    stream: &mut S,
    usbip_server: Arc<usbip::UsbIpServer>,
    claim_cancel: Arc<Notify>,
    stop_notify: Option<Arc<Notify>>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Some(stop) = stop_notify {
        tokio::select! {
            _ = claim_cancel.notified() => Ok(()),
            _ = stop.notified() => Ok(()),
            r = usbip::handler(stream, usbip_server) => r.map_err(CoreError::Io),
        }
    } else {
        tokio::select! {
            _ = claim_cancel.notified() => Ok(()),
            r = usbip::handler(stream, usbip_server) => r.map_err(CoreError::Io),
        }
    }
}
