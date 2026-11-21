//! Golden output tests captured from the READ-ONLY Python oracle:
//! `src/paradl/ui/display.py` (`format_size`, `format_speed`,
//! `LiveTransferDisplay._format_simple_line`, `_build_renderable`) and
//! `src/paradl/core/models.py`.
//!
//! Every expected string below was produced by executing the oracle with the
//! same inputs; these are exact-string assertions, not approximations.

use paradl_events::{Event, ProgressEvent, ResultEvent};
use paradl_ui::{ProgressDisplay, event_summary, format_size, format_speed};
use serde_json::json;

fn progress(
    title: &str,
    downloaded: i64,
    total: Option<i64>,
    speed: Option<f64>,
    connections: Option<i32>,
) -> ProgressEvent {
    ProgressEvent::new(
        1_763_376_000.123,
        title.to_string(),
        downloaded,
        total,
        Some(0.0),
        speed,
        Some(0.0),
        connections,
        None,
        None,
        None,
        None,
    )
}

#[test]
fn format_size_matches_python_oracle() {
    let cases: &[(Option<f64>, &str)] = &[
        (None, "Unknown size"),
        (Some(-5.0), "0 B"),
        (Some(0.0), "0 B"),
        (Some(1.0), "1.0 B"),
        (Some(1023.0), "1023.0 B"),
        (Some(1024.0), "1.0 KB"),
        (Some(1536.0), "1.5 KB"),
        (Some(1_048_576.0), "1.0 MB"),
        (Some(1_073_741_824.0), "1.0 GB"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_size(*input), *expected, "format_size({input:?})");
    }
}

#[test]
fn format_speed_matches_python_oracle() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0 B/s"),
        (12.4, "12 B/s"),
        (1024.0, "1.0 KB/s"),
        (1536.0, "1.5 KB/s"),
        (1_048_576.0, "1.0 MB/s"),
        (2_621_440.0, "2.5 MB/s"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_speed(*input), *expected, "format_speed({input})");
    }
}

#[test]
fn simple_line_typical() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 1024, Some(2048), Some(1000.0), None);
    assert_eq!(
        display.simple_line(&event),
        "  demo.bin   50.0%  1.0 KB / 2.0 KB  1000 B/s  ETA 00:01"
    );
}

#[test]
fn simple_line_indeterminate() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 512, None, Some(0.0), None);
    assert_eq!(
        display.simple_line(&event),
        "  demo.bin   ---.-%  512.0 B  0 B/s  ETA --:--"
    );
}

#[test]
fn simple_line_with_connections() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 512, Some(2048), Some(2048.0), Some(16));
    assert_eq!(
        display.simple_line(&event),
        "  demo.bin   25.0%  512.0 B / 2.0 KB  2.0 KB/s  ETA 00:00  [16 conns]"
    );
}

#[test]
fn simple_line_truncates_long_title_to_32_chars() {
    let display = ProgressDisplay::new();
    let event = progress(
        "a-very-long-download-filename-that-exceeds-32-characters.zip",
        1024,
        Some(2048),
        Some(500_000.0),
        None,
    );
    assert_eq!(
        display.simple_line(&event),
        "  a-very-long-download-filename-th   50.0%  1.0 KB / 2.0 KB  488.3 KB/s  ETA 00:00"
    );
}

#[test]
fn simple_line_partial() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 1536, Some(4096), Some(512.0), None);
    assert_eq!(
        display.simple_line(&event),
        "  demo.bin   37.5%  1.5 KB / 4.0 KB  512 B/s  ETA 00:05"
    );
}

#[test]
fn renderable_typical() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 1024, Some(2048), Some(1000.0), None);
    assert_eq!(
        display.renderable(&event),
        "  demo.bin [████████████░░░░░░░░░░░░]  50.0%  1.0 KB / 2.0 KB  1000 B/s  ETA 00:01  "
    );
}

#[test]
fn renderable_watchdog_state() {
    let display = ProgressDisplay::new();
    let mut event = progress("demo.bin", 1024, Some(2048), Some(1000.0), None);
    event.watchdog_state = Some("Tier2 Pause".into());
    assert_eq!(
        display.renderable(&event),
        "  demo.bin [████████████░░░░░░░░░░░░]  50.0%  1.0 KB / 2.0 KB  1000 B/s  ETA 00:01  [Tier2 Pause]"
    );
}

#[test]
fn renderable_connection_and_peer_stats() {
    let display = ProgressDisplay::new();
    let mut event = progress("demo.bin", 1024, Some(2048), Some(1000.0), Some(8));
    event.peer_count = Some(3);
    event.connection_speeds = Some(json!([1000.0, 2000.0]));
    assert_eq!(
        display.renderable(&event),
        "  demo.bin [████████████░░░░░░░░░░░░]  50.0%  1.0 KB / 2.0 KB  1000 B/s  ETA 00:01  [8 conns | 3 peers | 1.5 KB/s/conn]  "
    );
}

#[test]
fn renderable_per_file_progress() {
    let display = ProgressDisplay::new();
    let mut event = progress("demo.bin", 768, Some(3072), Some(1000.0), None);
    event.file_progress = Some(json!([
        {"name": "part-one.bin", "downloaded": 512, "total": 1024},
        {"name": "part-two.bin", "downloaded": 256, "total": 2048},
    ]));
    assert_eq!(
        display.renderable(&event),
        concat!(
            "  demo.bin [██████░░░░░░░░░░░░░░░░░░]  25.0%  768.0 B / 3.0 KB  1000 B/s  ETA 00:02  \n",
            "    part-one.bin                 [████████░░░░░░░░]  50.0%  512.0 B/1.0 KB\n",
            "    part-two.bin                 [██░░░░░░░░░░░░░░]  12.5%  256.0 B/2.0 KB\n",
        )
    );
}

#[test]
fn renderable_indeterminate_uses_injected_time() {
    let display = ProgressDisplay::new();
    let event = progress("demo.bin", 512, None, Some(0.0), None);
    assert_eq!(
        display.renderable_at(&event, 1000.0),
        "  demo.bin [░░░░░░░░████░░░░░░░░░░░░]  ---.-%  512.0 B  0 B/s  ETA --:--  "
    );
}

#[test]
fn event_summary_delegates_progress_to_plain_display() {
    let event = Event::Progress(progress("demo.bin", 1024, Some(2048), Some(1000.0), None));
    assert_eq!(
        event_summary(&event),
        "  demo.bin   50.0%  1.0 KB / 2.0 KB  1000 B/s  ETA 00:01"
    );
}

#[test]
fn event_summary_never_emits_jsonl() {
    let event = Event::Result(ResultEvent::new(
        1_763_376_000.123,
        Some("download".into()),
        Some("https://example.test/f.bin".into()),
        true,
        Some("/tmp/f.bin".into()),
        Some(true),
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
    ));
    let summary = event_summary(&event);
    assert!(!summary.starts_with('{'));
    assert!(!summary.contains("\"timestamp\""));
    assert!(summary.contains("result"));
    assert!(summary.contains("https://example.test/f.bin"));
}
