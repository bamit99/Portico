use crate::transport::AuthenticatedStream;
use crate::CliError;
use portico_proto::frame::{read_frame, write_frame, Frame, TunnelStatus};
use portico_proto::tls::configure_tunnel_tcp;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::copy_bidirectional_with_sizes;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinHandle;

pub async fn open_device_tunnel(
    stream: &mut AuthenticatedStream,
    bus_id: &str,
) -> Result<(), CliError> {
    write_frame(
        stream,
        &Frame::OpenTunnelReq {
            bus_id: bus_id.to_string(),
        },
    )
    .await?;
    match read_frame(stream).await? {
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        } => Ok(()),
        Frame::OpenTunnelResp { status, message } => Err(CliError::Remote(format!(
            "tunnel rejected ({status:?}): {message}"
        ))),
        Frame::Error { code, message } => {
            Err(CliError::Remote(format!("error {code}: {message}")))
        }
        other => Err(CliError::Remote(format!("unexpected response: {other:?}"))),
    }
}

pub async fn start_local_usbip_bridge(
    mut remote_stream: AuthenticatedStream,
    bus_id: &str,
) -> Result<(SocketAddr, JoinHandle<Result<(), CliError>>), CliError> {
    open_device_tunnel(&mut remote_stream, bus_id).await?;
    bridge_already_open_tunnel(remote_stream).await
}

pub async fn bridge_already_open_tunnel(
    mut remote_stream: AuthenticatedStream,
) -> Result<(SocketAddr, JoinHandle<Result<(), CliError>>), CliError> {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
    let local_addr = listener.local_addr()?;

    let handle = tokio::spawn(async move {
        let (mut local_tcp, _) = listener.accept().await?;
        configure_tunnel_tcp(&local_tcp);
        let _ = copy_bidirectional_with_sizes(&mut local_tcp, &mut remote_stream, 128 * 1024, 128 * 1024).await?;
        Ok(())
    });
    Ok((local_addr, handle))
}

/// Returns true when running as root (uid 0).
fn is_root() -> bool {
    // SAFETY: geteuid() takes no arguments and always succeeds.
    unsafe { libc::geteuid() == 0 }
}

/// Build the usbip attach invocation. When not root, wrap in `sudo` so the
/// kernel import step can write /sys/devices/platform/vhci_hcd.*/attach.
/// Only `usbip` runs elevated: the TLS handshake, TOFU pin and claim token
/// were already handled by the parent CLI as the regular user, and the
/// loopback bridge stays in this process — sudo never needs the config store.
pub fn usbip_attach_invocation(
    usbip_bin: &str,
    local_port: u16,
    bus_id: &str,
    is_root: bool,
) -> (String, Vec<String>) {
    let usbip_args: Vec<String> = [
        "--tcp-port".to_string(),
        local_port.to_string(),
        "attach".to_string(),
        "-r".to_string(),
        "127.0.0.1".to_string(),
        "-b".to_string(),
        bus_id.to_string(),
    ]
    .to_vec();
    if is_root {
        (usbip_bin.to_string(), usbip_args)
    } else {
        let mut args = vec!["--".to_string(), usbip_bin.to_string()];
        args.extend(usbip_args);
        ("sudo".to_string(), args)
    }
}

/// Detach a vhci port. Needs root for the sysfs write; wraps with sudo when
/// not root (same NOPASSWD rule as attach).
pub async fn run_usbip_detach(port: u8) -> Result<(), CliError> {
    let root = is_root();
    let mut args = vec!["detach".to_string(), "-p".to_string(), port.to_string()];
    let prog = if root {
        "usbip".to_string()
    } else {
        eprintln!("usbip detach needs root; requesting sudo");
        args = [vec!["--".to_string(), "usbip".to_string()], args].concat();
        "sudo".to_string()
    };
    let status = Command::new(&prog)
        .args(&args)
        .status()
        .await
        .map_err(|e| CliError::Remote(format!("failed to execute '{prog}': {e}")))?;
    if !status.success() {
        return Err(CliError::Remote(format!("usbip detach failed (exit {:?})", status.code())));
    }
    Ok(())
}

pub async fn run_usbip_attach_command(
    usbip_bin: &str,
    local_port: u16,
    bus_id: &str,
) -> Result<(), CliError> {
    let root = is_root();
    if !root {
        eprintln!(
            "usbip attach needs root to import the vhci device; requesting sudo (password prompt may appear)"
        );
    }
    let (prog, args) = usbip_attach_invocation(usbip_bin, local_port, bus_id, root);
    let mut cmd = Command::new(&prog);
    cmd.args(&args);
    if !root {
        // Let sudo prompt on the real terminal: inherit stdio instead of capturing it.
        cmd.stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        let status = cmd
            .status()
            .await
            .map_err(|e| CliError::Remote(format!("failed to execute '{prog}': {e}")))?;
        if !status.success() {
            return Err(CliError::Remote(format!(
                "usbip attach failed via sudo (exit {:?}) — sudo authentication or the vhci import was refused",
                status.code(),
            )));
        }
        return Ok(());
    }
    let output = cmd
        .output()
        .await
        .map_err(|e| CliError::Remote(format!("failed to execute '{prog}': {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(CliError::Remote(format!(
            "usbip attach failed (exit {:?}): {detail}",
            output.status.code(),
        )));
    }
    Ok(())
}
