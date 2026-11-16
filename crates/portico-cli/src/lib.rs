pub mod bridge;
pub mod discover;
pub mod transport;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("protocol error: {0}")]
    Proto(#[from] portico_proto::ProtoError),
    #[error("core error: {0}")]
    Core(#[from] portico_core::CoreError),
    #[error("untrusted server certificate ({endpoint}): SHA-256 {fingerprint}\nRe-run with --trust {fingerprint} or run `portico trust {endpoint} {fingerprint}`")]
    UntrustedCert {
        endpoint: String,
        fingerprint: String,
    },
    #[error("certificate pin mismatch for {endpoint}: expected {expected}, got {actual}")]
    CertPinMismatch {
        endpoint: String,
        expected: String,
        actual: String,
    },
    #[error("plaintext LAN mode refused for non-LAN target '{0}'")]
    PlaintextNonLanRefused(String),
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    #[error("remote error: {0}")]
    Remote(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
