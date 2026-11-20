//! Three-lane differential test harness for paraDL: non-media (Python oracle vs
//! Rust binary), media (golden replay), and boundary (protocol frame) lanes.
//! Development-only.
#![warn(missing_docs)]

pub mod boundary;
pub mod diff;
pub mod media;
pub mod nonmedia;

pub use boundary::{BoundaryCase, BoundaryOutcome, boundary_process_exit_code, run_boundary_case};
pub use diff::{DiffReport, Difference, diff_bytes, diff_snapshots, diff_text, normalize_jsonl, snapshot_dir};
pub use media::{MediaReplayOutcome, run_media_replay};
pub use nonmedia::{LoopbackServer, NonMediaCase, NonMediaOutcome, PythonGuard, run_nonmedia_case};
