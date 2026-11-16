use crate::config::{ReverseClientEntry, UsbSpeedSetting, VidPid};
use crate::control::spawn_control_server;
use crate::logging::{pct_encode, DaemonEvent, LogLevel};
use crate::network::NetworkSupervisor;
use crate::state::DaemonState;
use crate::CoreError;
use portico_proto::identity::{write_atomic_secure, NodeIdentity};
use portico_proto::token::generate_claim_token;
use portico_proto::DEFAULT_DISCOVERY_PORT;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct DaemonHandle {
    pub state: Arc<DaemonState>,
    pub network: Arc<NetworkSupervisor>,
    pub shutdown: Arc<Notify>,
    pub tcp_bind_override: Option<SocketAddr>,
    pub udp_bind_override: Option<SocketAddr>,
}

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Ok(true),
        "false" | "0" | "off" | "no" => Ok(false),
        other => Err(format!("invalid bool '{other}'")),
    }
}

impl DaemonHandle {
    pub async fn exec_command(&self, line: &str) -> String {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return "ERR empty_command empty command".into();
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
        let rest = parts.next().unwrap_or("").trim();

        match cmd.as_str() {
            "REGISTER_CLIENT" => {
                let name = if rest.is_empty() { "client" } else { rest };
                let id = format!(
                    "cli-{}",
                    rand::random::<u32>()
                );
                self.state
                    .registered_clients
                    .lock()
                    .await
                    .insert(id.clone(), name.to_string());
                self.state.log_and_emit(
                    LogLevel::Info,
                    "control",
                    &format!("registered client {name} ({id})"),
                );
                format!("OK {id}")
            }
            "UNREGISTER_CLIENT" => {
                self.state.registered_clients.lock().await.remove(rest);
                format!("OK")
            }
            "GET_STATE" => {
                let running = self.state.server_running.load(Ordering::SeqCst);
                let active_peers = self.state.active_peers.load(Ordering::Relaxed);
                let fp = self.state.identity.read().await.fingerprint_hex();
                let relay = self.state.relay_status.read().await.clone();
                let bound_port = self.network.bound_tcp_addr().await.map(|a| a.port());
                let cfg_guard = self.state.config.lock().await;
                let cfg = &cfg_guard.config;
                let port = bound_port.unwrap_or(cfg.port);
                let relay_addr = if relay.address().is_empty() {
                    "-".to_string()
                } else {
                    pct_encode(relay.address())
                };
                format!(
                    "OK running={running} server_active={running} tls={} plaintext_lan={} port={port} speed={} net_lock={} wake_lock={} boot={} relay_status={} relay_addr={relay_addr} fp={fp} active_peers={active_peers} tier={}",
                    cfg.tls_required,
                    cfg.plaintext_lan,
                    cfg.usb_speed.as_str(),
                    cfg.keep_network_awake,
                    cfg.keep_device_awake,
                    cfg.start_on_boot,
                    relay.label(),
                    cfg.tier.as_str()
                )
            }
            "SET_STATE" => {
                let mut kv = rest.splitn(2, char::is_whitespace);
                let key = kv.next().unwrap_or("");
                let val = kv.next().unwrap_or("").trim();
                let mut cfg_guard = self.state.config.lock().await;
                let res = match key {
                    "port" => match val.parse::<u16>() {
                        Ok(p) if p > 0 => {
                            cfg_guard.config.port = p;
                            Ok(())
                        }
                        _ => Err("invalid port".into()),
                    },
                    "plaintext_lan" => parse_bool(val).map(|b| cfg_guard.config.plaintext_lan = b),
                    "tls_required" => parse_bool(val).map(|b| cfg_guard.config.tls_required = b),
                    "start_on_boot" => parse_bool(val).map(|b| cfg_guard.config.start_on_boot = b),
                    _ => Err(format!("unknown state key '{key}'")),
                };
                match res {
                    Ok(()) => {
                        if let Err(e) = cfg_guard.save_atomic() {
                            return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                        }
                        "OK".into()
                    }
                    Err(msg) => format!("ERR invalid_arg {}", pct_encode(&msg)),
                }
            }
            "START_SERVER" => {
                let port = self.state.config.lock().await.config.port;
                let tcp_bind = self
                    .tcp_bind_override
                    .unwrap_or_else(|| SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
                let udp_bind = Some(self.udp_bind_override.unwrap_or_else(|| {
                    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_DISCOVERY_PORT)
                }));
                match self
                    .network
                    .start_server(self.state.clone(), tcp_bind, udp_bind)
                    .await
                {
                    Ok(_) => "OK".into(),
                    Err(e) => format!("ERR start_failed {}", pct_encode(&e.to_string())),
                }
            }
            "STOP_SERVER" => {
                self.network.stop_server(&self.state).await;
                "OK".into()
            }
            "GET_IGNORED" => {
                let list = self.state.config.lock().await.config.ignored_devices.clone();
                let mut out = vec![format!("OK {}", list.len())];
                for vp in list {
                    out.push(format!("ITEM {vp}"));
                }
                out.push("END".into());
                out.join("\n")
            }
            "SET_IGNORED" => {
                let parsed = if rest.is_empty() || rest == "-" {
                    Ok(Vec::new())
                } else {
                    rest.split(',')
                        .map(VidPid::parse)
                        .collect::<Result<Vec<_>, _>>()
                };
                let new_list = match parsed {
                    Ok(v) => v,
                    Err(e) => return format!("ERR invalid_vid_pid {}", pct_encode(&e.to_string())),
                };
                {
                    let mut cfg = self.state.config.lock().await;
                    cfg.config.ignored_devices = new_list.clone();
                    if let Err(e) = cfg.save_atomic() {
                        return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                    }
                }
                if let Err(e) = self.state.registry.apply_ignored_list(&new_list).await {
                    return format!("ERR apply_failed {}", pct_encode(&e.to_string()));
                }
                self.state.log_and_emit(
                    LogLevel::Info,
                    "policy",
                    &format!("updated ignored list ({} entries)", new_list.len()),
                );
                "OK".into()
            }
            "GET_REVERSE_CLIENTS" => {
                let list = self.state.config.lock().await.config.reverse_clients.clone();
                let mut out = vec![format!("OK {}", list.len())];
                for entry in list {
                    out.push(format!(
                        "ITEM id={} addr={} fp={} enabled={}",
                        pct_encode(&entry.id),
                        pct_encode(&entry.addr),
                        entry.cert_sha256,
                        entry.enabled
                    ));
                }
                out.push("END".into());
                out.join("\n")
            }
            "SET_REVERSE_CLIENTS" => {
                let parsed = if rest.is_empty() || rest == "-" {
                    Ok(Vec::new())
                } else {
                    rest.split(',')
                        .enumerate()
                        .map(|(idx, raw)| {
                            let segs: Vec<&str> = raw.trim().split('@').collect();
                            if segs.len() < 2 || segs.len() > 3 {
                                return Err(CoreError::InvalidInput(
                                    "expected addr@sha256[@enabled]".into(),
                                ));
                            }
                            let enabled = if segs.len() == 3 {
                                parse_bool(segs[2]).map_err(CoreError::InvalidInput)?
                            } else {
                                true
                            };
                            ReverseClientEntry {
                                id: format!("rev-{}", idx + 1),
                                addr: segs[0].to_string(),
                                cert_sha256: segs[1].to_string(),
                                enabled,
                            }
                            .validate()
                        })
                        .collect::<Result<Vec<_>, _>>()
                };
                let new_list = match parsed {
                    Ok(v) => v,
                    Err(e) => {
                        return format!("ERR invalid_reverse_client {}", pct_encode(&e.to_string()))
                    }
                };
                {
                    let mut cfg = self.state.config.lock().await;
                    cfg.config.reverse_clients = new_list.clone();
                    if let Err(e) = cfg.save_atomic() {
                        return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                    }
                }
                self.network.sync_reverse_clients(self.state.clone()).await;
                self.state.log_and_emit(
                    LogLevel::Info,
                    "policy",
                    &format!("updated reverse clients ({} entries)", new_list.len()),
                );
                "OK".into()
            }
            "SET_RELAY" => {
                let mut sp = rest.splitn(2, char::is_whitespace);
                let enabled_str = sp.next().unwrap_or("");
                let enabled = match parse_bool(enabled_str) {
                    Ok(b) => b,
                    Err(e) => return format!("ERR invalid_arg {}", pct_encode(&e)),
                };
                let ep_arg = sp.next().map(str::trim).filter(|s| !s.is_empty());
                {
                    let mut cfg = self.state.config.lock().await;
                    if let Some(ep) = ep_arg {
                        cfg.config.relay.endpoint = ep.to_string();
                    }
                    if enabled && cfg.config.relay.endpoint.trim().is_empty() {
                        return "ERR invalid_relay relay%20endpoint%20is%20required".into();
                    }
                    cfg.config.relay.enabled = enabled;
                    if let Err(e) = cfg.save_atomic() {
                        return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                    }
                }
                self.network.sync_relay(self.state.clone()).await;
                "OK".into()
            }
            "GET_RELAY" => {
                let cfg = self.state.config.lock().await.config.relay.clone();
                let status = self.state.relay_status.read().await.clone();
                let addr = if status.address().is_empty() {
                    "-".into()
                } else {
                    pct_encode(status.address())
                };
                let ep = if cfg.endpoint.is_empty() {
                    "-".into()
                } else {
                    pct_encode(&cfg.endpoint)
                };
                format!(
                    "OK enabled={} status={} address={addr} endpoint={ep}",
                    cfg.enabled,
                    status.label()
                )
            }
            "SET_NET_LOCK" => match parse_bool(rest) {
                Ok(b) => {
                    let wake = {
                        let mut cfg = self.state.config.lock().await;
                        cfg.config.keep_network_awake = b;
                        if let Err(e) = cfg.save_atomic() {
                            return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                        }
                        cfg.config.keep_device_awake
                    };
                    self.state.emit(DaemonEvent::LockChanged {
                        net_lock: b,
                        wake_lock: wake,
                    });
                    "OK".into()
                }
                Err(e) => format!("ERR invalid_arg {}", pct_encode(&e)),
            },
            "SET_WAKE_LOCK" => match parse_bool(rest) {
                Ok(b) => {
                    let net = {
                        let mut cfg = self.state.config.lock().await;
                        cfg.config.keep_device_awake = b;
                        if let Err(e) = cfg.save_atomic() {
                            return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                        }
                        cfg.config.keep_network_awake
                    };
                    self.state.emit(DaemonEvent::LockChanged {
                        net_lock: net,
                        wake_lock: b,
                    });
                    "OK".into()
                }
                Err(e) => format!("ERR invalid_arg {}", pct_encode(&e)),
            },
            "SET_SPEED" => match UsbSpeedSetting::parse(rest) {
                Ok(sp) => {
                    let mut cfg = self.state.config.lock().await;
                    cfg.config.usb_speed = sp;
                    if let Err(e) = cfg.save_atomic() {
                        return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                    }
                    "OK".into()
                }
                Err(e) => format!("ERR invalid_arg {}", pct_encode(&e.to_string())),
            },
            "LIST_DEVICES" => {
                let devs = self.state.registry.list_devices().await;
                let mut out = vec![format!("OK {}", devs.len())];
                for d in devs {
                    out.push(format!(
                        "ITEM bus_id={} vid={:04X} pid={:04X} product={} mfr={} serial={} state={} peer={}",
                        pct_encode(&d.bus_id),
                        d.vid,
                        d.pid,
                        pct_encode(&d.product),
                        pct_encode(&d.manufacturer),
                        pct_encode(&d.serial),
                        d.state.label(),
                        pct_encode(d.state.peer())
                    ));
                }
                out.push("END".into());
                out.join("\n")
            }
            "DETACH_DEVICE" => match self.state.registry.detach_device(rest).await {
                Ok(_) => {
                    self.state.emit(DaemonEvent::DeviceRemoved {
                        bus_id: rest.to_string(),
                    });
                    "OK".into()
                }
                Err(e) => format!("ERR not_found {}", pct_encode(&e.to_string())),
            },
            "GET_TOKEN" => {
                let tok = self.state.config.lock().await.config.claim_token.clone();
                format!("OK {tok}")
            }
            "REGEN_TOKEN" => {
                let new_tok = generate_claim_token();
                let mut cfg = self.state.config.lock().await;
                cfg.config.claim_token = new_tok.clone();
                if let Err(e) = cfg.save_atomic() {
                    return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                }
                drop(cfg);
                self.state
                    .log_and_emit(LogLevel::Info, "security", "claim token regenerated");
                format!("OK {new_tok}")
            }
            "ROTATE_IDENTITY" => match NodeIdentity::generate("portico-node") {
                Ok(new_id) => {
                    let cert_path = self.state.data_dir.join("identity_cert.der");
                    let key_path = self.state.data_dir.join("identity_key.der");
                    if let Err(e) = write_atomic_secure(&cert_path, &new_id.cert_der)
                        .and_then(|_| write_atomic_secure(&key_path, &new_id.key_der))
                    {
                        return format!("ERR save_failed {}", pct_encode(&e.to_string()));
                    }
                    let fp = new_id.fingerprint_hex();
                    *self.state.identity.write().await = new_id;
                    self.network.sync_relay(self.state.clone()).await;
                    self.state.log_and_emit(
                        LogLevel::Info,
                        "security",
                        "server identity certificate rotated",
                    );
                    format!("OK {fp}")
                }
                Err(e) => format!("ERR keygen_failed {}", pct_encode(&e.to_string())),
            },
            "GET_LOGS" => {
                let after = rest.parse::<u64>().unwrap_or(0);
                let entries = self.state.logs.since(after);
                let mut out = vec![format!("OK {}", entries.len())];
                for e in entries {
                    out.push(format!(
                        "ITEM seq={} ts={} level={} target={} msg={}",
                        e.seq,
                        e.timestamp_ms,
                        e.level.as_str(),
                        pct_encode(&e.target),
                        pct_encode(&e.message)
                    ));
                }
                out.push("END".into());
                out.join("\n")
            }
            "CLEAR_LOGS" => {
                self.state.logs.clear();
                "OK".into()
            }
            "SHUTDOWN" => {
                self.network.stop_server(&self.state).await;
                self.shutdown.notify_waiters();
                "OK".into()
            }
            other => format!("ERR unknown_cmd {}", pct_encode(other)),
        }
    }
}

