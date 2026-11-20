//! The single JSONL event emitter for paraDL.
//!
//! All observable events (~48 shapes), their redaction rules, and the event
//! schema version constant live here. Worker progress frames are re-emitted
//! through this crate only. Wave 0 scaffolding; the schema lands in W2-4.
#![warn(missing_docs)]
