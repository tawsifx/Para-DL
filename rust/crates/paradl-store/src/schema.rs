//! Byte-exact DDL and migrations for the shipped `history.db` schema v4.
//!
//! The SQL strings below are copied verbatim from the Python oracle
//! (`src/paradl/storage/history.py` and `src/paradl/throttle/store.py`),
//! including indentation. SQLite stores `sqlite_master.sql` verbatim, so any
//! whitespace change would break byte-compatibility with a shipped database.

use rusqlite::Connection;

/// Schema version stored in `schema_meta` (`HISTORY_SCHEMA_VERSION`).
pub(crate) const SCHEMA_VERSION: &str = "4";

const CREATE_SCHEMA_META: &str = r#"CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )"#;

const CREATE_DOWNLOADS: &str = r#"CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                target TEXT NOT NULL,
                type TEXT NOT NULL,
                filename TEXT NOT NULL,
                path TEXT NOT NULL,
                size INTEGER NOT NULL DEFAULT 0,
                duration REAL NOT NULL DEFAULT 0.0,
                success INTEGER NOT NULL DEFAULT 1,
                adb INTEGER NOT NULL DEFAULT 0,
                sha256 TEXT
            )"#;

const CREATE_HOST_STRATEGIES: &str = r#"CREATE TABLE IF NOT EXISTS host_strategies (
                host TEXT PRIMARY KEY,
                best_action TEXT NOT NULL DEFAULT 'tier1_native',
                connections INTEGER NOT NULL DEFAULT 16,
                ip_family TEXT NOT NULL DEFAULT 'auto',
                speed_before REAL NOT NULL DEFAULT 0.0,
                speed_after REAL NOT NULL DEFAULT 0.0,
                supports_range INTEGER,
                throttle_class TEXT,
                updated_at TEXT NOT NULL,
                avg_rtt_ms REAL NOT NULL DEFAULT 0.0,
                packet_loss_pct REAL NOT NULL DEFAULT 0.0
            )"#;

/// The v4 base table: RTT/loss columns are added by `ALTER TABLE`, exactly as
/// the oracle does, so the stored SQL text matches an upgraded database.
const CREATE_LEARNED_PROFILES: &str = r#"CREATE TABLE IF NOT EXISTS learned_profiles (
                host TEXT NOT NULL,
                hour_bucket INTEGER NOT NULL,
                network_id TEXT NOT NULL,
                connections INTEGER NOT NULL DEFAULT 16,
                ip_family TEXT NOT NULL DEFAULT 'auto',
                recovery_action TEXT NOT NULL DEFAULT 'tier2_pause_unpause',
                speed_before REAL NOT NULL DEFAULT 0.0,
                speed_after REAL NOT NULL DEFAULT 0.0,
                supports_range INTEGER,
                throttle_class TEXT,
                updated_at TEXT NOT NULL,
                updated_epoch REAL NOT NULL DEFAULT 0.0,
                source TEXT NOT NULL DEFAULT 'watchdog',
                PRIMARY KEY (host, hour_bucket, network_id)
            )"#;

const CREATE_HTTP_CACHE: &str = r#"CREATE TABLE IF NOT EXISTS http_cache (
                url TEXT PRIMARY KEY,
                etag TEXT NOT NULL,
                body_json TEXT NOT NULL,
                updated_at REAL NOT NULL
            )"#;

const CREATE_HOST_CAPS: &str = r#"CREATE TABLE IF NOT EXISTS host_caps (
                host TEXT PRIMARY KEY,
                cap INTEGER NOT NULL,
                seen INTEGER NOT NULL DEFAULT 0,
                updated_epoch REAL NOT NULL DEFAULT 0.0
            )"#;

const CREATE_QUEUE_JOBS: &str = r#"CREATE TABLE IF NOT EXISTS queue_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL,
                target_type TEXT NOT NULL DEFAULT 'direct',
                out_dir TEXT NOT NULL,
                filename TEXT,
                mirrors_json TEXT NOT NULL DEFAULT '[]',
                priority INTEGER NOT NULL DEFAULT 0,
                connections INTEGER NOT NULL DEFAULT 16,
                status TEXT NOT NULL DEFAULT 'queued',
                require_ac INTEGER NOT NULL DEFAULT 1,
                pause_on_low_power INTEGER NOT NULL DEFAULT 0,
                when_fast_bps REAL NOT NULL DEFAULT 0.0,
                start_at TEXT,
                expected_length INTEGER,
                expected_digest TEXT,
                category TEXT NOT NULL DEFAULT '',
                bytes_downloaded INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                options_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"#;

const CREATE_THROTTLE_SAMPLES: &str = r#"CREATE TABLE IF NOT EXISTS throttle_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                epoch REAL NOT NULL,
                hour INTEGER NOT NULL,
                is_weekend INTEGER NOT NULL DEFAULT 0,
                interface TEXT NOT NULL DEFAULT 'en0',
                ssid TEXT NOT NULL DEFAULT '',
                network_id TEXT NOT NULL DEFAULT 'default',
                source TEXT NOT NULL DEFAULT 'active',
                target_group TEXT NOT NULL DEFAULT 'international',
                host TEXT NOT NULL DEFAULT '',
                download_bps REAL NOT NULL DEFAULT 0.0,
                upload_bps REAL NOT NULL DEFAULT 0.0,
                rtt_ms REAL NOT NULL DEFAULT 0.0,
                bytes_used INTEGER NOT NULL DEFAULT 0,
                series_json TEXT NOT NULL DEFAULT '[]'
            )"#;

