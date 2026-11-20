//! Thread-safe in-memory task store (`server.py:197-312`).
//!
//! The runner and the HTTP handlers share an [`Arc<TaskStore>`]. Every method
//! takes the lock briefly and returns owned data, so no `MutexGuard` is ever
//! held across an `.await` point in the server.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use chrono::Local;
use paradl_events::py_round;

use crate::models::TaskInfo;

/// `%Y-%m-%d %H:%M:%S` in local time, matching `datetime.now().strftime(...)`.
pub(crate) fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Mutable state backing one API task.
#[derive(Debug, Clone)]
pub struct ApiTask {
    /// API-facing task id (UUID v4 in the pure-Rust implementation).
    pub id: String,
    /// Original download target.
    pub target: String,
    /// Classified target type.
    pub target_type: String,
    /// One of `queued`, `running`, `paused`, `completed`, `failed`.
    pub status: String,
    /// Output directory.
    pub out_dir: String,
    /// Suggested filename.
    pub filename: Option<String>,
    /// Parallel connection count.
    pub connections: i32,
    /// Queue priority.
    pub priority: i32,
    /// Bytes transferred so far.
    pub bytes_downloaded: i64,
    /// Total bytes, when known.
    pub total_bytes: Option<i64>,
    /// Instantaneous speed in bytes/s.
    pub speed_bps: f64,
    /// Final path on disk.
    pub file_path: Option<String>,
    /// Failure text.
    pub error: Option<String>,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Shared stop flag used by pause/delete to signal a running worker.
    pub stop: Arc<AtomicBool>,
}

impl ApiTask {
    /// Project mutable state into the JSON response model.
    ///
    /// `speed_bps` is rounded with Python's banker's rounding to two decimals,
    /// exactly like `_ApiTask.to_api()` in the oracle.
    pub fn to_api(&self) -> TaskInfo {
        TaskInfo {
            id: self.id.clone(),
            target: self.target.clone(),
            target_type: self.target_type.clone(),
            status: self.status.clone(),
            out_dir: self.out_dir.clone(),
            filename: self.filename.clone(),
            connections: self.connections,
            priority: self.priority,
            bytes_downloaded: self.bytes_downloaded,
            total_bytes: self.total_bytes,
            speed_bps: py_round(self.speed_bps, 2),
            file_path: self.file_path.clone(),
            error: self.error.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
        }
    }

    /// Whether pause/delete has requested this task to stop.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Default)]
struct Inner {
    /// Insertion-ordered so stable sorting reproduces Python's `sorted`.
    tasks: Vec<ApiTask>,
}

/// Thread-safe task registry with per-task stop signals.
#[derive(Debug, Default)]
pub struct TaskStore {
    inner: Mutex<Inner>,
}

impl TaskStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Insert or replace a task, giving it a fresh stop signal.
    pub fn create(&self, mut task: ApiTask) {
        task.stop = Arc::new(AtomicBool::new(false));
        let mut inner = self.lock();
        inner.tasks.retain(|existing| existing.id != task.id);
        inner.tasks.push(task);
    }

    /// Look up a task by id.
    pub fn get(&self, task_id: &str) -> Option<ApiTask> {
        self.lock().tasks.iter().find(|task| task.id == task_id).cloned()
    }

    /// All tasks in `created_at DESC` order (stable for equal timestamps).
    pub fn list_all(&self) -> Vec<ApiTask> {
        let mut tasks = self.lock().tasks.clone();
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        tasks
    }

    /// Next runnable task by `priority DESC, created_at ASC`.
    pub fn next_queued(&self) -> Option<ApiTask> {
        let mut queued: Vec<ApiTask> = self
            .lock()
            .tasks
            .iter()
            .filter(|task| task.status == "queued" && !task.is_stopped())
            .cloned()
            .collect();
        queued.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.created_at.cmp(&b.created_at)));
        queued.into_iter().next()
    }

    fn mutate<F: FnOnce(&mut ApiTask)>(&self, task_id: &str, change: F) -> bool {
        let mut inner = self.lock();
        let Some(task) = inner.tasks.iter_mut().find(|task| task.id == task_id) else {
            return false;
        };
        change(task);
        task.updated_at = now_string();
        true
    }

    /// Transition a task status and refresh `updated_at`.
    pub fn set_status(&self, task_id: &str, status: &str) -> bool {
        self.mutate(task_id, |task| task.status = status.into())
    }

    /// Mark a task as running when the runner claims it.
    pub fn mark_running(&self, task_id: &str) -> bool {
        self.set_status(task_id, "running")
    }

    /// Record progress updates produced by a worker.
    pub fn update_progress(&self, task_id: &str, downloaded: i64, total: Option<i64>, speed: f64) -> bool {
        self.mutate(task_id, |task| {
            task.bytes_downloaded = downloaded;
            task.total_bytes = total;
            task.speed_bps = speed;
            task.status = "running".into();
        })
    }

    /// Mark a task completed with its final path and byte count.
    pub fn complete(&self, task_id: &str, file_path: Option<String>, bytes: i64) -> bool {
        self.mutate(task_id, |task| {
            task.status = "completed".into();
            task.file_path = file_path;
            task.bytes_downloaded = bytes;
            task.error = None;
        })
    }

    /// Mark a task failed with an error message.
    pub fn fail(&self, task_id: &str, error: String) -> bool {
        self.mutate(task_id, |task| {
            task.status = "failed".into();
            task.error = Some(error);
        })
    }

    /// Remove a task entirely.
    pub fn delete(&self, task_id: &str) -> bool {
        let mut inner = self.lock();
        let before = inner.tasks.len();
        inner.tasks.retain(|task| task.id != task_id);
        inner.tasks.len() != before
    }

    /// Signal a running worker to stop. Returns `false` for an unknown id.
    pub fn request_stop(&self, task_id: &str) -> bool {
        let Some(task) = self.get(task_id) else {
            return false;
        };
        task.stop.store(true, Ordering::SeqCst);
        true
    }

    /// Clear a previously requested stop (used by resume).
    pub fn clear_stop(&self, task_id: &str) -> bool {
        let Some(task) = self.get(task_id) else {
            return false;
        };
        task.stop.store(false, Ordering::SeqCst);
        true
    }
}
