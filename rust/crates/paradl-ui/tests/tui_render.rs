//! Buffer-level render tests for the ratatui TUI (no TTY required).

#![cfg(feature = "tui")]

use paradl_events::{Event, QueueJobRow, QueueListEvent};
use paradl_ui::tui::{TuiApp, render_app};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    let cells = buffer.content();
    let mut text = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let index = y as usize * area.width as usize + x as usize;
            text.push_str(cells[index].symbol());
        }
        text.push('\n');
    }
    text
}

fn render(rows: Vec<QueueJobRow>) -> String {
    let mut app = TuiApp::new();
    app.apply_event(&Event::QueueList(QueueListEvent::new(
        1_763_376_000.123,
        rows,
    )));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("test backend");
    terminal
        .draw(|frame| render_app(&app, frame))
        .expect("draw");
    buffer_text(terminal.backend().buffer())
}

#[test]
fn renders_header_filters_empty_state_and_status_bar() {
    let text = render(Vec::new());
    assert!(text.contains("pdl"), "{text}");
    assert!(text.contains("Parallel Download Manager"), "{text}");
    assert!(text.contains("No downloads yet"), "{text}");
    assert!(
        text.contains("Press 'a' to add your first download"),
        "{text}"
    );
    assert!(text.contains("No tasks  a:add  p:pause"), "{text}");
    assert!(text.contains("Downloads"), "{text}");
}

#[test]
fn renders_task_row_and_counts_without_empty_state() {
    let rows = vec![QueueJobRow::new(
        7,
        "https://example.test/big-file.zip".into(),
        "running".into(),
        5,
        16,
        true,
        0.0,
        None,
        512,
        Some(2048),
        "/tmp".into(),
        Some("big-file.zip".into()),
        None,
    )];
    let text = render(rows);
    assert!(text.contains("big-file.zip"), "{text}");
    assert!(text.contains("512.0 B / 2.0 KB (25.0%)"), "{text}");
    assert!(text.contains("running: 1"), "{text}");
    assert!(!text.contains("No downloads yet"), "{text}");
}
