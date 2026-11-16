use portico_proto::relay_proto::RelayFrame;
use portico_proto::tls::configure_tunnel_tcp;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::copy_bidirectional_with_sizes;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

struct SlotEntry {
    secret_hash: [u8; 32],
    notify_tx: mpsc::Sender<[u8; 16]>,
    generation: u64,
}

#[derive(Default)]
struct RelayState {
    next_gen: u64,
    slots: HashMap<[u8; 16], SlotEntry>,
    pending: HashMap<([u8; 16], [u8; 16]), oneshot::Sender<TcpStream>>,
}

pub struct RelayServer;

impl RelayServer {
    pub async fn run(listener: TcpListener) -> std::io::Result<()> {
        let state = Arc::new(Mutex::new(RelayState::default()));
        loop {
            let (stream, peer) = listener.accept().await?;
            configure_tunnel_tcp(&stream);
            let st = state.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_conn(stream, st).await {
                    debug!("relay conn {peer} ended: {e}");
                }
            });
        }
    }
}

async fn handle_conn(
    mut stream: TcpStream,
    state: Arc<Mutex<RelayState>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let first = RelayFrame::read_from(&mut stream).await?;
    match first {
        RelayFrame::ControlRegister {
            node_id,
            slot_secret_hash,
        } => {
            let (tx, mut rx) = mpsc::channel::<[u8; 16]>(64);
            let my_gen = {
                let mut guard = state.lock().await;
                if let Some(existing) = guard.slots.get(&node_id) {
                    if !bool::from(existing.secret_hash.ct_eq(&slot_secret_hash)) {
                        drop(guard);
                        RelayFrame::RegisterRejected.write_to(&mut stream).await?;
                        return Ok(());
                    }
                }
                guard.next_gen += 1;
                let g = guard.next_gen;
                guard.slots.insert(
                    node_id,
                    SlotEntry {
                        secret_hash: slot_secret_hash,
                        notify_tx: tx,
                        generation: g,
                    },
                );
                g
            };
            RelayFrame::Registered.write_to(&mut stream).await?;
            info!("registered relay slot {}", hex::encode(node_id));

            while let Some(conn_nonce) = rx.recv().await {
                let msg = RelayFrame::IncomingClient { conn_nonce };
                if msg.write_to(&mut stream).await.is_err() {
                    break;
                }
            }

            let mut guard = state.lock().await;
            if guard.slots.get(&node_id).map(|s| s.generation) == Some(my_gen) {
                guard.slots.remove(&node_id);
            }
        }
        RelayFrame::ClientConnect {
            node_id,
            conn_nonce,
        } => {
            let (notify_tx, rx) = {
                let mut guard = state.lock().await;
                let Some(slot) = guard.slots.get(&node_id) else {
                    return Ok(());
                };
                let tx = slot.notify_tx.clone();
                let (ptx, prx) = oneshot::channel::<TcpStream>();
                guard.pending.insert((node_id, conn_nonce), ptx);
                (tx, prx)
            };

            if notify_tx.send(conn_nonce).await.is_err() {
                state.lock().await.pending.remove(&(node_id, conn_nonce));
                return Ok(());
            }

            let srv_stream = match tokio::time::timeout(Duration::from_secs(15), rx).await {
                Ok(Ok(s)) => s,
                _ => {
                    state.lock().await.pending.remove(&(node_id, conn_nonce));
                    return Ok(());
                }
            };

            let mut cli_stream = stream;
            let mut srv_stream = srv_stream;
            let _ = copy_bidirectional_with_sizes(
                &mut cli_stream,
                &mut srv_stream,
                128 * 1024,
                128 * 1024,
            )
            .await;
        }
        RelayFrame::ServerAccept {
            node_id,
            slot_secret_hash,
            conn_nonce,
        } => {
            let pending_tx = {
                let mut guard = state.lock().await;
                let authorized = guard
                    .slots
                    .get(&node_id)
                    .map(|s| bool::from(s.secret_hash.ct_eq(&slot_secret_hash)))
                    .unwrap_or(false);
                if !authorized {
                    drop(guard);
                    warn!("rejected unauthorized ServerAccept for {}", hex::encode(node_id));
                    RelayFrame::AcceptRejected.write_to(&mut stream).await?;
                    return Ok(());
                }
                guard.pending.remove(&(node_id, conn_nonce))
            };

            if let Some(tx) = pending_tx {
                let _ = tx.send(stream);
            } else {
                RelayFrame::AcceptRejected.write_to(&mut stream).await?;
            }
        }
        other => {
            debug!("unexpected initial relay frame: {other:?}");
        }
    }
    Ok(())
}
