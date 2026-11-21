//! The runner delegates download work through `CommandExecutor` and honours
//! the concurrency cap, mirroring `tests/integration/test_concurrency.py`.

#![cfg(feature = "api")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use paradl_api::{
    ApiTask, CommandExecutor, DownloadOutcome, ResolveResponse, TaskRunner, TaskStore,
};
use tokio::sync::Notify;

struct GatedExecutor {
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
    gate: Arc<Notify>,
    release: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl CommandExecutor for GatedExecutor {
    async fn resolve(&self, target: &str) -> paradl_api::CommandResult<ResolveResponse> {
        Ok(ResolveResponse {
            target: target.into(),
            target_type: "direct".into(),
            ..ResolveResponse::default()
        })
    }

    async fn download(&self, task: ApiTask) -> paradl_api::CommandResult<DownloadOutcome> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.calls.lock().unwrap().push(task.id.clone());
        while !self.release.load(Ordering::SeqCst) {
            let _ = tokio::time::timeout(Duration::from_millis(50), self.gate.notified()).await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(DownloadOutcome {
            file_path: Some("/tmp/f.bin".into()),
            bytes_downloaded: 42,
        })
    }
}

fn executor(release: bool) -> Arc<GatedExecutor> {
    Arc::new(GatedExecutor {
        active: Arc::new(AtomicUsize::new(0)),
        peak: Arc::new(AtomicUsize::new(0)),
        calls: Arc::new(Mutex::new(Vec::new())),
        gate: Arc::new(Notify::new()),
        release: Arc::new(AtomicBool::new(release)),
    })
}

fn task(id: &str) -> ApiTask {
    let now = "2026-11-21 00:00:00".to_string();
    ApiTask {
        id: id.into(),
        target: "https://example.test/f".into(),
        target_type: "direct".into(),
        status: "queued".into(),
        out_dir: "/tmp".into(),
        filename: None,
        connections: 16,
        priority: 0,
        bytes_downloaded: 0,
        total_bytes: None,
        speed_bps: 0.0,
        file_path: None,
        error: None,
        created_at: now.clone(),
        updated_at: now,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

async fn wait_for(store: &TaskStore, id: &str, status: &str) {
    for _ in 0..80 {
        if store.get(id).is_some_and(|task| task.status == status) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("task {id} never reached {status}");
}

#[tokio::test]
async fn runner_delegates_and_records_outcome() {
    let store = Arc::new(TaskStore::new());
    let exec = executor(true);
    let runner = Arc::new(TaskRunner::new(Arc::clone(&store), exec.clone(), 2)).spawn();
    store.create(task("t1"));

    wait_for(&store, "t1", "completed").await;
    let done = store.get("t1").unwrap();
    assert_eq!(done.file_path.as_deref(), Some("/tmp/f.bin"));
    assert_eq!(done.bytes_downloaded, 42);
    assert_eq!(exec.calls.lock().unwrap().as_slice(), ["t1"]);
    runner.stop().await;
}

#[tokio::test]
async fn runner_never_exceeds_max_concurrent() {
    let store = Arc::new(TaskStore::new());
    let exec = executor(false);
    let runner = Arc::new(TaskRunner::new(Arc::clone(&store), exec.clone(), 1)).spawn();
    for id in ["a", "b", "c"] {
        store.create(task(id));
    }

    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(exec.calls.lock().unwrap().len(), 1, "only one slot reserved");
    assert_eq!(exec.peak.load(Ordering::SeqCst), 1);

    exec.release.store(true, Ordering::SeqCst);
    exec.gate.notify_waiters();
    for id in ["a", "b", "c"] {
        wait_for(&store, id, "completed").await;
    }
    assert_eq!(exec.peak.load(Ordering::SeqCst), 1);
    assert_eq!(exec.calls.lock().unwrap().len(), 3);
    runner.stop().await;
}
