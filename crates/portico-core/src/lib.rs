pub mod config;
pub mod logging;
pub mod registry;
pub mod usb_handler;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("protocol error: {0}")]
    Proto(#[from] portico_proto::ProtoError),
    #[error("invalid config or input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("usb error: {0}")]
    Usb(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
