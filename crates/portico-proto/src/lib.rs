pub mod frame;
pub mod token;

use thiserror::Error;

pub const DEFAULT_PORT: u16 = 37420;
pub const DEFAULT_DISCOVERY_PORT: u16 = 37421;
pub const DEFAULT_RELAY_PORT: u16 = 37422;
pub const MAX_FRAME_LEN: u32 = 4_194_304;

#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("invalid protocol magic")]
    InvalidMagic,
    #[error("unsupported protocol version: {0}")]
    UnsupportedVersion(u8),
    #[error("unknown message type: {0:#04x}")]
    UnknownMessageType(u8),
    #[error("frame length {0} exceeds max {MAX_FRAME_LEN}")]
    FrameTooLarge(u32),
    #[error("malformed frame payload: {0}")]
    MalformedPayload(String),
    #[error("invalid fingerprint: {0}")]
    InvalidFingerprint(String),
    #[error("invalid relay URI: {0}")]
    InvalidRelayUri(String),
    #[error("certificate generation or TLS error: {0}")]
    Tls(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
