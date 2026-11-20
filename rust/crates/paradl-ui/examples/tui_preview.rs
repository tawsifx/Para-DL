//! TUI-feature example: feed typed events into the ratatui app and render a
//! frame to an in-memory buffer (no TTY or system tools required).
//!
//! Run with:
//! ```sh
//! cargo run -p paradl-ui --features tui --example tui_preview
//! ```

use paradl_events::{Event, QueueJobRow, QueueListEvent, ResultEvent};
use paradl_ui::tui::{TuiApp, render_app};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn main() {
    let mut app = TuiApp::new();
    app.apply_event(&Event::QueueList(QueueListEvent::new(
        1.0,
        vec![QueueJobRow::new(
            1,
            "https://example.test/sample.bin".into(),
            "running".into(),
            0,
            8,
            false,
            0.0,
            None,
            512,
            Some(1024),
            ".".into(),
            Some("sample.bin".into()),
            None,
        )],
    )));
    app.apply_event(&Event::Result(ResultEvent::new(
        2.0,
        None,
        Some("https://example.test/sample.bin".into()),
        true,
        Some("./sample.bin".into()),
        None,
        None,
        0,
        None,
        Some(1024),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )));

    let mut terminal = Terminal::new(TestBackend::new(96, 22)).expect("test backend");
    terminal
        .draw(|frame| render_app(&app, frame))
        .expect("draw");

    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    let cells = buffer.content();
    for y in 0..area.height {
        for x in 0..area.width {
            let index = y as usize * area.width as usize + x as usize;
            print!("{}", cells[index].symbol());
        }
        println!();
    }
}
