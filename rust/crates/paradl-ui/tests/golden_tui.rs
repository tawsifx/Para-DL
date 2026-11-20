//! Golden tests for the feature-gated TUI model, captured from the READ-ONLY
//! Python oracle `src/paradl/ui/tui.py` (`_TaskState`, `STATUS_LABELS`).

#![cfg(feature = "tui")]

use paradl_events::{
    Event, ProgressEvent, QueueJobRow, QueueJobStartedEvent, QueueListEvent, ResultEvent,
};
use paradl_ui::tui::{TaskState, TaskStore, apply_event, status_label};

fn sample_task() -> TaskState {
    TaskState {
        id: 7,
        target: "https://example.test/big-file.zip".into(),
        target_type: "direct".into(),
        category: "software".into(),
        out_dir: "/tmp".into(),
        filename: Some("big-file.zip".into()),
        status: "running".into(),
        connections: 16,
        priority: 5,
        bytes_downloaded: 512,
        total_bytes: Some(2048),
        speed_bps: 0.0,
        error: None,
        file_path: None,
        created_at: "2026-11-20 10:00:00".into(),
        updated_at: "2026-11-20 10:01:00".into(),
    }
}

#[test]
fn task_state_progress_pct_and_bar_match_oracle() {
    let task = sample_task();
    assert_eq!(task.progress_pct(), 25.0);
    assert_eq!(task.progress_bar(24), "██████░░░░░░░░░░░░░░░░░░");
    assert_eq!(task.progress_bar(10), "██░░░░░░░░");
}

#[test]
fn task_state_status_labels_match_oracle() {
    for status in ["queued", "running", "paused", "completed", "failed"] {
        assert_eq!(status_label(status), status);
        assert_eq!(sample_task().status_text(), "running");
    }
    let mut odd = sample_task();
    odd.status = "weird".into();
    assert_eq!(odd.status_text(), "weird");
}

#[test]
fn task_state_row_without_speed_matches_oracle() {
    let row = sample_task().to_row();
    assert_eq!(
        row,
        [
            "7",
            "big-file.zip",
            "software",
            "512.0 B / 2.0 KB (25.0%)",
            "running",
            "—",
            "16",
        ]
    );
}

#[test]
fn task_state_row_with_speed_and_no_filename_matches_oracle() {
    let mut task = sample_task();
    task.id = 1;
    task.target = "https://example.test/archive.tar.gz".into();
    task.filename = None;
    task.category.clear();
    task.status = "completed".into();
    task.connections = 8;
    task.bytes_downloaded = 1_048_576;
    task.total_bytes = Some(2_097_152);
    task.speed_bps = 1536.0;
    assert_eq!(
        task.to_row(),
        [
            "1",
            "https://example.test/archive.tar.gz",
            "—",
            "1.0 MB / 2.0 MB (50.0%)",
            "completed",
            "1.5 KB/s",
            "8",
        ]
    );
}

#[test]
fn task_state_zero_total_matches_oracle() {
    let mut task = sample_task();
    task.bytes_downloaded = 0;
    task.total_bytes = None;
    assert_eq!(task.progress_pct(), 0.0);
    assert_eq!(task.progress_bar(24), "░".repeat(24));
    assert_eq!(task.to_row()[3], "0 B");
}

#[test]
fn typed_events_populate_task_store_and_render_oracle_row() {
    let mut store = TaskStore::new();
    let row = QueueJobRow::new(
        7,
        "https://example.test/big-file.zip".into(),
        "queued".into(),
        5,
        16,
        true,
        0.0,
        None,
        0,
        Some(2048),
        "/tmp".into(),
        Some("big-file.zip".into()),
        None,
    );
    apply_event(
        &mut store,
        &Event::QueueList(QueueListEvent::new(1_763_376_000.123, vec![row])),
    );
    apply_event(
        &mut store,
        &Event::QueueJobStarted(QueueJobStartedEvent::new(
            1_763_376_000.123,
            7,
            "https://example.test/big-file.zip".into(),
            5,
            16,
        )),
    );
    apply_event(
        &mut store,
        &Event::Progress(ProgressEvent::new(
            1_763_376_000.123,
            "big-file.zip".into(),
            512,
            Some(2048),
            Some(25.0),
            Some(0.0),
            Some(0.0),
            Some(16),
            None,
            Some("Active".into()),
            None,
            None,
        )),
    );

    let task = store.get(7).expect("job 7 should be in the store");
    assert_eq!(task.status, "running");
    assert_eq!(
        task.to_row(),
        [
            "7",
            "big-file.zip",
            "—",
            "512.0 B / 2.0 KB (25.0%)",
            "running",
            "—",
            "16",
        ]
    );
}

#[test]
fn result_event_marks_task_completed() {
    let mut store = TaskStore::new();
    store.upsert(sample_task());
    apply_event(
        &mut store,
        &Event::Result(ResultEvent::new(
            1_763_376_000.123,
            None,
            Some("https://example.test/big-file.zip".into()),
            true,
            Some("/tmp/big-file.zip".into()),
            None,
            None,
            0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )),
    );
    let task = store.get(7).expect("job 7 should remain");
    assert_eq!(task.status, "completed");
    assert_eq!(task.file_path.as_deref(), Some("/tmp/big-file.zip"));
}
