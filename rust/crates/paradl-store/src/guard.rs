//! Process-wide, once-per-physical-file schema initialization guard.
//!
//! Port of `HistoryManager._schema_guard_key` / `ThrottleStore._schema_guard_key`
//! (`history.py:131-143`, `store.py:36-60`). The identity is the **resolved path
//! plus device and inode**, so deleting and recreating a database file gets a
//! fresh initialization even if the path string is unchanged.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GuardKey {
    path: PathBuf,
    device: u64,
    inode: u64,
}

static INITIALIZED: OnceLock<Mutex<HashSet<GuardKey>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashSet<GuardKey>> {
    INITIALIZED.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((
        u64::from(meta.volume_serial_number().unwrap_or(0)),
        meta.file_index().unwrap_or(0),
    ))
}

#[cfg(not(any(unix, windows)))]
fn identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// A snapshot of the identity of a database file, captured after opening it.
pub(crate) struct InitGuard {
    key: Option<GuardKey>,
}

impl InitGuard {
    /// Capture the identity of `path`. Returns a key-less guard when the file
    /// cannot be stat'ed; in that case initialization is always attempted.
    pub(crate) fn new(path: &Path) -> Self {
        let key = identity(path).map(|(device, inode)| GuardKey {
            path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
            device,
            inode,
        });
        Self { key }
    }

    /// Return `true` when this exact physical file was already initialized.
    pub(crate) fn already_initialized(&self) -> bool {
        let Some(key) = &self.key else {
            return false;
        };
        registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(key)
    }

    /// Record that this physical file has been initialized.
    pub(crate) fn mark_initialized(&self) {
        if let Some(key) = &self.key {
            registry()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.clone());
        }
    }
}
