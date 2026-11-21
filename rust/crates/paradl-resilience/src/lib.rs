//! # paradl-resilience
//!
//! Platform services around the download engine: priority queuing on the shared
//! `queue_jobs` table, deterministic launchd plist generation, isolated result
//! hooks (30 s timeout, JSON on stdin, SIGKILL on hang), the WAKE / NET_CHANGE /
//! STALL event source, and credential lookup (env → keychain → none).
//!
//! **Single-emitter rule:** this crate never formats JSON by hand and never
//! writes stdout. Every observable outcome is a typed `paradl_events` value,
//! wrapped in [`Emitted`] and serialized exclusively through
//! [`paradl_events::emit_jsonl`]. In particular, the raw `json.dumps` call at
//! `src/paradl/throttle/cli.py:296-308` (E58) is replaced by
//! `paradl_events::ThrottleLaunchdPlistGeneratedEvent`.
//!
//! All errors use [`paradl_core::Error`] and therefore carry the full
//! [`paradl_core::ExitCode`] mapping.
//!
//! ```
//! use paradl_resilience::{Emitted, auth::{Service, EnvLookup, MapEnv, InMemorySecretStore, lookup_token}};
//!
//! let env = MapEnv::new([("HF_TOKEN".to_string(), "token-value-1234".to_string())]);
//! let store = InMemorySecretStore::new();
//! let found = lookup_token(Service::Hf, &env, &store).unwrap();
//! assert_eq!(found.source, paradl_resilience::auth::TokenSource::Env);
//! let _: Emitted = Emitted::from(paradl_events::Event::HostsReset(
//!     paradl_events::HostsResetEvent::new(1.0, 2),
//! ));
//! ```

#![warn(missing_docs)]

pub mod auth;
pub mod events;
pub mod hooks;
pub mod plist;
pub mod queue;

use paradl_events::Event;

/// Result type used throughout this crate, with [`paradl_core::Error`].
pub type Result<T> = std::result::Result<T, paradl_core::Error>;

/// Return `round(time.time(), 3)` for an emitted event.
pub(crate) fn now() -> f64 {
    paradl_events::now_timestamp()
}

/// A typed event produced by this crate, ready for the single emitter.
///
/// The wrapped [`Event`] is public so callers can inspect it; [`Emitted::to_jsonl`]
/// is the only serialization path and delegates to `paradl-events`.
#[derive(Debug, Clone, PartialEq)]
pub struct Emitted {
    event: Event,
}

impl Emitted {
    /// Wrap a typed `paradl-events` event.
    pub fn new(event: Event) -> Self {
        Self { event }
    }

    /// Borrow the underlying typed event.
    pub fn event(&self) -> &Event {
        &self.event
    }

    /// Canonical event name.
    pub fn name(&self) -> &'static str {
        self.event.name()
    }

    /// Serialize to one JSONL line using the single emitter.
    pub fn to_jsonl(&self) -> String {
        paradl_events::emit_jsonl(&self.event)
    }
}

impl From<Event> for Emitted {
    fn from(event: Event) -> Self {
        Self::new(event)
    }
}
