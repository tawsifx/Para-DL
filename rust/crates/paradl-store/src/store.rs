//! `Store` implementation over a bundled SQLite connection.

use crate::{
    Error, Queue, Result, TransferRecord, guard::InitGuard, map_err, now_str, schema,
};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Number of times the full schema initialization has run in this process.
/// Exposed to unit tests so the device+inode guard can be asserted directly.
pub(crate) static SCHEMA_INIT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// SQLite-backed implementation of [`crate::Store`].
///
/// The default constructor opens `paradl_core::history_db_file()` (i.e.
/// `$PARADL_HOME/history.db`); [`SqliteStore::open_at`] accepts an explicit path
/// for tests and embedding. Every table lives in that one file. All ten tables
/// are created/migrated on first open, guarded process-wide by resolved path
/// plus device and inode.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    path: PathBuf,
    history_limit: usize,
}

impl SqliteStore {
    /// Open (and, if needed, create and migrate) the default `history.db`.
    pub fn open() -> Result<Self> {
        Self::open_at(paradl_core::history_db_file())
    }

    /// Open (and, if needed, create and migrate) a database at `path`.
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let store = Self {
            path: path.as_ref().to_path_buf(),
            history_limit: 50,
        };
        // Force schema initialization (and legacy migration) before returning.
        drop(store.connect()?);
        Ok(store)
    }

    /// Set the maximum number of transfer-history rows retained on write
    /// (the oracle's `history_limit`, default `50`; values below 1 become 1).
    pub fn with_history_limit(mut self, limit: usize) -> Self {
        self.history_limit = limit;
        self
    }

    /// Return the resolved path of the backing database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run a closure against a fresh managed connection.
    ///
    /// The connection has WAL + `synchronous=NORMAL` applied and the schema
    /// guard checked, exactly like every other connection opened by the store.
    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T> {
        let conn = self.connect()?;
        f(&conn).map_err(map_err)
    }

    /// Apply WAL pragmas, the schema guard and (once) legacy JSON migration.
    fn connect(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        match self.open_and_init() {
            Ok(conn) => Ok(conn),
            Err(err @ rusqlite::Error::SqliteFailure(..)) => {
                // Port of HistoryManager._connect: rotate a corrupt DB to
                // `.bad`, drop stale WAL sidecars, and rebuild from scratch.
                let bad = PathBuf::from(format!("{}.bad", self.path.display()));
                if self.path.exists() {
                    std::fs::rename(&self.path, &bad)?;
                }
                for suffix in ["-wal", "-shm"] {
                    let side = PathBuf::from(format!("{}{suffix}", self.path.display()));
                    if side.exists() {
                        std::fs::remove_file(side)?;
                    }
                }
                self.open_and_init().map_err(|retry| {
                    Error::failure(format!("sqlite after rotating corrupt DB ({err}): {retry}"))
                })
            }
            Err(err) => Err(map_err(err)),
        }
    }

    fn open_and_init(&self) -> rusqlite::Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        let guard = InitGuard::new(&self.path);
        if !guard.already_initialized() {
            schema::initialize(&conn)?;
            SCHEMA_INIT_COUNT.fetch_add(1, Ordering::SeqCst);
            guard.mark_initialized();
            let legacy = self.path.with_file_name("history.json");
            schema::migrate_legacy_json(&conn, &legacy)?;
        }
        Ok(conn)
    }
}

impl crate::Store for SqliteStore {
    fn record_transfer(&self, record: &TransferRecord) -> Result<()> {
        let conn = self.connect()?;
        let duration = (record.duration * 100.0).round() / 100.0;
        conn.execute(
            "INSERT INTO downloads (timestamp, target, type, filename, path, size, duration, success, adb, sha256)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                now_str(),
                record.target,
                record.target_type,
                record.filename,
                record.path,
                record.size,
                duration,
                i64::from(record.success),
                i64::from(record.adb),
                record.sha256,
            ],
        )
        .map_err(map_err)?;
        let cap = i64::try_from(self.history_limit.max(1)).unwrap_or(i64::MAX);
        conn.execute(
            "DELETE FROM downloads WHERE id NOT IN (
                SELECT id FROM downloads ORDER BY id DESC LIMIT ?
             )",
            [cap],
        )
        .map_err(map_err)?;
        Ok(())
    }

    fn recent(&self, limit: usize) -> Result<Vec<TransferRecord>> {
        let conn = self.connect()?;
        let cap = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let mut stmt = conn
            .prepare("SELECT * FROM downloads ORDER BY id DESC LIMIT ?")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([cap], |row| {
                Ok(TransferRecord {
                    id: row.get("id")?,
                    timestamp: row.get("timestamp")?,
                    target: row.get("target")?,
                    target_type: row.get("type")?,
                    filename: row.get("filename")?,
                    path: row.get("path")?,
                    size: row.get("size")?,
                    duration: row.get("duration")?,
                    success: row.get::<_, i64>("success")? != 0,
                    adb: row.get::<_, i64>("adb")? != 0,
                    sha256: row.get("sha256")?,
                })
            })
            .map_err(map_err)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(map_err)?);
        }
        Ok(records)
    }

    fn open_queue(&self) -> Result<Queue> {
        Ok(Queue::new(self.connect()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn guard_initializes_once_per_physical_file() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("history.db");
        let before = SCHEMA_INIT_COUNT.load(Ordering::SeqCst);

        for _ in 0..5 {
            let store = SqliteStore::open_at(&db).expect("open");
            drop(store.open_queue().expect("queue"));
        }
        assert_eq!(SCHEMA_INIT_COUNT.load(Ordering::SeqCst) - before, 1);

        // Deleting and recreating the path changes device+inode, so the guard
        // must allow (and require) a fresh initialization.
        std::fs::remove_file(&db).expect("delete db");
        for suffix in ["-wal", "-shm"] {
            let side = PathBuf::from(format!("{}{suffix}", db.display()));
            if side.exists() {
                std::fs::remove_file(side).expect("remove sidecar");
            }
        }
        let store = SqliteStore::open_at(&db).expect("reopen after delete");
        drop(store);
        assert_eq!(SCHEMA_INIT_COUNT.load(Ordering::SeqCst) - before, 2);
    }

    #[test]
    fn batch_of_writes_uses_one_initialized_connection() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp dir");
        let db = dir.path().join("history.db");
        let store = SqliteStore::open_at(&db).expect("open");
        let before = SCHEMA_INIT_COUNT.load(Ordering::SeqCst);

        store
            .with_connection(|conn| {
                for i in 0..3 {
                    conn.execute(
                        "INSERT INTO learned_profiles (host, hour_bucket, network_id, updated_at) VALUES (?1, 0, 'net', ?2)",
                        params![format!("host{i}"), now_str()],
                    )?;
                }
                Ok(())
            })
            .expect("batch");
        assert_eq!(SCHEMA_INIT_COUNT.load(Ordering::SeqCst), before);
        let count: i64 = store
            .with_connection(|c| c.query_row("SELECT COUNT(*) FROM learned_profiles", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(count, 3);
    }
}
