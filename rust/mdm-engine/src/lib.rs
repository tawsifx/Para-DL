// SPDX-License-Identifier: GPL-3.0-or-later
// Trimmed MDM engine for paradl — ported from Mister Download Manager (c) Ahmed Nefzaoui,
// https://github.com/anefzaoui/misterdownloadmanager, GPL-3.0-or-later.
// Adapted by Tawsif Rahman, 2026-11-17. See each module header for specific modifications.
//! A trimmed, in-process download engine: pure segmentation planner, crash-safe resume state,
//! token-bucket limiter, checksum verifier, Range probe, and a bounded Tokio worker pool.
//! No git-patched dependencies, no BoringSSL/rquickjs, no FTP/torrent/site modules.

pub mod download;
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
pub use error::{Error, Result};
pub use planner::{Assignment, Claim, Planner, SavedSegment};
pub use verify::{Algo, Hash};