const CREATE_THROTTLE_RUNS: &str = r#"CREATE TABLE IF NOT EXISTS throttle_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL UNIQUE,
                mode TEXT NOT NULL,
                isp TEXT NOT NULL,
                created_at TEXT NOT NULL,
                created_epoch REAL NOT NULL,
                budget_bytes INTEGER NOT NULL DEFAULT 0,
                bytes_used INTEGER NOT NULL DEFAULT 0,
                report_json TEXT NOT NULL
            )"#;

const CREATE_ASN_CACHE: &str = r#"CREATE TABLE IF NOT EXISTS asn_cache (
                ip TEXT PRIMARY KEY,
                asn TEXT NOT NULL,
                country TEXT NOT NULL,
                org TEXT NOT NULL,
                updated_epoch REAL NOT NULL
            )"#;

/// Return `true` when `table` already has a column named `column`.
fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_column(conn: &Connection, table: &str, column: &str, definition: &str) -> rusqlite::Result<()> {
    if !has_column(conn, table, column)? {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {definition}"),
            [],
        )?;
    }
    Ok(())
}

/// Create all ten tables and apply every additive migration.
///
/// This is safe to call on a shipped v4 database: `CREATE TABLE IF NOT EXISTS`
/// plus `ALTER TABLE ... ADD COLUMN` never drop or rewrite existing rows.
pub(crate) fn initialize(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(CREATE_SCHEMA_META, [])?;
    conn.execute(CREATE_DOWNLOADS, [])?;

    conn.execute(CREATE_HOST_STRATEGIES, [])?;
    add_column(
        conn,
        "host_strategies",
        "avg_rtt_ms",
        "avg_rtt_ms REAL NOT NULL DEFAULT 0.0",
    )?;
    add_column(
        conn,
        "host_strategies",
        "packet_loss_pct",
        "packet_loss_pct REAL NOT NULL DEFAULT 0.0",
    )?;

    conn.execute(CREATE_LEARNED_PROFILES, [])?;
    add_column(
        conn,
        "learned_profiles",
        "source",
        "source TEXT NOT NULL DEFAULT 'watchdog'",
    )?;
    add_column(
        conn,
        "learned_profiles",
        "avg_rtt_ms",
        "avg_rtt_ms REAL NOT NULL DEFAULT 0.0",
    )?;
    add_column(
        conn,
        "learned_profiles",
        "packet_loss_pct",
        "packet_loss_pct REAL NOT NULL DEFAULT 0.0",
    )?;

    conn.execute(CREATE_HTTP_CACHE, [])?;
    conn.execute(CREATE_HOST_CAPS, [])?;

    conn.execute(CREATE_QUEUE_JOBS, [])?;
    add_column(
        conn,
        "queue_jobs",
        "category",
        "category TEXT NOT NULL DEFAULT ''",
    )?;

    conn.execute(
        "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION],
    )?;

    // Throttle-owned tables live in the very same file.
    conn.execute(CREATE_THROTTLE_SAMPLES, [])?;
    conn.execute(CREATE_THROTTLE_RUNS, [])?;
    conn.execute(CREATE_ASN_CACHE, [])?;
    Ok(())
}

/// Read-only one-time import of a legacy `history.json` into `downloads`.
pub(crate) fn migrate_legacy_json(
    conn: &Connection,
    legacy_file: &std::path::Path,
) -> rusqlite::Result<()> {
    let migrated: Option<String> = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'json_migrated'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if migrated.as_deref() == Some("1") {
        return Ok(());
    }

    let set_flag = |conn: &Connection| -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('json_migrated', '1')",
            [],
        )?;
        Ok(())
    };

    if !legacy_file.exists() {
        return set_flag(conn);
    }

    let text = match std::fs::read_to_string(legacy_file) {
        Ok(text) => text,
        Err(_) => return set_flag(conn),
    };
    let entries: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return set_flag(conn),
    };
    let Some(entries) = entries.as_array() else {
        return set_flag(conn);
    };

    let now = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mut stmt = conn.prepare(
        "INSERT INTO downloads (timestamp, target, type, filename, path, size, duration, success, adb, sha256)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    for item in entries.iter().rev() {
        let Some(item) = item.as_object() else {
            continue;
        };
        let get_str = |key: &str, default: &str| -> String {
            match item.get(key) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Null) | None => default.to_string(),
                Some(other) => other.to_string(),
            }
        };
        let size = match item.get("size") {
            Some(serde_json::Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|v| v as i64)).unwrap_or(0),
            _ => 0,
        };
        let duration = match item.get("duration") {
            Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            _ => 0.0,
        };
        let truthy = |key: &str, default: bool| -> bool {
            match item.get(key) {
                None => default,
                Some(serde_json::Value::Bool(b)) => *b,
                Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
                Some(serde_json::Value::String(s)) => !s.is_empty(),
                Some(serde_json::Value::Array(v)) => !v.is_empty(),
                Some(serde_json::Value::Object(v)) => !v.is_empty(),
            }
        };
        let sha256 = match item.get("sha256") {
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        stmt.execute(rusqlite::params![
            get_str("timestamp", &now),
            get_str("target", "-"),
            get_str("type", "direct"),
            get_str("filename", "-"),
            get_str("path", "-"),
            size,
            duration,
            i64::from(truthy("success", true)),
            i64::from(truthy("adb", false)),
            sha256,
        ])?;
    }
    drop(stmt);
    set_flag(conn)
}

/// Extension trait for `query_row(...).optional()` without pulling in
/// rusqlite's optional-extension helpers.
trait OptionalExt<T> {
    /// Convert a `QueryReturnedNoRows` error into `None`.
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err),
        }
    }
}
