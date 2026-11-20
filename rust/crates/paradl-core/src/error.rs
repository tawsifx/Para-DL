//! The single public error type for every `paradl-*` crate, plus the `ExitCode` model.
//!
//! Rust owns the entire error-to-exit-code mapping (framework charter section 3,
//! parity row L15). The Python media worker emits only a structured `error.kind`;
//! it never emits exit code 3.

use std::fmt;

/// Process exit codes, byte-identical to `paradl.core.models.ExitCode`.
///
/// ```
/// use paradl_core::ExitCode;
/// assert_eq!(ExitCode::VerificationFailed.as_i32(), 3);
/// assert_eq!(ExitCode::Interrupted.to_string(), "130");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ExitCode {
    /// Terminal success (`0`).
    Ok = 0,
    /// Transfer, runtime, or guard failure (`1`).
    Failure = 1,
    /// Bad CLI argument, configuration, or rate specification (`2`).
    Usage = 2,
    /// Checksum, size, or archive-security verification failure (`3`).
    VerificationFailed = 3,
    /// Missing external dependency such as yt-dlp or a JS runtime (`4`).
    MissingDependency = 4,
    /// SIGINT/SIGTERM interruption (`130`).
    Interrupted = 130,
}

impl ExitCode {
    /// Return the numeric value of this exit code.
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Map a media worker `error.kind` string to an exit code.
    ///
    /// `metadata`, `download`, and `conversion` map to [`ExitCode::Failure`];
    /// `dependency` to [`ExitCode::MissingDependency`]; `usage` to
    /// [`ExitCode::Usage`]; `interrupted` to [`ExitCode::Interrupted`]. Unknown
    /// kinds fail closed to [`ExitCode::Failure`]. The worker never emits `3`.
    pub fn from_media_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "dependency" => Self::MissingDependency,
            "usage" => Self::Usage,
            "interrupted" => Self::Interrupted,
            _ => Self::Failure,
        }
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_i32())
    }
}

impl From<ExitCode> for i32 {
    fn from(code: ExitCode) -> Self {
        code.as_i32()
    }
}

/// The single public error type reused by every crate in the framework.
///
/// Each variant carries a human-readable message and maps deterministically to
/// one [`ExitCode`] via [`Error::exit_code`].
///
/// ```
/// use paradl_core::{Error, ExitCode};
/// assert_eq!(Error::usage("bad flag").exit_code(), ExitCode::Usage);
/// assert_eq!(
///     Error::from_media_kind("dependency", "yt-dlp missing").exit_code(),
///     ExitCode::MissingDependency,
/// );
/// ```
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Terminal success sentinel, used where a `Result` needs the zero code.
    #[error("ok")]
    Ok,

    /// Transfer, runtime, or guard failure (exit code `1`).
    #[error("{0}")]
    Failure(String),

    /// Invalid CLI argument, configuration, or rate (exit code `2`).
    #[error("{0}")]
    Usage(String),

    /// Checksum, size, or archive-security failure (exit code `3`).
    #[error("verification failed: {0}")]
    VerificationFailed(String),

    /// Missing external dependency such as yt-dlp (exit code `4`).
    #[error("missing dependency: {0}")]
    MissingDependency(String),

    /// SIGINT/SIGTERM interruption (exit code `130`).
    #[error("interrupted: {0}")]
    Interrupted(String),

    /// Underlying I/O failure; maps to [`ExitCode::Failure`].
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Construct a generic failure (exit code `1`).
    pub fn failure(message: impl Into<String>) -> Self {
        Self::Failure(message.into())
    }

    /// Construct a usage/configuration error (exit code `2`).
    pub fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    /// Construct a verification failure (exit code `3`).
    pub fn verification_failed(message: impl Into<String>) -> Self {
        Self::VerificationFailed(message.into())
    }

    /// Construct a missing-dependency error (exit code `4`).
    pub fn missing_dependency(message: impl Into<String>) -> Self {
        Self::MissingDependency(message.into())
    }

    /// Construct an interruption error (exit code `130`).
    pub fn interrupted(message: impl Into<String>) -> Self {
        Self::Interrupted(message.into())
    }

    /// Build an error from a media worker `error.kind` and message.
    pub fn from_media_kind(kind: &str, message: impl Into<String>) -> Self {
        match ExitCode::from_media_kind(kind) {
            ExitCode::Ok => Self::Ok,
            ExitCode::Failure => Self::failure(message),
            ExitCode::Usage => Self::usage(message),
            ExitCode::VerificationFailed => Self::verification_failed(message),
            ExitCode::MissingDependency => Self::missing_dependency(message),
            ExitCode::Interrupted => Self::interrupted(message),
        }
    }

    /// Return the process exit code this error maps to.
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Ok => ExitCode::Ok,
            Self::Failure(_) | Self::Io(_) => ExitCode::Failure,
            Self::Usage(_) => ExitCode::Usage,
            Self::VerificationFailed(_) => ExitCode::VerificationFailed,
            Self::MissingDependency(_) => ExitCode::MissingDependency,
            Self::Interrupted(_) => ExitCode::Interrupted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_maps_to_failure() {
        let err = Error::from(std::io::Error::other("disk"));
        assert_eq!(err.exit_code(), ExitCode::Failure);
        assert!(err.to_string().contains("disk"));
    }
}
