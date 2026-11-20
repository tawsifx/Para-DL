//! Byte-exact golden fixtures and contract tests for the paraDL Rust framework migration.
//!
//! This crate mirrors the Python oracle so that later crates can verify byte-exact parity.
//! It is a **dev-only documentation crate**: it holds captured oracle outputs as JSON fixtures
//! and tests that assert those values verbatim. It contains no application logic and is never
//! published.
//!
//! # Standing ruling
//!
//! The Python application becomes Rust **except the media subsystem**, which stays Python and
//! is invoked as `python -m paradl_media` over the versioned JSON-lines protocol
//! `paradl-media/1`. Prior "no Python anywhere" criteria are void. Rust owns the full
//! `ExitCode` map and is the sole JSONL emitter; the worker emits only structured
//! `error.kind` and writes human logs to stderr.
//!
//! # Fixtures
//!
//! Every golden is captured by *running* the Python oracle (or the `rapids` Rust oracle), never
//! invented. See [`fixtures`] for accessors and the crate README for the source of each file.
//!
//! # Contract tests
//!
//! The `#[cfg(test)]` modules below assert exact captured literals. Tests that need a crate not
//! yet implemented are `#[ignore]`d with their expected values already present.
#![warn(missing_docs)]

/// Accessors for the embedded golden fixtures.
pub mod fixtures;

#[cfg(test)]
mod classifier_tests;
#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod media_protocol_tests;
#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod sidecar_tests;
#[cfg(test)]
mod watchdog_tests;