pub struct PorticoDaemon {
    handle: DaemonHandle,
    ctrl_task: Option<JoinHandle<()>>,
    ctrl_socket_path: PathBuf,
}

impl PorticoDaemon {
    pub async fn start(data_dir: PathBuf, ctrl_socket_path: PathBuf) -> Result<Self, CoreError> {
        Self::start_with_bind_overrides(data_dir, ctrl_socket_path, None, None).await
    }

    pub async fn start_with_bind_overrides(
        data_dir: PathBuf,
        ctrl_socket_path: PathBuf,
        tcp_bind_override: Option<SocketAddr>,
        udp_bind_override: Option<SocketAddr>,
    ) -> Result<Self, CoreError> {
        let state = DaemonState::new(data_dir)?;
        let network = Arc::new(NetworkSupervisor::new());
        let shutdown = Arc::new(Notify::new());
        let handle = DaemonHandle {
            state,
            network,
            shutdown: shutdown.clone(),
            tcp_bind_override,
            udp_bind_override,
        };
        let ctrl_task =
            spawn_control_server(&ctrl_socket_path, handle.clone(), shutdown).await?;
        Ok(Self {
            handle,
            ctrl_task: Some(ctrl_task),
            ctrl_socket_path,
        })
    }

    pub fn handle(&self) -> DaemonHandle {
        self.handle.clone()
    }

    pub async fn shutdown(mut self) {
        self.handle.shutdown.notify_waiters();
        self.handle.network.stop_server(&self.handle.state).await;
        if let Some(t) = self.ctrl_task.take() {
            t.abort();
        }
        let _ = std::fs::remove_file(&self.ctrl_socket_path);
    }
}
