//! # paradl-store
//!
//! SQLite persistence for paraDL: the shipped `history.db` schema v4, WAL,
//! additive migrations, the device+inode schema guard, the shared
//! `queue_jobs` table, and a one-time read-only `history.json` import.
//!
//! All ten tables (7 history-owned, including `queue_jobs`, plus 3
//! throttle-owned) live in one file. Every error uses [`paradl_core::Error`]
//! and therefore carries the full [`paradl_core::ExitCode`] mapping.
//!
//! ```
//! use paradl_store::{SqliteStore, Store, TransferRecord};
//!
//! let dir = tempfile::tempdir()?;
//! let store = SqliteStore::open_at(dir.path().join("history.db"))?;
//! store.record_transfer(&TransferRecord::new(
//!     "https://example.com/file.zip", "direct", "/tmp/file.zip",
//!     1024, 1.25, true, false, None,
//! ))?;
//! assert_eq!(store.recent(5)?.len(), 1);
//! # Ok::<(), paradl_core::Error>(())
//! ```

#![warn(missing_docs)]

mod guard;
mod models;
mod queue;
mod schema;
mod store;

pub use models::{QueueJob, TransferRecord};
pub use queue::Queue;
pub use store::SqliteStore;

/// Result type used throughout this crate, with [`paradl_core::Error`].
pub type Result<T> = std::result::Result<T, paradl_core::Error>;

/// Re-export of the single framework error type for convenience.
pub use paradl_core::Error;

/// Schema version stored in `schema_meta` (`HISTORY_SCHEMA_VERSION`).
pub const SCHEMA_VERSION: &str = "4";

/// The seven history-owned tables (including the shared `queue_jobs`).
pub const HISTORY_TABLES: [&str; 7] = [
    "schema_meta",
    "downloads",
    "host_strategies",
    "learned_profiles",
    "http_cache",
    "host_caps",
    "queue_jobs",
];

/// The three throttle-owned tables, created in the same database file.
pub const THROTTLE_TABLES: [&str; 3] = ["throttle_samples", "throttle_runs", "asn_cache"];

/// The authoritative persistence interface (framework charter §2).
///
/// Every method opens a fresh managed connection, so implementations are
/// `Send + Sync` even though rusqlite connections themselves are not `Sync`.
pub trait Store: Send + Sync {
    /// Insert one transfer-history row, applying the configured history cap.
    fn record_transfer(&self, record: &TransferRecord) -> Result<()>;

    /// Return the newest transfer-history rows, most recent first.
    ///
    /// A `limit` below 1 is clamped to 1, matching `HistoryManager.get_entries`.
    fn recent(&self, limit: usize) -> Result<Vec<TransferRecord>>;

    /// Open a handle to the shared `queue_jobs` table.
    fn open_queue(&self) -> Result<Queue>;
}

/// Local timestamp in the oracle's `YYYY-MM-DD HH:MM:SS` format.
pub(crate) fn now_str() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Map a rusqlite error into the framework error type (exit code `1`).
pub(crate) fn map_err(err: rusqlite::Error) -> Error {
    Error::failure(format!("sqlite error: {err}"))
}
