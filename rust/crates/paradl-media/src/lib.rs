//! Rust-side protocol client for the Python media worker.
//!
//! This crate contains **no extractor logic**: it discovers the venv Python,
//! spawns one-shot `python -m paradl_media` workers, and enforces the
//! `paradl-media/1` handshake. Wave 0 scaffolding; the public `MediaResolver`
//! trait and `media` feature land in W3-4b.
#![warn(missing_docs)]
