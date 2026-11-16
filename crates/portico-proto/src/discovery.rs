use crate::ProtoError;

pub const DISCOVERY_MAGIC: &[u8; 4] = b"PRDS";
pub const DISCOVERY_VERSION_V0: u8 = 0x00;
const TYPE_REQUEST: u8 = 0x01;
const TYPE_RESPONSE: u8 = 0x02;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryPacket {
    Request {
        nonce: [u8; 8],
    },
    Response {
        nonce: [u8; 8],
        tcp_port: u16,
        tls_required: bool,
        plaintext_lan: bool,
        cert_fp_prefix: [u8; 8],
        node_name: String,
    },
}

impl DiscoveryPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(DISCOVERY_MAGIC);
        out.push(DISCOVERY_VERSION_V0);
        match self {
            DiscoveryPacket::Request { nonce } => {
                out.push(TYPE_REQUEST);
                out.extend_from_slice(nonce);
            }
            DiscoveryPacket::Response {
                nonce,
                tcp_port,
                tls_required,
                plaintext_lan,
                cert_fp_prefix,
                node_name,
            } => {
                out.push(TYPE_RESPONSE);
                out.extend_from_slice(nonce);
                out.extend_from_slice(&tcp_port.to_be_bytes());
                let mut flags = 0u8;
                if *tls_required {
                    flags |= 1 << 0;
                }
                if *plaintext_lan {
                    flags |= 1 << 1;
                }
                out.push(flags);
                out.extend_from_slice(cert_fp_prefix);
                let name_bytes = node_name.as_bytes();
                let name_len = name_bytes.len().min(255);
                out.push(name_len as u8);
                out.extend_from_slice(&name_bytes[..name_len]);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        if bytes.len() < 6 {
            return Err(ProtoError::MalformedPayload("discovery packet too short".into()));
        }
        if &bytes[0..4] != DISCOVERY_MAGIC {
            return Err(ProtoError::InvalidMagic);
        }
        if bytes[4] != DISCOVERY_VERSION_V0 {
            return Err(ProtoError::UnsupportedVersion(bytes[4]));
        }
        match bytes[5] {
            TYPE_REQUEST => {
                if bytes.len() != 14 {
                    return Err(ProtoError::MalformedPayload("invalid discovery request len".into()));
                }
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&bytes[6..14]);
                Ok(DiscoveryPacket::Request { nonce })
            }
            TYPE_RESPONSE => {
                if bytes.len() < 26 {
                    return Err(ProtoError::MalformedPayload("discovery response too short".into()));
                }
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&bytes[6..14]);
                let tcp_port = u16::from_be_bytes([bytes[14], bytes[15]]);
                let flags = bytes[16];
                let tls_required = (flags & (1 << 0)) != 0;
                let plaintext_lan = (flags & (1 << 1)) != 0;
                let mut cert_fp_prefix = [0u8; 8];
                cert_fp_prefix.copy_from_slice(&bytes[17..25]);
                let name_len = bytes[25] as usize;
                if bytes.len() != 26 + name_len {
                    return Err(ProtoError::MalformedPayload("discovery node_name len mismatch".into()));
                }
                let node_name = String::from_utf8(bytes[26..].to_vec())
                    .map_err(|_| ProtoError::MalformedPayload("invalid utf8 node_name".into()))?;
                Ok(DiscoveryPacket::Response {
                    nonce,
                    tcp_port,
                    tls_required,
                    plaintext_lan,
                    cert_fp_prefix,
                    node_name,
                })
            }
            other => Err(ProtoError::UnknownMessageType(other)),
        }
    }
}
