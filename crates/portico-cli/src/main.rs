use clap::{Parser, Subcommand};
use portico_cli::bridge::{run_usbip_attach_command, start_local_usbip_bridge};
use portico_cli::discover::discover_servers;
use portico_cli::reverse::ReverseListener;
use portico_cli::serve::{start_host_daemon, ServeOptions};
use portico_cli::transport::{cmd_list, cmd_pair, cmd_trust, connect_and_auth, resolve_token, ConnectOptions};
use portico_proto::identity::format_short_fingerprint;
use portico_proto::{DEFAULT_DISCOVERY_PORT, DEFAULT_PORT};
use portico_relay::RelayServer;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::Command;

#[derive(Parser)]
#[command(name = "portico", version, about = "Portico USB-over-network client and tools")]
struct Cli {
    #[arg(long, global = true, help = "Config directory (default: ~/.config/portico)")]
    config_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Discover Portico servers on the local network via UDP
    Discover {
        #[arg(long, default_value_t = 1500)]
        timeout_ms: u64,
        #[arg(long, default_value = "255.255.255.255:37421")]
        target: SocketAddr,
    },
    /// Pin a server SHA-256 certificate fingerprint (TOFU store)
    Trust {
        target: String,
        fingerprint: String,
    },
    /// Pair with a Portico server: interactive TOFU confirm, pin the certificate and store the claim token
    Pair {
        target: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(short = 'y', long)]
        assume_yes: bool,
    },
    /// List shared USB devices on a Portico server
    List {
        target: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        trust: Option<String>,
        #[arg(long)]
        insecure_lan_plaintext: bool,
    },
    /// Attach a remote shared USB device to local Linux vhci-hcd via loopback bridge
    Attach {
        target: String,
        bus_id: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        trust: Option<String>,
        #[arg(long)]
        insecure_lan_plaintext: bool,
        #[arg(long, help = "Start local USB/IP bridge without invoking `usbip attach`")]
        proxy_only: bool,
    },
    /// Detach a virtual USB/IP port via `usbip detach`
    Detach {
        #[arg(long)]
        port: u8,
    },
    /// Listen for outbound mutual-TLS connections from reverse-client servers
    Listen {
        #[arg(long, default_value = "0.0.0.0:37425")]
        bind: SocketAddr,
        #[arg(long)]
        token: Option<String>,
        #[arg(long = "allow-server", required = true)]
        allow_servers: Vec<String>,
        #[arg(long = "attach-bus")]
        attach_buses: Vec<String>,
        #[arg(long)]
        proxy_only: bool,
    },
    /// Print this desktop client's mTLS certificate SHA-256 fingerprint
    Identity,
    /// Run a local Portico USB server daemon on this host
    Serve {
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        #[arg(long)]
        scan_host: bool,
        #[arg(long)]
        simulate_cdc: bool,
        #[arg(long)]
        show_token: bool,
    },
    /// Run a ciphertext-only Portico NAT relay server
    Relay {
        #[arg(long, default_value = "0.0.0.0:37422")]
        listen: SocketAddr,
    },
}

fn default_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("portico");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("portico");
    }
    PathBuf::from(".portico")
}

