//! # paradl-engine
//!
//! Reusable download engine for paraDL, promoting the `rapids` core into a
//! public, documented crate. It provides the framework's [`DownloadEngine`]
//! trait (charter section 2) together with the planner, resume state, token
//! bucket limiter, checksum verifier, Range probe and bounded Tokio worker
//! pool that back it.
//!
//! Byte-exact contracts are preserved:
//!
//! - [`MAX_CONNECTIONS`] is 16 and every requested count is clamped with
//!   `min(max(n, 1), 16)` (parity literal L06);
//! - a close-delimited 200 is streamed once with `total = None` until EOF,
//!   never a false 100% (L08);
//! - `.pdl.part` plus a pretty-printed `.pdl.json` sidecar survive interrupts
//!   and resume byte-exactly, with verify-then-rename (L14);
//! - redirected URIs survive connection recycle, with IPv6, proxy, host-scoped
//!   credentials and mirror failover.
//!
//! DNS-over-HTTPS is opt-in via the `doh` feature (hickory-dns); with it off
//! the operating-system resolver is used. HTTP/3 is accepted and logged-dropped
//! with a reason — it is deliberately not a Cargo feature.
//!
//! ```
//! use paradl_engine::{DownloadEngine, NdEngine, Target, clamp_connections, MAX_CONNECTIONS};
//!
//! let engine = NdEngine::new();
//! assert_eq!(engine.capabilities().max_connections, MAX_CONNECTIONS);
//! assert_eq!(clamp_connections(99), 16);
//! let target = Target {
//!     connections: 99,
//!     http3: true, // accepted, logged-dropped with a reason — not a Cargo feature
//!     ..Target::default()
//! };
//! assert_eq!(clamp_connections(target.connections), 16);
//! // A real transfer is `let plan = engine.plan(&target).await?;` followed by
//! // `engine.download(plan, &mut sink).await?`.
//! ```
//!
//! [`DownloadEngine`]: engine::DownloadEngine

#![warn(missing_docs)]

pub mod download;
pub mod engine;
pub mod error;
pub mod limiter;
pub mod naming;
pub mod planner;
pub mod probe;
pub mod state;
pub mod verify;

#[cfg(test)]
mod tests;

pub use download::{Download, Outcome, Snapshot, StartOptions, State, UriPool};
pub use engine::{
    DownloadEngine, EngineCapabilities, MAX_CONNECTIONS, MIN_CONNECTIONS, NdEngine, NoopProgressSink,
    Plan, ProgressSink, Target, clamp_connections,
};
pub use error::Result;
pub use limiter::Limiter;
pub use naming::{part_path, state_path};
pub use planner::{Assignment, Claim, InvalidState, Planner, SavedSegment, Segment, WorkerId};
pub use probe::{ProbeInfo, headers_from_pairs, scope_headers};
pub use state::{ResumeState, Validators};
pub use verify::{Algo, Hash};
