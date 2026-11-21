//! Three-lane differential test harness for paraDL: non-media (Python oracle vs
//! Rust binary), media (golden replay), and boundary (protocol frame) lanes.
//! Development-only.
#![warn(missing_docs)]

pub mod boundary;
pub mod diff;
pub mod media;
pub mod nonmedia;

pub use boundary::{BoundaryCase, BoundaryOutcome, boundary_process_exit_code, run_boundary_case};
pub use diff::{
    DiffReport, Difference, NONDETERMINISTIC_FIELDS, NormalizedField, diff_bytes, diff_snapshots,
    diff_text, normalize_jsonl, normalize_stdout_fields, normalize_stdout_timestamps,
    snapshot_dir, strip_nondeterministic_fields, strip_timestamp,
};
pub use media::{MediaReplayOutcome, run_media_replay};
pub use nonmedia::{
    LoopbackServer, NonMediaCase, NonMediaGate, NonMediaOutcome, PythonGuard, interrupt_payload,
    run_full_nonmedia_gate, run_interrupt_resume_gate, run_nonmedia_case,
};
