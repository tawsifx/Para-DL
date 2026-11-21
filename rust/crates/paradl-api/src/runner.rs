//! Background task runner and the typed command boundary (W3-3).
//!
//! The runner owns concurrency-slot accounting and status transitions but
//! deliberately contains **no download body**: actual resolve/download work is
//! delegated to a [`CommandExecutor`]. W4-1 supplies the real executor (wired
//! to `paradl-net` / `paradl-engine` / media bridge); [`NoopExecutor`] keeps
//! this crate independently runnable and testable.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;

use crate::models::ResolveResponse;
use crate::store::{ApiTask, TaskStore};

/// Result type used by the command boundary, carrying `paradl_core::Error`.
pub type Result<T> = std::result::Result<T, paradl_core::Error>;

/// Result of executing a queued download command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DownloadOutcome {
    /// Final path of the downloaded artifact, when known.
    pub file_path: Option<String>,
    /// Number of bytes transferred.
    pub bytes_downloaded: i64,
}

/// Typed commands the runner can delegate to the surrounding application.
///
/// This is the seam W4-1 wires; the API crate never implements download logic
/// itself.
#[async_trait]
pub trait CommandExecutor: Send + Sync + 'static {
    /// Classify a target without downloading it (`POST /resolve`).
    async fn resolve(&self, target: &str) -> Result<ResolveResponse>;

    /// Execute a queued task's download command.
    async fn download(&self, task: ApiTask) -> Result<DownloadOutcome>;
}

/// Default executor: classifies everything as `direct` and performs no I/O.
///
/// Used by the standalone example and by tests. W4-1 replaces it with the
/// production executor.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopExecutor;

impl NoopExecutor {
    /// Construct a no-op executor.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CommandExecutor for NoopExecutor {
    async fn resolve(&self, target: &str) -> Result<ResolveResponse> {
        Ok(ResolveResponse {
            target: target.to_string(),
            target_type: "direct".into(),
            ..ResolveResponse::default()
        })
    }

    async fn download(&self, _task: ApiTask) -> Result<DownloadOutcome> {
        Ok(DownloadOutcome::default())
    }
}

/// Pulls queued tasks from the store and runs at most `max_concurrent` at once.
pub struct TaskRunner {
    store: Arc<TaskStore>,
    executor: Arc<dyn CommandExecutor>,
    max_concurrent: usize,
    shutdown: watch::Sender<bool>,
}

impl TaskRunner {
    /// Build a runner with the given concurrency limit (at least one slot).
    pub fn new(
        store: Arc<TaskStore>,
        executor: Arc<dyn CommandExecutor>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            store,
            executor,
            max_concurrent: max_concurrent.max(1),
            shutdown: watch::channel(false).0,
        }
    }

    /// Effective maximum number of concurrent workers.
    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// Spawn the runner loop and return a handle used to stop it.
    pub fn spawn(self: Arc<Self>) -> RunnerHandle {
        let mut shutdown_rx = self.shutdown.subscribe();
        let store = Arc::clone(&self.store);
        let executor = Arc::clone(&self.executor);
        let permits = Arc::new(Semaphore::new(self.max_concurrent));

        let task = tokio::spawn(async move {
            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                let permit = tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    result = Arc::clone(&permits).acquire_owned() => match result {
                        Ok(permit) => permit,
                        Err(_) => break,
                    },
                };

                let Some(next) = store.next_queued() else {
                    drop(permit);
                    tokio::select! {
                        _ = shutdown_rx.changed() => break,
                        _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                    }
                    continue;
                };

                if !store.mark_running(&next.id) || next.is_stopped() {
                    store.set_status(&next.id, "paused");
                    drop(permit);
                    continue;
                }

                let worker_store = Arc::clone(&store);
                let worker_executor = Arc::clone(&executor);
                tokio::spawn(async move {
                    let task_id = next.id.clone();
                    match worker_executor.download(next).await {
                        Ok(outcome) => {
                            if worker_store.get(&task_id).is_some_and(|task| task.is_stopped()) {
                                worker_store.set_status(&task_id, "paused");
                            } else {
                                worker_store.complete(
                                    &task_id,
                                    outcome.file_path,
                                    outcome.bytes_downloaded,
                                );
                            }
                        }
                        Err(error) => {
                            worker_store.fail(&task_id, error.to_string());
                        }
                    }
                    drop(permit);
                });
            }
        });

        RunnerHandle { task: Some(task), shutdown: self.shutdown.clone() }
    }
}

/// Handle used to stop a spawned [`TaskRunner`].
pub struct RunnerHandle {
    task: Option<JoinHandle<()>>,
    shutdown: watch::Sender<bool>,
}

impl RunnerHandle {
    /// Signal shutdown and wait for the runner loop to finish.
    pub async fn stop(mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for RunnerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
