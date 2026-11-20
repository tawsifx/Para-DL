//! Public data models persisted by [`crate::Store`].

use serde_json::{Map, Value};
use std::path::Path;

/// One row of the `downloads` transfer history table.
///
/// Mirrors the dict returned by `HistoryManager.get_entries` in the Python
/// oracle. `id` and `timestamp` are filled in by
/// [`Store::record_transfer`](crate::Store::record_transfer) and
/// [`Store::recent`](crate::Store::recent) respectively; callers pass them as
/// zero / empty via [`TransferRecord::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRecord {
    /// Database primary key; `0` before insertion.
    pub id: i64,
    /// Local timestamp in `YYYY-MM-DD HH:MM:SS` form.
    pub timestamp: String,
    /// Original download URL or search target.
    pub target: String,
    /// Download type (`direct`, `stream`, ...); the `type` SQL column.
    pub target_type: String,
    /// Basename of the downloaded file (`-` when unknown).
    pub filename: String,
    /// Full local path of the downloaded file (`-` when unknown).
    pub path: String,
    /// Size in bytes.
    pub size: i64,
    /// Duration in seconds, rounded to two decimals on write.
    pub duration: f64,
    /// Whether the transfer succeeded.
    pub success: bool,
    /// Whether the file was handed to ADB.
    pub adb: bool,
    /// Optional SHA-256 digest.
    pub sha256: Option<String>,
}

impl TransferRecord {
    /// Build a record with the oracle's defaults for `id` and `timestamp`.
    // Eight arguments mirror the eight persisted `downloads` columns written by
    // `HistoryManager.record`; splitting them would obscure the 1:1 parity.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        target: impl Into<String>,
        target_type: impl Into<String>,
        path: impl AsRef<Path>,
        size: i64,
        duration: f64,
        success: bool,
        adb: bool,
        sha256: Option<String>,
    ) -> Self {
        let path = path.as_ref();
        Self {
            id: 0,
            timestamp: String::new(),
            target: target.into(),
            target_type: target_type.into(),
            filename: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".to_string()),
            path: path.to_string_lossy().into_owned(),
            size,
            duration,
            success,
            adb,
            sha256,
        }
    }
}

/// One row of the shared `queue_jobs` table.
///
/// The defaults match `QueueJob` in `src/paradl/resilience/queue.py`, including
/// `require_ac = true` and the 1–16 connection clamp applied on insert.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueJob {
    /// Database primary key; `0` before insertion.
    pub id: i64,
    /// Download target (URL), stored after trimming.
    pub target: String,
    /// Target type; defaults to `direct`.
    pub target_type: String,
    /// Absolute output directory.
    pub out_dir: String,
    /// Optional output filename.
    pub filename: Option<String>,
    /// Mirror URLs.
    pub mirrors: Vec<String>,
    /// Higher priority jobs run first.
    pub priority: i64,
    /// Connection count, clamped to `1..=16` on insert.
    pub connections: i64,
    /// One of `queued`, `running`, `paused`, `completed`, `failed`.
    pub status: String,
    /// Require AC power before starting.
    pub require_ac: bool,
    /// Pause on low battery.
    pub pause_on_low_power: bool,
    /// Minimum speed threshold for `--when-fast`.
    pub when_fast_bps: f64,
    /// Optional ISO start time.
    pub start_at: Option<String>,
    /// Optional expected content length.
    pub expected_length: Option<i64>,
    /// Optional expected digest.
    pub expected_digest: Option<String>,
    /// Job category label.
    pub category: String,
    /// Bytes downloaded so far.
    pub bytes_downloaded: i64,
    /// Last error message, if any.
    pub error: Option<String>,
    /// Arbitrary job options, serialized as `options_json`.
    pub options: Value,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
}

impl QueueJob {
    /// Build a queued job with the oracle's defaults.
    pub fn new(target: impl Into<String>, out_dir: impl AsRef<Path>) -> Self {
        let out_dir = out_dir.as_ref();
        Self {
            id: 0,
            target: target.into(),
            target_type: "direct".to_string(),
            out_dir: out_dir.to_string_lossy().into_owned(),
            filename: None,
            mirrors: Vec::new(),
            priority: 0,
            connections: 16,
            status: "queued".to_string(),
            require_ac: true,
            pause_on_low_power: false,
            when_fast_bps: 0.0,
            start_at: None,
            expected_length: None,
            expected_digest: None,
            category: String::new(),
            bytes_downloaded: 0,
            error: None,
            options: Value::Object(Map::new()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}
