// Modification: only the `.pdl.part` naming helper is retained; the full naming.rs
// (length limits, per-platform path rules) is out of scope here.

use std::path::{Path, PathBuf};

/// `<dest>.pdl.part` — the sparse part file every worker writes into at its own offset.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".pdl.part");
    PathBuf::from(s)
}

/// `<dest>.pdl.json` — the resume sidecar (kept in `state.rs`, re-exported here for callers).
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".pdl.json");
    PathBuf::from(s)
}
