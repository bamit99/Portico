use crate::daemon::DaemonHandle;
use crate::CoreError;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub const MAX_CTRL_LINE_LEN: usize = 65_536;

pub async fn spawn_control_server(
    socket_path: &Path,
    handle: DaemonHandle,
    shutdown: Arc<Notify>,
) -> Result<JoinHandle<()>, CoreError> {
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(socket_path);
    }
    let listener = UnixListener::bind(socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
    }

    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.notified() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        break;
                    };
                    let h = handle.clone();
                    tokio::spawn(async move {
                        let _ = handle_control_client(stream, h).await;
                    });
                }
            }
        }
    });
    Ok(task)
}

async fn read_bounded_line<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    let mut buf = Vec::with_capacity(256);
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if buf.is_empty() {
                return Ok(None);
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            if buf.len() + pos + 1 > MAX_CTRL_LINE_LEN {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "control line exceeds max length",
                ));
            }
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&buf).into_owned()));
        }
        if buf.len() + available.len() > MAX_CTRL_LINE_LEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "control line exceeds max length",
            ));
        }
        buf.extend_from_slice(available);
        let len = available.len();
        reader.consume(len);
    }
}

async fn handle_control_client(stream: UnixStream, handle: DaemonHandle) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);

    while let Some(line) = read_bounded_line(&mut reader).await? {
        if line.trim().eq_ignore_ascii_case("SUBSCRIBE") {
            let mut rx = handle.state.events_tx.subscribe();
            write_half.write_all(b"OK\n").await?;
            write_half.flush().await?;
            loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let msg = format!("{}\n", ev.to_control_line());
                        if write_half.write_all(msg.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = write_half.flush().await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            return Ok(());
        }

        let is_shutdown = line.trim().eq_ignore_ascii_case("SHUTDOWN");
        let resp = handle.exec_command(&line).await;
        write_half.write_all(resp.as_bytes()).await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
        if is_shutdown {
            break;
        }
    }
    Ok(())
}
