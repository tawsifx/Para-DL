//! `Store` implementation over a bundled SQLite connection.

use crate::{
    Error, Queue, Result, TransferRecord, guard::InitGuard, map_err, now_str, schema,
};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
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
    /// One connection per store. `Connection` is `Send` but not `Sync`, so the
    /// shared handle is mutex-wrapped; `Arc` lets queues share the same guard.
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    /// Open (and, if needed, create and migrate) the default `history.db`.
    pub fn open() -> Result<Self> {
        Self::open_at(paradl_core::history_db_file())
    }

    /// Open (and, if needed, create and migrate) a database at `path`.
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        // The managed connection must exist before `open_connection(&self)` can
        // run; start from an in-memory placeholder and replace it with the
        // single, initialized connection that every later operation reuses.
        let store = Self {
            path: path.as_ref().to_path_buf(),
            history_limit: 50,
            conn: Arc::new(Mutex::new(
                Connection::open_in_memory().map_err(map_err)?,
            )),
        };
        // Force schema initialization (and legacy migration) before returning,
        // then retain that one connection for all subsequent operations.
        let conn = store.open_connection()?;
        *store.lock_conn() = conn;
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

    /// Lock the shared connection, recovering from a poisoned mutex rather than
    /// failing the operation (a panicking holder cannot corrupt SQLite state).
    fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run a closure against the store's single managed connection.
    ///
    /// The connection has WAL + `synchronous=NORMAL` applied and the schema
    /// guard checked at open time, exactly like every operation on the store.
    pub fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T> {
        let conn = self.lock_conn();
        f(&conn).map_err(map_err)
    }

    /// Open the one managed connection, rotating a corrupt DB to `.bad` on a
    /// SQLite failure and retrying against a rebuilt file.
    fn open_connection(&self) -> Result<Connection> {
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
                let rebuilt = self.open_and_init().map_err(|retry| {
                    Error::failure(format!("sqlite after rotating corrupt DB ({err}): {retry}"))
                })?;
                // The original connect-then-close flow checkpointed and removed
                // the WAL sidecars after recovery. A store now keeps its
                // connection open, so checkpoint explicitly: switching to DELETE
                // drops -wal/-shm, and switching back restores WAL mode (stored
                // in the file header) for this connection without new sidecars.
                rebuilt
                    .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA journal_mode=WAL;")
                    .map_err(map_err)?;
                Ok(rebuilt)
            }
            Err(err) => Err(map_err(err)),
        }
    }

    /// Open the database once: apply `busy_timeout` on every open, apply the
    /// WAL pragmas at initialization, and run the schema guard / migration.
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

/// Python-compatible `round(value, ndigits)` (round-half-to-even), matching
/// `HistoryManager.record`'s `round(float(duration), 2)` (`history.py:444`).
///
/// This mirrors the public `paradl_watchdog::types::py_round` implementation,
/// kept local because `paradl-store` is a foundational crate: depending on the
/// higher-level watchdog would invert the layering (the watchdog's SQLite
/// profile store needs `paradl-store`) and create a dependency cycle.
fn round_ties_even(value: f64, ndigits: i32) -> f64 {
    let factor = 10_f64.powi(ndigits);
    (value * factor).round_ties_even() / factor
}

impl crate::Store for SqliteStore {
    fn record_transfer(&self, record: &TransferRecord) -> Result<()> {
        let conn = self.lock_conn();
        let duration = round_ties_even(record.duration, 2);
        conn.prepare_cached(
            "INSERT INTO downloads (timestamp, target, type, filename, path, size, duration, success, adb, sha256)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .map_err(map_err)?
        .execute(params![
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
        ])
        .map_err(map_err)?;
        let cap = i64::try_from(self.history_limit.max(1)).unwrap_or(i64::MAX);
        conn.prepare_cached(
            "DELETE FROM downloads WHERE id NOT IN (
                SELECT id FROM downloads ORDER BY id DESC LIMIT ?
             )",
        )
        .map_err(map_err)?
        .execute([cap])
        .map_err(map_err)?;
        Ok(())
    }

    fn recent(&self, limit: usize) -> Result<Vec<TransferRecord>> {
        let conn = self.lock_conn();
        let cap = i64::try_from(limit.max(1)).unwrap_or(i64::MAX);
        let mut stmt = conn
            .prepare_cached("SELECT * FROM downloads ORDER BY id DESC LIMIT ?")
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
        Ok(Queue::new(self.conn.clone()))
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

    #[test]
    fn with_connection_reuses_one_connection() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
        let ptr = { let g = store.conn.lock().unwrap(); &*g as *const Connection };
        store.recent(5).unwrap();
        store.with_connection(|c| c.execute_batch("SELECT 1")).unwrap();
        let again = { let g = store.conn.lock().unwrap(); &*g as *const Connection };
        assert_eq!(ptr, again, "operations must reuse one Connection, not reconnect");
    }

    #[test]
    fn reused_connection_is_in_wal_mode() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
        let mode: String = store
            .with_connection(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn queue_add_then_status_update_completes() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
        let queue = store.open_queue().unwrap(); // must not deadlock with store's mutex
        let job = queue.add_job(&crate::QueueJob::new("https://e.test/f", "/tmp")).unwrap();
        queue
            .update_job_status(job.id, "running", None, None, None)
            .unwrap();
        assert_eq!(queue.get_job(job.id).unwrap().unwrap().status, "running");
    }
}
