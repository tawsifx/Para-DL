//! Handle for the shared `queue_jobs` table.

use crate::{QueueJob, now_str};
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::map_err;

/// A persistent download queue backed by the shared `history.db`.
///
/// Obtained from [`crate::Store::open_queue`]. Each operation mirrors the
/// SQL of `QueueManager` in `src/paradl/resilience/queue.py` byte-for-byte.
/// The queue shares the store's single `Mutex<Connection>` so operations from
/// both handles serialize against the same connection.
#[derive(Debug)]
pub struct Queue {
    conn: Arc<Mutex<Connection>>,
}

impl Queue {
    pub(crate) fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Lock the shared connection, recovering from a poisoned mutex rather than
    /// failing the operation.
    fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Insert a job and return the stored row (with `id` and timestamps set).
    ///
    /// Connections are clamped to `1..=16`; `mirrors_json` and `options_json`
    /// are compact JSON (`[]` / `{}` for empty values), matching the oracle.
    pub fn add_job(&self, job: &QueueJob) -> crate::Result<QueueJob> {
        let now = now_str();
        let connections = job.connections.clamp(1, 16);
        let mirrors_json = serde_json::to_string(&job.mirrors).map_err(|e| {
            crate::Error::failure(format!("failed to serialize queue mirrors: {e}"))
        })?;
        let options_json = serde_json::to_string(&job.options).map_err(|e| {
            crate::Error::failure(format!("failed to serialize queue options: {e}"))
        })?;
        let id = {
            let conn = self.lock_conn();
            conn.prepare_cached(
                "INSERT INTO queue_jobs (
                    target, target_type, out_dir, filename, mirrors_json,
                    priority, connections, status, require_ac, pause_on_low_power,
                    when_fast_bps, start_at, expected_length, expected_digest,
                    category, bytes_downloaded, error, options_json, created_at, updated_at
                 )
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, 0, NULL, ?, ?, ?)",
            )
            .map_err(map_err)?
            .execute(params![
                job.target.trim(),
                job.target_type,
                job.out_dir,
                job.filename,
                mirrors_json,
                job.priority,
                connections,
                i64::from(job.require_ac),
                i64::from(job.pause_on_low_power),
                job.when_fast_bps,
                job.start_at,
                job.expected_length,
                job.expected_digest,
                job.category,
                options_json,
                now,
                now,
            ])
            .map_err(map_err)?;
            conn.last_insert_rowid()
        }; // guard drops here, before get_job locks the same mutex
        self.get_job(id)?
            .ok_or_else(|| crate::Error::failure("inserted queue job disappeared"))
    }

    /// Return a job by id, or `None` when it does not exist.
    pub fn get_job(&self, id: i64) -> crate::Result<Option<QueueJob>> {
        let conn = self.lock_conn();
        let mut stmt = conn
            .prepare_cached("SELECT * FROM queue_jobs WHERE id = ?1")
            .map_err(map_err)?;
        stmt.query_row([id], row_to_job)
            .optional()
            .map_err(map_err)
    }

    /// List jobs by `priority DESC, id ASC`, optionally filtered by status.
    pub fn list_jobs(&self, status: Option<&str>) -> crate::Result<Vec<QueueJob>> {
        let sql = match status {
            Some(_) => "SELECT * FROM queue_jobs WHERE status = ?1 ORDER BY priority DESC, id ASC",
            None => "SELECT * FROM queue_jobs ORDER BY priority DESC, id ASC",
        };
        let conn = self.lock_conn();
        let mut stmt = conn.prepare_cached(sql).map_err(map_err)?;
        let rows = match status {
            Some(value) => stmt.query_map([value], row_to_job),
            None => stmt.query_map([], row_to_job),
        }
        .map_err(map_err)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(row.map_err(map_err)?);
        }
        Ok(jobs)
    }

    /// Update a job's status, preserving existing byte counts and filename when
    /// the corresponding argument is `None`.
    pub fn update_job_status(
        &self,
        id: i64,
        status: &str,
        bytes_downloaded: Option<i64>,
        error: Option<&str>,
        filename: Option<&str>,
    ) -> crate::Result<()> {
        let Some(existing) = self.get_job(id)? else {
            return Ok(());
        };
        let bytes = bytes_downloaded.unwrap_or(existing.bytes_downloaded);
        let fname = filename.map(str::to_string).or(existing.filename);
        // get_job above already released its guard; lock only after it returns
        // so the two statement-scoped locks never overlap.
        let conn = self.lock_conn();
        conn.prepare_cached(
            "UPDATE queue_jobs SET status = ?, bytes_downloaded = ?, error = ?, filename = ?, updated_at = ? WHERE id = ?",
        )
        .map_err(map_err)?
        .execute(params![status, bytes, error, fname, now_str(), id])
        .map_err(map_err)?;
        Ok(())
    }

    /// Remove jobs by id, or every job when `remove_all` is true. Returns the
    /// number of deleted rows.
    pub fn remove_jobs(&self, ids: &[i64], remove_all: bool) -> crate::Result<usize> {
        let conn = self.lock_conn();
        let changed = if remove_all {
            conn.prepare_cached("DELETE FROM queue_jobs")
                .map_err(map_err)?
                .execute([])
                .map_err(map_err)?
        } else if ids.is_empty() {
            return Ok(0);
        } else {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!("DELETE FROM queue_jobs WHERE id IN ({placeholders})");
            let params: Vec<i64> = ids.to_vec();
            conn.execute(&sql, rusqlite::params_from_iter(params)).map_err(map_err)?
        };
        Ok(changed)
    }

    /// Pause unfinished jobs. Returns the number of changed rows.
    pub fn pause_jobs(&self, ids: &[i64], pause_all: bool) -> crate::Result<usize> {
        let now = now_str();
        let conn = self.lock_conn();
        let changed = if pause_all {
            conn.prepare_cached(
                "UPDATE queue_jobs SET status = 'paused', updated_at = ? WHERE status IN ('queued', 'running')",
            )
            .map_err(map_err)?
            .execute([now])
            .map_err(map_err)?
        } else if ids.is_empty() {
            return Ok(0);
        } else {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "UPDATE queue_jobs SET status = 'paused', updated_at = ? WHERE id IN ({placeholders}) AND status != 'completed'"
            );
            let mut params: Vec<String> = vec![now];
            params.extend(ids.iter().map(ToString::to_string));
            conn.execute(&sql, rusqlite::params_from_iter(params))
                .map_err(map_err)?
        };
        Ok(changed)
    }

    /// Re-queue paused, crashed or failed jobs. Returns the number of changed rows.
    pub fn resume_jobs(&self, ids: &[i64], resume_all: bool) -> crate::Result<usize> {
        let now = now_str();
        let conn = self.lock_conn();
        let changed = if resume_all {
            conn.prepare_cached(
                "UPDATE queue_jobs SET status = 'queued', error = NULL, updated_at = ? WHERE status IN ('paused', 'running', 'failed')",
            )
            .map_err(map_err)?
            .execute([now])
            .map_err(map_err)?
        } else if ids.is_empty() {
            return Ok(0);
        } else {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!(
                "UPDATE queue_jobs SET status = 'queued', error = NULL, updated_at = ? WHERE id IN ({placeholders}) AND status != 'completed'"
            );
            let mut params: Vec<String> = vec![now];
            params.extend(ids.iter().map(ToString::to_string));
            conn.execute(&sql, rusqlite::params_from_iter(params))
                .map_err(map_err)?
        };
        Ok(changed)
    }

    /// Transition jobs left in `running` after a crash back to `queued`.
    pub fn recover_crashed_jobs(&self) -> crate::Result<usize> {
        let conn = self.lock_conn();
        let changed = conn
            .prepare_cached(
                "UPDATE queue_jobs SET status = 'queued', updated_at = ? WHERE status = 'running'",
            )
            .map_err(map_err)?
            .execute([now_str()])
            .map_err(map_err)?;
        Ok(changed)
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueueJob> {
    let get_opt = |index: &str| -> rusqlite::Result<Option<String>> { row.get(index) };
    let mirrors: Vec<String> = match get_opt("mirrors_json")? {
        Some(text) => serde_json::from_str(&text).unwrap_or_default(),
        None => Vec::new(),
    };
    let options: serde_json::Value = match get_opt("options_json")? {
        Some(text) => serde_json::from_str(&text).unwrap_or_default(),
        None => serde_json::Value::Null,
    };
    Ok(QueueJob {
        id: row.get("id")?,
        target: row.get("target")?,
        target_type: row.get::<_, Option<String>>("target_type")?.unwrap_or_else(|| "direct".into()),
        out_dir: row.get("out_dir")?,
        filename: get_opt("filename")?,
        mirrors,
        priority: row.get("priority")?,
        connections: row.get("connections")?,
        status: row.get::<_, Option<String>>("status")?.unwrap_or_else(|| "queued".into()),
        require_ac: row.get::<_, i64>("require_ac")? != 0,
        pause_on_low_power: row.get::<_, i64>("pause_on_low_power")? != 0,
        when_fast_bps: row.get("when_fast_bps")?,
        start_at: get_opt("start_at")?,
        expected_length: row.get("expected_length")?,
        expected_digest: get_opt("expected_digest")?,
        category: row.get::<_, Option<String>>("category")?.unwrap_or_default(),
        bytes_downloaded: row.get("bytes_downloaded")?,
        error: get_opt("error")?,
        options,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}
