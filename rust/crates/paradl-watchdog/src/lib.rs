//! # paradl-watchdog
//!
//! Anti-throttle watchdog for paraDL: a faithful Rust port of the Python
//! v1/v2 state machine, rolling p90/p95 percentiles, recycle cooldown, learned
//! per-(host, hour, network) profiles, and a clock-injected supervision loop.
//!
//! The public interface is the [`Watchdog`] trait plus its concrete
//! [`WatchdogController`]. Every [`WatchdogAction`] carries the data required
//! to build a byte-exact [`WatchdogActionEvent`]; `paradl-events` (W2-4) is the
//! *only* JSONL emitter and will consume that serializable type. This crate
//! never writes to stdout and never reads real time directly — it uses
//! [`paradl_core::Clock`].
//!
//! ```
//! use std::sync::Arc;
//! use paradl_core::{FakeClock, WatchdogMode};
//! use paradl_watchdog::{
//!     ThroughputSample, Watchdog, WatchdogActionType, WatchdogConfig, WatchdogController,
//! };
//!
//! let clock = Arc::new(FakeClock::default());
//! let mut wd = WatchdogController::with_clock(
//!     WatchdogConfig {
//!         mode: WatchdogMode::V2,
//!         user_floor_bps: 1000.0,
//!         recycle_interval_sec: 0.0,
//!         ..WatchdogConfig::default()
//!     },
//!     clock,
//! );
//! let action = wd.observe(ThroughputSample::new(10.0, 500.0));
//! assert_eq!(action.action, WatchdogActionType::None);
//! assert_eq!(wd.state().mode, WatchdogMode::V2);
//! ```

#![warn(missing_docs)]

pub mod controller;
pub mod profile;
pub mod types;

pub use controller::{WatchdogConfig, WatchdogController, STALE_PROFILE_SECONDS};
pub use profile::{InMemoryProfileStore, LearnedProfile, ProfileStore};
pub use types::{
    ResilienceEvent, ResilienceEventType, ThroughputSample, WatchdogAction, WatchdogActionEvent,
    WatchdogActionType, WatchdogState, compute_hour_bucket, compute_percentile, py_round,
};

pub use paradl_core::WatchdogMode;

/// Public watchdog interface mandated by the framework charter (§2).
///
/// Implementors consume one telemetry [`ThroughputSample`] at a time and
/// expose a snapshot of their runtime state. Implementations must be safe to
/// share across download-engine threads.
pub trait Watchdog: Send + Sync {
    /// Process a telemetry sample and return the resulting action.
    fn observe(&mut self, sample: ThroughputSample) -> WatchdogAction;

    /// Return an immutable snapshot of runtime state.
    fn state(&self) -> WatchdogState;
}
