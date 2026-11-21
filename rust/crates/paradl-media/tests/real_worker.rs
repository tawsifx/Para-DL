//! Real-worker integration test: drives `python -m paradl_media` end to end.
//!
//! The stub tests in `tests/worker.rs` intentionally ignore `out_dir`, which
//! is exactly how the empty-`out_dir` defect stayed hidden: the Python worker
//! validates a non-empty `out_dir` for **every** op (including `claims`), but
//! the stubs never checked it. This test uses the ACTUAL Python worker so the
//! Rust client must send a real absolute path or the worker rejects the
//! request with `error.kind="usage"` / exit 2.
//!
//! Run (from the worktree root, or anywhere cargo reaches this crate):
//!
//! ```bash
//! cargo test -p paradl-media --test real_worker -- --nocapture
//! ```
//!
//! The test discovers the worktree `.venv/bin/python` automatically. In CI,
//! set `PARADL_TEST_MEDIA_PY` to a Python with `paradl_media` importable.

use std::path::{Path, PathBuf};

use paradl_media::{MediaResolver, WorkerLaunch, WorkerMediaResolver};

/// Worktree root, derived from this crate's manifest dir
/// (`<root>/rust/crates/paradl-media`).
fn worktree_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("worktree root must exist")
}

fn worker_python(root: &Path) -> PathBuf {
    std::env::var_os("PARADL_TEST_MEDIA_PY")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".venv/bin/python"))
}

#[tokio::test]
async fn real_worker_accepts_claims_with_nonempty_out_dir() {
    let root = worktree_root();
    let python = worker_python(&root);
    if !python.is_file() {
        eprintln!(
            "SKIP: media worker Python not found at {}; set PARADL_TEST_MEDIA_PY",
            python.display()
        );
        return;
    }

    // The worker imports `paradl_media` straight from the source tree; no pip
    // install into the venv is required for this test.
    // SAFETY: this integration-test binary runs a single test, so no other
    // thread reads the process environment while it is being changed, and the
    // worker spawned below inherits the updated value.
    unsafe { std::env::set_var("PYTHONPATH", root.join("media")) };

    let resolver = WorkerMediaResolver::with_launch(WorkerLaunch::python(&python));
    let claimed = resolver
        .claims("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        .await
        .expect("non-empty absolute out_dir must be accepted and claims result must parse");

    assert!(
        claimed,
        "a YouTube watch URL is claimed by yt-dlp's extractor registry"
    );
}
