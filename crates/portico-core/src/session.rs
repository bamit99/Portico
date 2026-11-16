use crate::config::LicenseTier;
use crate::logging::{DaemonEvent, LogLevel};
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
    mut stream: S,
    peer_addr: SocketAddr,
    is_tls: bool,
    state: Arc<DaemonState>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _guard = PeerGuard::enter(state.clone());

    let hello = match read_frame(&mut stream).await {
        Ok(f) => f,
        Err(e) => return Err(e.into()),
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
        let frame = match read_frame(&mut stream).await {
            Ok(f) => f,
            Err(portico_proto::ProtoError::Io(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
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
                match state.registry.check_tunnel_eligible(&bus_id).await {
                    Ok(()) => {
                        write_frame(
                            &mut stream,
                            &Frame::OpenTunnelResp {
                                status: TunnelStatus::Ok,
                                message: String::new(),
                            },
                        )
                        .await?;
                        let peer_label = format!("{client_name}@{peer_addr}");
                        state.registry.mark_in_use(&bus_id, &peer_label).await;
                        state.emit(DaemonEvent::DeviceState {
                            bus_id: bus_id.clone(),
                            state: "in_use".into(),
                            peer: peer_label,
                        });
                        let res = usbip::handler(&mut stream, state.registry.usbip_server()).await;
                        let ignored = state.config.lock().await.config.ignored_devices.clone();
                        state.registry.mark_released(&bus_id, &ignored).await;
                        let final_state = state
                            .registry
                            .list_devices()
                            .await
                            .into_iter()
                            .find(|d| d.bus_id == bus_id)
                            .map(|d| d.state.label().to_string())
                            .unwrap_or_else(|| "detached".into());
                        state.emit(DaemonEvent::DeviceState {
                            bus_id,
                            state: final_state,
                            peer: "-".into(),
                        });
                        return res.map_err(CoreError::Io);
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
    mut stream: S,
    peer_addr: SocketAddr,
    bus_id: &str,
    conn_nonce: [u8; 16],
    state: Arc<DaemonState>,
) -> Result<(), CoreError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let _guard = PeerGuard::enter(state.clone());
    write_frame(&mut stream, &Frame::ReverseDataHello { conn_nonce }).await?;

    match state.registry.check_tunnel_eligible(bus_id).await {
        Ok(()) => {
            write_frame(
                &mut stream,
                &Frame::OpenTunnelResp {
                    status: TunnelStatus::Ok,
                    message: String::new(),
                },
            )
            .await?;
            let peer_label = format!("reverse@{peer_addr}");
            state.registry.mark_in_use(bus_id, &peer_label).await;
            state.emit(DaemonEvent::DeviceState {
                bus_id: bus_id.to_string(),
                state: "in_use".into(),
                peer: peer_label,
            });
            let res = usbip::handler(&mut stream, state.registry.usbip_server()).await;
            let ignored = state.config.lock().await.config.ignored_devices.clone();
            state.registry.mark_released(bus_id, &ignored).await;
            let final_state = state
                .registry
                .list_devices()
                .await
                .into_iter()
                .find(|d| d.bus_id == bus_id)
                .map(|d| d.state.label().to_string())
                .unwrap_or_else(|| "detached".into());
            state.emit(DaemonEvent::DeviceState {
                bus_id: bus_id.to_string(),
                state: final_state,
                peer: "-".into(),
            });
            res.map_err(CoreError::Io)
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
