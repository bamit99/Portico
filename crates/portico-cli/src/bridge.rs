use crate::transport::AuthenticatedStream;
use crate::CliError;
use portico_proto::frame::{read_frame, write_frame, Frame, TunnelStatus};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::io::copy_bidirectional;
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
        let _ = copy_bidirectional(&mut local_tcp, &mut remote_stream).await?;
        Ok(())
    });
    Ok((local_addr, handle))
}

pub async fn run_usbip_attach_command(
    usbip_bin: &str,
    local_port: u16,
    bus_id: &str,
) -> Result<(), CliError> {
    let output = Command::new(usbip_bin)
        .arg("--tcp-port")
        .arg(local_port.to_string())
        .arg("attach")
        .arg("-r")
        .arg("127.0.0.1")
        .arg("-b")
        .arg(bus_id)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::Remote(format!(
            "usbip attach failed (exit {:?}): {}",
            output.status.code(),
            stderr.trim()
        )));
    }
    Ok(())
}
