//! Error construction for the engine.
//!
//! Every failure surfaced by this crate is a [`paradl_core::Error`], whose
//! [`ExitCode`](paradl_core::ExitCode) mapping is owned by `paradl-core`
//! (framework charter section 3). The helpers below centralise that mapping so
//! callers never invent their own exit codes:
//!
//! - transport, HTTP, timeout and invalid-state failures → `Failure` (1);
//! - checksum and verification failures → `VerificationFailed` (3);
//! - cancellation → `Interrupted` (130).

use paradl_core::Error;

/// The result type used throughout `paradl-engine`.
pub type Result<T> = std::result::Result<T, Error>;

/// A network or transport failure (exit code 1).
pub(crate) fn network(msg: impl Into<String>) -> Error {
    Error::failure(msg)
}

/// An unexpected HTTP status (exit code 1).
pub(crate) fn status(code: u16) -> Error {
    Error::failure(format!("unexpected HTTP status {code}"))
}

/// No URI was available to download from (exit code 1).
pub(crate) fn no_uri() -> Error {
    Error::failure("no URI available")
}

/// A corrupt or inconsistent saved resume state (exit code 1).
pub(crate) fn invalid_state(msg: impl Into<String>) -> Error {
    Error::failure(format!("invalid resume state: {}", msg.into()))
}

/// A general verification failure (exit code 3).
pub(crate) fn verification(msg: impl Into<String>) -> Error {
    Error::verification_failed(msg)
}

/// A checksum mismatch after verifying the part file (exit code 3).
pub(crate) fn checksum_mismatch(expected: &str) -> Error {
    Error::verification_failed(format!("checksum mismatch: expected {expected}"))
}

/// A cancelled transfer (exit code 130, matching SIGINT/SIGTERM).
pub(crate) fn cancelled() -> Error {
    Error::interrupted("download cancelled")
}

/// A bounded wait elapsed (exit code 1).
pub(crate) fn timeout() -> Error {
    Error::failure("timed out")
}

/// A URL parse failure (exit code 1). Used by the opt-in DoH endpoint parser.
#[cfg(feature = "doh")]
pub(crate) fn url(e: url::ParseError) -> Error {
    Error::failure(format!("url error: {e}"))
}