async fn futures_wait_any(
    tasks: Vec<tokio::task::JoinHandle<Result<(), portico_cli::CliError>>>,
) -> Result<(), portico_cli::CliError> {
    let mut set = tokio::task::JoinSet::new();
    for t in tasks {
        set.spawn(async move {
            match t.await {
                Ok(r) => r,
                Err(e) => Err(portico_cli::CliError::Remote(format!("bridge task panicked: {e}"))),
            }
        });
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(())) => continue,
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(portico_cli::CliError::Remote(format!("join error: {e}"))),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let cfg_dir = cli.config_dir.unwrap_or_else(default_config_dir);

    if let Err(err) = run(cli.command, &cfg_dir).await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn run(cmd: Commands, cfg_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Commands::Discover { timeout_ms, target } => {
            let bind: SocketAddr = "0.0.0.0:0".parse()?;
            let found = discover_servers(bind, target, Duration::from_millis(timeout_ms)).await?;
            if found.is_empty() {
                println!("No Portico servers found.");
            } else {
                for s in found {
                    println!(
                        "{}\t{}\ttls={}\tplaintext_lan={}\tfp_prefix={}",
                        s.addr, s.node_name, s.tls_required, s.plaintext_lan, s.cert_fp_prefix
                    );
                }
            }
        }
        Commands::Trust {
            target,
            fingerprint,
        } => {
            cmd_trust(cfg_dir, &target, &fingerprint)?;
            println!("Pinned {target}");
        }
        Commands::Pair {
            target,
            token,
            assume_yes,
        } => {
            cmd_pair(cfg_dir, &target, token.as_deref(), assume_yes, |_| {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line).unwrap_or(0);
                line.trim().eq_ignore_ascii_case("y")
            })
            .await?;
        }
        Commands::List {
            target,
            token,
            trust,
            insecure_lan_plaintext,
        } => {
            let token = resolve_token(cfg_dir, &target, token.as_deref())?;
            let devs = cmd_list(ConnectOptions {
                config_dir: cfg_dir,
                target: &target,
                token: &token,
                trust_fp: trust.as_deref(),
                insecure_lan_plaintext,
                client_name: "portico-cli",
            })
            .await?;
            if devs.is_empty() {
                println!("No shared USB devices.");
            } else {
                for d in devs {
                    println!(
                        "{}\t{:04x}:{:04x}\t{}\t{}\t{}",
                        d.bus_id,
                        d.vid,
                        d.pid,
                        d.product,
                        d.manufacturer,
                        if d.in_use { "in_use" } else { "available" }
                    );
                }
            }
        }
        Commands::Attach {
            target,
            bus_id,
            token,
            trust,
            insecure_lan_plaintext,
            proxy_only,
        } => {
            let token = resolve_token(cfg_dir, &target, token.as_deref())?;
            let stream = connect_and_auth(ConnectOptions {
                config_dir: cfg_dir,
                target: &target,
                token: &token,
                trust_fp: trust.as_deref(),
                insecure_lan_plaintext,
                client_name: "portico-cli",
            })
            .await?;
            let (local_addr, bridge_task) = start_local_usbip_bridge(stream, &bus_id).await?;
            println!("Local USB/IP bridge for {bus_id} listening on {local_addr}");
            if proxy_only {
                println!(
                    "Proxy-only mode: run `usbip --tcp-port {} attach -r 127.0.0.1 -b {bus_id}`",
                    local_addr.port()
                );
            } else {
                run_usbip_attach_command("usbip", local_addr.port(), &bus_id).await?;
                println!("Attached {bus_id} via vhci-hcd. Press Ctrl-C to disconnect.");
            }
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                res = bridge_task => {
                    if let Ok(Err(e)) = res {
                        return Err(e.into());
                    }
                }
            }
        }
        Commands::Detach { port } => {
            let out = Command::new("usbip")
                .arg("detach")
                .arg("-p")
                .arg(port.to_string())
                .output()
                .await?;
            if !out.status.success() {
                return Err(format!(
                    "usbip detach failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )
                .into());
            }
            println!("Detached vhci port {port}");
        }
        Commands::Listen {
            bind,
            token,
            allow_servers,
            attach_buses,
            proxy_only,
        } => {
            let token = token.ok_or_else(|| "no claim token — pass --token or store it via pair".to_string())?;
            let id = ReverseListener::load_or_create_identity(cfg_dir)?;
            let fp = id.fingerprint_hex();
            println!("Desktop reverse-listener fingerprint: {fp}");
            println!("Short: {}", format_short_fingerprint(&fp));
            let listener = ReverseListener::bind(bind, id, allow_servers, token).await?;
            println!("Listening for mTLS reverse servers on {}", listener.local_addr());

            let ctrl = listener.wait_for_server_ready().await?;
            println!("Connected reverse server {}", ctrl.server_fp);
            for d in &ctrl.devices {
                println!("  {}\t{:04x}:{:04x}\t{}", d.bus_id, d.vid, d.pid, d.product);
            }

            if !attach_buses.is_empty() {
                let mut tasks = Vec::new();
                for bus_id in attach_buses {
                    let (local_addr, bridge_task) = listener
                        .open_reverse_tunnel(&ctrl.server_fp, &bus_id)
                        .await?;
                    println!("Reverse USB/IP bridge for {bus_id} on {local_addr}");
                    if proxy_only {
                        println!(
                            "Proxy-only mode: run `usbip --tcp-port {} attach -r 127.0.0.1 -b {bus_id}`",
                            local_addr.port()
                        );
                    } else {
                        run_usbip_attach_command("usbip", local_addr.port(), &bus_id).await?;
                    }
                    tasks.push(bridge_task);
                }
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    res = futures_wait_any(tasks) => {
                        if let Err(e) = res {
                            return Err(e.into());
                        }
                    }
                }
            } else {
                tokio::signal::ctrl_c().await?;
            }
        }
        Commands::Identity => {
            let id = ReverseListener::load_or_create_identity(cfg_dir)?;
            let fp = id.fingerprint_hex();
            println!("Fingerprint (SHA-256): {fp}");
            println!("Short: {}", format_short_fingerprint(&fp));
        }
        Commands::Serve {
            data_dir,
            port,
            scan_host,
            simulate_cdc,
            show_token,
        } => {
            let ddir = data_dir.unwrap_or_else(|| cfg_dir.join("server"));
            let ctrl_sock = ddir.join("portico-ctrl.sock");
            let tcp_bind: SocketAddr = format!("0.0.0.0:{port}").parse()?;
            let daemon = start_host_daemon(ServeOptions {
                data_dir: ddir,
                ctrl_socket: ctrl_sock,
                tcp_bind: Some(tcp_bind),
                udp_bind: Some(format!("0.0.0.0:{DEFAULT_DISCOVERY_PORT}").parse()?),
                scan_host,
                simulate_cdc,
            })
            .await?;
            let h = daemon.handle();
            let fp = h.state.identity.read().await.fingerprint_hex();
            println!("Portico server listening on {}", h.network.bound_tcp_addr().await.unwrap());
            println!("Fingerprint: {fp}");
            println!("Short: {}", format_short_fingerprint(&fp));
            if show_token {
                let tok = h.exec_command("GET_TOKEN").await;
                println!("Claim token: {}", tok.strip_prefix("OK ").unwrap_or(&tok));
            }
            tokio::signal::ctrl_c().await?;
            daemon.shutdown().await;
        }
        Commands::Relay { listen } => {
            let listener = TcpListener::bind(listen).await?;
            println!("Portico ciphertext relay listening on {}", listener.local_addr()?);
            RelayServer::run(listener).await?;
        }
    }
    Ok(())
}
