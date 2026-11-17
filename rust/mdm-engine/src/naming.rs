// Modification: only the `.mdm.part` naming helper is retained; MDM's full naming.rs
// (length limits, per-platform path rules) is out of scope for the trimmed port.

use std::path::{Path, PathBuf};

/// `<dest>.mdm.part` — the sparse part file every worker writes into at its own offset.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".mdm.part");
    PathBuf::from(s)
}

/// `<dest>.mdm.json` — the resume sidecar (kept in `state.rs`, re-exported here for callers).
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".mdm.json");
    PathBuf::from(s)
}
