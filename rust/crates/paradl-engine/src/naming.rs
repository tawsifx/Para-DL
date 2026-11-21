//! Byte-exact filenames for the download's part file and resume sidecar.
//!
//! The suffixes are contract literals L14 in the parity matrix: `<dest>.pdl.part`
//! holds the sparse bytes and `<dest>.pdl.json` holds the resume state.

use std::path::{Path, PathBuf};

/// `<dest>.pdl.part` — the sparse part file every worker writes into at its own offset.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".pdl.part");
    PathBuf::from(s)
}

/// `<dest>.pdl.json` — the resume sidecar written beside the part file.
///
/// This is defined here and re-exported from [`crate::state`] so both callers
/// and the ported tests refer to a single implementation.
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".pdl.json");
    PathBuf::from(s)
}
