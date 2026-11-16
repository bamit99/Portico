use crate::ProtoError;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const RELAY_MAGIC: &[u8; 4] = b"PRLY";
pub const RELAY_VERSION_V0: u8 = 0x00;

const OP_CONTROL_REGISTER: u8 = 0x01;
const OP_CLIENT_CONNECT: u8 = 0x02;
const OP_INCOMING_CLIENT: u8 = 0x03;
const OP_SERVER_ACCEPT: u8 = 0x04;
const OP_REGISTERED: u8 = 0x10;
const OP_REGISTER_REJECTED: u8 = 0x11;
const OP_ACCEPT_REJECTED: u8 = 0x12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayFrame {
    ControlRegister {
        node_id: [u8; 16],
        slot_secret_hash: [u8; 32],
    },
    Registered,
    RegisterRejected,
    ClientConnect {
        node_id: [u8; 16],
        conn_nonce: [u8; 16],
    },
    IncomingClient {
        conn_nonce: [u8; 16],
    },
    ServerAccept {
        node_id: [u8; 16],
        slot_secret_hash: [u8; 32],
        conn_nonce: [u8; 16],
    },
    AcceptRejected,
}

impl RelayFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(70);
        out.extend_from_slice(RELAY_MAGIC);
        out.push(RELAY_VERSION_V0);
        match self {
            RelayFrame::ControlRegister {
                node_id,
                slot_secret_hash,
            } => {
                out.push(OP_CONTROL_REGISTER);
                out.extend_from_slice(node_id);
                out.extend_from_slice(slot_secret_hash);
            }
            RelayFrame::Registered => out.push(OP_REGISTERED),
            RelayFrame::RegisterRejected => out.push(OP_REGISTER_REJECTED),
            RelayFrame::ClientConnect {
                node_id,
                conn_nonce,
            } => {
                out.push(OP_CLIENT_CONNECT);
                out.extend_from_slice(node_id);
                out.extend_from_slice(conn_nonce);
            }
            RelayFrame::IncomingClient { conn_nonce } => {
                out.push(OP_INCOMING_CLIENT);
                out.extend_from_slice(conn_nonce);
            }
            RelayFrame::ServerAccept {
                node_id,
                slot_secret_hash,
                conn_nonce,
            } => {
                out.push(OP_SERVER_ACCEPT);
                out.extend_from_slice(node_id);
                out.extend_from_slice(slot_secret_hash);
                out.extend_from_slice(conn_nonce);
            }
            RelayFrame::AcceptRejected => out.push(OP_ACCEPT_REJECTED),
        }
        out
    }

    pub async fn read_from<R: AsyncRead + Unpin>(r: &mut R) -> Result<Self, ProtoError> {
        let mut hdr = [0u8; 6];
        r.read_exact(&mut hdr).await?;
        if &hdr[0..4] != RELAY_MAGIC {
            return Err(ProtoError::InvalidMagic);
        }
        if hdr[4] != RELAY_VERSION_V0 {
            return Err(ProtoError::UnsupportedVersion(hdr[4]));
        }
        match hdr[5] {
            OP_CONTROL_REGISTER => {
                let mut node_id = [0u8; 16];
                let mut slot_secret_hash = [0u8; 32];
                r.read_exact(&mut node_id).await?;
                r.read_exact(&mut slot_secret_hash).await?;
                Ok(RelayFrame::ControlRegister {
                    node_id,
                    slot_secret_hash,
                })
            }
            OP_REGISTERED => Ok(RelayFrame::Registered),
            OP_REGISTER_REJECTED => Ok(RelayFrame::RegisterRejected),
            OP_CLIENT_CONNECT => {
                let mut node_id = [0u8; 16];
                let mut conn_nonce = [0u8; 16];
                r.read_exact(&mut node_id).await?;
                r.read_exact(&mut conn_nonce).await?;
                Ok(RelayFrame::ClientConnect {
                    node_id,
                    conn_nonce,
                })
            }
            OP_INCOMING_CLIENT => {
                let mut conn_nonce = [0u8; 16];
                r.read_exact(&mut conn_nonce).await?;
                Ok(RelayFrame::IncomingClient { conn_nonce })
            }
            OP_SERVER_ACCEPT => {
                let mut node_id = [0u8; 16];
                let mut slot_secret_hash = [0u8; 32];
                let mut conn_nonce = [0u8; 16];
                r.read_exact(&mut node_id).await?;
                r.read_exact(&mut slot_secret_hash).await?;
                r.read_exact(&mut conn_nonce).await?;
                Ok(RelayFrame::ServerAccept {
                    node_id,
                    slot_secret_hash,
                    conn_nonce,
                })
            }
            OP_ACCEPT_REJECTED => Ok(RelayFrame::AcceptRejected),
            other => Err(ProtoError::UnknownMessageType(other)),
        }
    }

    pub async fn write_to<W: AsyncWrite + Unpin>(&self, w: &mut W) -> Result<(), ProtoError> {
        let bytes = self.encode();
        w.write_all(&bytes).await?;
        w.flush().await?;
        Ok(())
    }
}

pub fn parse_relay_uri(uri: &str) -> Result<(String, [u8; 16]), ProtoError> {
    let rest = uri
        .strip_prefix("prly://")
        .ok_or_else(|| ProtoError::InvalidRelayUri(uri.to_string()))?;
    let (host_port, id_hex) = rest
        .split_once('/')
        .ok_or_else(|| ProtoError::InvalidRelayUri(uri.to_string()))?;
    let Some((host, port_str)) = host_port.rsplit_once(':') else {
        return Err(ProtoError::InvalidRelayUri(uri.to_string()));
    };
    if host.is_empty()
        || port_str.parse::<u16>().map(|p| p == 0).unwrap_or(true)
        || id_hex.len() != 32
    {
        return Err(ProtoError::InvalidRelayUri(uri.to_string()));
    }
    let decoded = hex::decode(id_hex)
        .map_err(|_| ProtoError::InvalidRelayUri(uri.to_string()))?;
    let node_id: [u8; 16] = decoded
        .try_into()
        .map_err(|_| ProtoError::InvalidRelayUri(uri.to_string()))?;
    Ok((host_port.to_string(), node_id))
}

pub fn format_relay_uri(endpoint: &str, node_id: &[u8; 16]) -> String {
    format!("prly://{endpoint}/{}", hex::encode(node_id))
}

pub fn hash_slot_secret(secret: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"portico-relay-slot-v0");
    h.update(secret);
    h.finalize().into()
}
