//! # paradl-media
//!
//! Rust-side **protocol client and router** for the Python media worker.
//! This crate contains **no extractor logic**: media stays a one-shot worker
//! invoked as `python -m paradl_media` over `paradl-media/1` (final master
//! prompt ruling 1). No PyO3, no long-lived sidecar, no Python runtime on
//! non-media paths.
//!
//! The client owns the version handshake, request correlation, timeout/SIGINT
//! supervision, and the complete `error.kind` → [`paradl_core::ExitCode`]
//! mapping. Worker progress frames are converted to `paradl-events` lines with
//! redaction and handed to a [`ProgressSink`]; this crate never writes raw
//! JSONL (or anything else) to stdout.
//!
//! ```
//! use paradl_media::{MediaRoute, WorkerMediaResolver, check_protocol, classify_route};
//!
//! assert!(check_protocol(Some("paradl-media/1.1")).is_ok());
//! assert!(check_protocol(Some("paradl-media/2")).is_err());
//! assert_eq!(classify_route("ytsearch1:lofi"), MediaRoute::Managed);
//! let _resolver = WorkerMediaResolver::new(); // discovery happens per op
//! ```
//!
//! See `examples/protocol_client.rs` for a runnable stub-worker walkthrough.
#![warn(missing_docs)]

pub mod discovery;
pub mod progress;
pub mod protocol;
pub mod router;
mod worker;

pub use discovery::{
    MEDIA_PY_ENV, WORKER_MODULE, WorkerLaunch, discover_python,
};
pub use progress::{NoopProgressSink, ProgressSink, reemit_progress};
pub use protocol::{
    ErrorKind, MediaData, MediaOptions, MediaRequest, MediaResult, ProgressFrame, PROTOCOL_ID,
    PROTOCOL_MAJOR, PROTOCOL_PREFIX, ProtocolOp, ProtocolRequest, WorkerFrame, WorkerVersions,
    check_protocol, error_kind_to_error, error_kind_to_exit_code,
};
pub use router::{MediaRoute, classify_route};
pub use worker::{DEFAULT_WORKER_TIMEOUT, WorkerMediaResolver};

use async_trait::async_trait;
use paradl_core::Error;

/// Convenience result alias: every media operation returns the framework error.
pub type Result<T> = std::result::Result<T, Error>;

/// The authoritative media resolver interface (framework charter §2).
///
/// Implementations spawn one short-lived worker process per call. `domains()`
/// comes from the single `paradl-core` domain constants; the live `claims`
/// answer and the direct-vs-managed decision come from the worker.
///
/// ```
/// use paradl_media::{MediaResolver, WorkerMediaResolver};
/// fn takes_resolver(resolver: &dyn MediaResolver) -> usize {
///     resolver.domains().len()
/// }
/// assert!(takes_resolver(&WorkerMediaResolver::new()) > 0);
/// ```
#[async_trait]
pub trait MediaResolver: Send + Sync {
    /// Media domains served by this resolver (from `paradl_core::MEDIA_DOMAINS`).
    fn domains(&self) -> &'static [&'static str];

    /// Ask the worker for metadata without downloading media bytes.
    ///
    /// A `claims=false` answer returns [`MediaResult::is_direct`] with no data,
    /// so the caller can fall back to `paradl-engine`.
    async fn resolve(&self, req: MediaRequest) -> Result<MediaResult>;

    /// Download and tag managed media, forwarding progress to `sink`.
    async fn download_managed(
        &self,
        req: MediaRequest,
        sink: &mut dyn ProgressSink,
    ) -> Result<MediaResult>;

    /// Ask the live yt-dlp extractor registry whether it claims `url`.
    async fn claims(&self, url: &str) -> Result<bool>;
}
