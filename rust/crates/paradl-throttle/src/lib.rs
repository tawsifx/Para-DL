//! # paradl-throttle
//!
//! Pure computational core of the paraDL throttling diagnostic suite: a
//! byte-exact Rust port of the Python classifier, diagnostic-text parsers,
//! data-budget tracker, probe statistics, target helpers, report generators,
//! and an optional Tokio VPS ground-truth server.
//!
//! The media subsystem stays Python (`paradl-media/1`); everything in this
//! crate is native Rust and never spawns a Python process.
//!
//! ## Byte-exact parity
//!
//! * The 12 seeded `test_classifier.py` cases and all coverage edge cases
//!   reproduce the W0-2 `classifier_seeded.json` golden strings exactly.
//! * [`report::generate_report_json`] reproduces the `report.json` schema
//!   `1.0` golden (`report_schema.json`) byte-for-byte, including
//!   `modules.m0..m10` key order.
//! * Python `round()` (banker's rounding), `statistics.median/fmean/pstdev`,
//!   and f-string fixed-point formatting are mirrored in [`numeric`].
//!
//! ## Single-emitter rule
//!
//! This crate never writes raw JSON to stdout. [`events`] defines typed,
//! serde-serializable event payloads with the `{event, timestamp, ...redacted}`
//! shape for W2-4's `paradl-events` crate to consume and emit centrally.
//!
//! ```
//! use paradl_throttle::classifier::{compute_next_best_hours, mann_whitney_u_test};
//! use paradl_throttle::models::{SampleRecord, ThrottleReport};
//! use paradl_throttle::report::generate_report_json;
//!
//! assert_eq!(mann_whitney_u_test(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]), (0.0, 0.080856));
//! assert_eq!(compute_next_best_hours(&[]).len(), 4);
//! let sample = SampleRecord { hour: 21, download_bps: 5_000_000.0, ..SampleRecord::default() };
//! assert_eq!(compute_next_best_hours(&[sample]).len(), 4);
//! assert!(generate_report_json(&ThrottleReport::default()).contains("\"schema_version\": \"1.0\""));
//! ```

#![warn(missing_docs)]
// Nested `if` blocks mirror the Python oracle's `and`-chained conditions
// one-to-one, which keeps the byte-exact port auditable against the source.
#![allow(clippy::collapsible_if)]

pub use paradl_core::{Error, ExitCode};

/// Convenience result alias using the single framework error type.
pub type Result<T> = std::result::Result<T, Error>;

pub mod budget;
pub mod classifier;
pub mod events;
pub mod models;
pub mod numeric;
pub mod parsers;
pub mod probes;
pub mod report;
pub mod server;
pub mod targets;
