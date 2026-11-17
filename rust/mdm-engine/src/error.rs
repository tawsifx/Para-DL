
use thiserror::Error;

/// Errors surfaced by the trimmed engine. Kept small: the Python adapter maps these to
/// the existing `ExitCode`/`TransferResult` contract (notably VERIFICATION_FAILED).
#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(String),

    #[error("unexpected HTTP status {0}")]
    Status(u16),

    #[error("no URI available")]
    NoUri,

    #[error("invalid resume state: {0}")]
    InvalidState(String),

    #[error("verification failed: {0}")]
    Verification(String),

    #[error("checksum mismatch after download")]
    ChecksumMismatch,

    #[error("download cancelled")]
    Cancelled,

    #[error("timed out")]
    Timeout,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
