//! Interactive ratatui terminal UI driven by typed `paradl-events` values.
//!
//! This module is compiled only with the `tui` feature. Its pure model
//! (`TaskState`, `TaskStore`, `apply_event`) is TTY-independent and can
//! be tested with an in-memory buffer; only `TuiApp::run` / `run_tui`
//! enter raw terminal mode.
//!
//! The model and status colors mirror the Python oracle
//! `src/paradl/ui/tui.py` (`_TaskState`, `STATUS_LABELS`, `STATUS_COLORS`).

use std::collections::BTreeMap;
use std::io::{self, IsTerminal};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use paradl_events::{Event, py_round};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use serde_json::Value;

use crate::format::{format_size, format_speed};
use crate::{Error, Result};

/// Status label shown for an unknown or empty status string.
const DEFAULT_CATEGORY: &str = "—";

/// Map a queue status to the oracle's human-readable label.
///
/// Unknown statuses are returned unchanged, matching
/// `STATUS_LABELS.get(self.status, self.status)` in `tui.py`.
pub fn status_label(status: &str) -> &str {
    match status {
        "queued" | "running" | "paused" | "completed" | "failed" => status,
        _ => status,
    }
}

/// Semantic status colors from `STATUS_COLORS` in `tui.py`.
///
/// Values are exact RGB equivalents of the oracle's hex tokens
/// (`#888888`, `#00ff88`, `#ffcc00`, `#00cc66`, `#ff4444`).
pub fn status_color(status: &str) -> Color {
    match status {
        "queued" => Color::Rgb(0x88, 0x88, 0x88),
        "running" => Color::Rgb(0x00, 0xFF, 0x88),
        "paused" => Color::Rgb(0xFF, 0xCC, 0x00),
        "completed" => Color::Rgb(0x00, 0xCC, 0x66),
        "failed" => Color::Rgb(0xFF, 0x44, 0x44),
        _ => Color::Reset,
    }
}

/// Thread-safe-in-spirit snapshot of one task, ported from `_TaskState`.
///
/// In the Rust UI updates flow through [`TaskStore`], which owns the
/// synchronization boundary at the application layer instead of a mutex on
/// every field.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskState {
    /// Queue job identifier.
    pub id: i64,
    /// Original download target.
    pub target: String,
    /// Resolved target type (e.g. `direct`).
    pub target_type: String,
    /// User-defined category label.
    pub category: String,
    /// Absolute output directory.
    pub out_dir: String,
    /// Output filename, when known.
    pub filename: Option<String>,
    /// Current status (`queued`, `running`, `paused`, `completed`, `failed`).
    pub status: String,
    /// Effective connection count.
    pub connections: i64,
    /// Job priority.
    pub priority: i64,
    /// Bytes downloaded so far.
    pub bytes_downloaded: i64,
    /// Expected total bytes, when known.
    pub total_bytes: Option<i64>,
    /// Instantaneous speed in bytes/s.
    pub speed_bps: f64,
    /// Failure message, when failed.
    pub error: Option<String>,
    /// Final file path, when completed.
    pub file_path: Option<String>,
    /// Creation timestamp string (used for stable ordering).
    pub created_at: String,
    /// Last-update timestamp string.
    pub updated_at: String,
}

impl Default for TaskState {
    fn default() -> Self {
        Self {
            id: 0,
            target: String::new(),
            target_type: "direct".into(),
            category: String::new(),
            out_dir: ".".into(),
            filename: None,
            status: "queued".into(),
            connections: 16,
            priority: 0,
            bytes_downloaded: 0,
            total_bytes: None,
            speed_bps: 0.0,
            error: None,
            file_path: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }
}

impl TaskState {
    /// Return the completion percentage rounded to one decimal, like the
    /// oracle's `round(bytes * 100.0 / total, 1)` (banker's rounding).
    pub fn progress_pct(&self) -> f64 {
        match self.total_bytes {
            Some(total) if total > 0 => {
                py_round(self.bytes_downloaded as f64 * 100.0 / total as f64, 1)
            }
            _ => 0.0,
        }
    }

    /// Return a Unicode block progress bar of `width` cells.
    ///
    /// Zero progress renders entirely as `░`; any non-zero progress renders at
    /// least one `█`, matching `_TaskState.progress_bar` in the oracle.
    pub fn progress_bar(&self, width: usize) -> String {
        let pct = self.progress_pct();
        if pct == 0.0 {
            return "░".repeat(width);
        }
        let filled = (width as f64 * pct / 100.0) as usize;
        let filled = filled.clamp(1, width);
        format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
    }

    /// Return the human-readable status label.
    pub fn status_text(&self) -> &str {
        status_label(&self.status)
    }

    /// Return the seven-cell `DataTable` row produced by `to_row()`.
    pub fn to_row(&self) -> [String; 7] {
        let pct = self.progress_pct();
        let progress = match self.total_bytes {
            Some(total) if total > 0 => format!(
                "{} / {} ({pct:.1}%)",
                format_size(Some(self.bytes_downloaded as f64)),
                format_size(Some(total as f64)),
            ),
            _ => format_size(Some(self.bytes_downloaded as f64)),
        };
        let name = self.filename.as_deref().unwrap_or(&self.target);
        [
            self.id.to_string(),
            name.chars().take(46).collect(),
            if self.category.is_empty() {
                DEFAULT_CATEGORY.to_string()
            } else {
                self.category.clone()
            },
            progress,
            self.status_text().to_string(),
            if self.speed_bps > 0.0 {
                format_speed(self.speed_bps)
            } else {
                DEFAULT_CATEGORY.to_string()
            },
            self.connections.to_string(),
        ]
    }
}

/// Ordered collection of task snapshots keyed by job id.
#[derive(Debug, Clone, Default)]
pub struct TaskStore {
    tasks: BTreeMap<i64, TaskState>,
}

impl TaskStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a task snapshot.
    pub fn upsert(&mut self, task: TaskState) {
        self.tasks.insert(task.id, task);
    }

    /// Return a task by id.
    pub fn get(&self, id: i64) -> Option<&TaskState> {
        self.tasks.get(&id)
    }

    /// Return all tasks, newest `created_at` first (oracle `list_all` order).
    pub fn list_all(&self) -> Vec<&TaskState> {
        let mut tasks: Vec<&TaskState> = self.tasks.values().collect();
        tasks.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        tasks
    }

    /// Return `true` when no tasks are stored.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Update the mutable fields of one task, if present.
    #[allow(clippy::too_many_arguments)]
    pub fn update_progress(
        &mut self,
        id: i64,
        bytes_downloaded: Option<i64>,
        total_bytes: Option<i64>,
        speed_bps: Option<f64>,
        status: Option<&str>,
        error: Option<&str>,
        file_path: Option<&str>,
    ) {
        if let Some(task) = self.tasks.get_mut(&id) {
            if let Some(bytes) = bytes_downloaded {
                task.bytes_downloaded = bytes;
            }
            if let Some(total) = total_bytes {
                task.total_bytes = Some(total);
            }
            if let Some(speed) = speed_bps {
                task.speed_bps = speed;
            }
            if let Some(status) = status {
                task.status = status.to_string();
            }
            if let Some(error) = error {
                task.error = Some(error.to_string());
            }
            if let Some(path) = file_path {
                task.file_path = Some(path.to_string());
            }
        }
    }

    /// Change only the status (and update timestamp) of one task.
    pub fn set_status(&mut self, id: i64, status: &str) {
        if let Some(task) = self.tasks.get_mut(&id) {
            task.status = status.to_string();
        }
    }

    /// Remove a task by id.
    pub fn remove(&mut self, id: i64) {
        self.tasks.remove(&id);
    }

    /// Remove every task.
    pub fn clear(&mut self) {
        self.tasks.clear();
    }
}

/// Apply one typed `paradl-events` event to the TUI task store.
///
/// This is the only bridge between the emitter crate and the visible task
/// list. Events that do not affect the task list are ignored. Progress events
/// have no job id in the oracle schema, so they update the first `running`
/// task (or the first task when none is running).
pub fn apply_event(store: &mut TaskStore, event: &Event) {
    match event {
        Event::QueueList(list) => {
            for row in &list.jobs {
                store.upsert(TaskState {
                    id: row.id,
                    target: row.target.clone(),
                    target_type: "direct".into(),
                    status: row.status.clone(),
                    priority: row.priority,
                    connections: row.connections,
                    bytes_downloaded: row.bytes_downloaded,
                    total_bytes: row.expected_length,
                    out_dir: row.out_dir.clone(),
                    filename: row.filename.clone(),
                    error: row.error.clone(),
                    ..TaskState::default()
                });
            }
        }
        Event::QueueJobStarted(started) => {
            store.update_progress(
                started.job_id,
                None,
                None,
                None,
                Some("running"),
                None,
                None,
            );
        }
        Event::Progress(progress) => {
            let tasks = store.list_all();
            let target = tasks
                .iter()
                .find(|task| task.status == "running")
                .or_else(|| tasks.first());
            if let Some(id) = target.map(|task| task.id) {
                store.update_progress(
                    id,
                    Some(progress.downloaded_bytes),
                    progress.total_bytes,
                    progress.speed_bps,
                    Some("running"),
                    None,
                    None,
                );
            }
        }
        Event::Result(result) => {
            let status = if result.success {
                "completed"
            } else {
                "failed"
            };
            let tasks = store.list_all();
            let target = tasks
                .iter()
                .find(|task| result.target.as_deref() == Some(task.target.as_str()))
                .or_else(|| {
                    tasks
                        .iter()
                        .find(|task| matches!(task.status.as_str(), "running" | "queued"))
                });
            if let Some(id) = target.map(|task| task.id) {
                store.update_progress(
                    id,
                    result.bytes_downloaded,
                    None,
                    None,
                    Some(status),
                    result.error.as_deref(),
                    result.file_path.as_deref(),
                );
            }
        }
        Event::QueuePaused(paused) => {
            for id in changed_ids(&paused.paused) {
                store.set_status(id, "paused");
            }
        }
        Event::QueueResumed(resumed) => {
            for id in changed_ids(&resumed.resumed) {
                store.set_status(id, "queued");
            }
        }
        Event::QueueRemoved(removed) => {
            let ids: Vec<i64> = store
                .list_all()
                .iter()
                .take(removed.removed)
                .map(|task| task.id)
                .collect();
            for id in ids {
                store.remove(id);
            }
        }
        _ => {}
    }
}

/// Parse a `queue_paused`/`queue_resumed` payload into job ids.
fn changed_ids(value: &Value) -> Vec<i64> {
    match value {
        Value::Number(number) => number.as_i64().into_iter().collect(),
        Value::Array(items) => items.iter().filter_map(Value::as_i64).collect(),
        _ => Vec::new(),
    }
}

/// Application state for the ratatui screen.
#[derive(Debug, Clone, Default)]
pub struct TuiApp {
    store: TaskStore,
    search: String,
    category: String,
    selected: usize,
}

impl TuiApp {
    /// Create an empty application state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the task store.
    pub fn store(&self) -> &TaskStore {
        &self.store
    }

    /// Mutably borrow the task store.
    pub fn store_mut(&mut self) -> &mut TaskStore {
        &mut self.store
    }

    /// Set the search filter (case-insensitive target/filename substring).
    pub fn set_search(&mut self, search: impl Into<String>) {
        self.search = search.into().to_ascii_lowercase();
        self.selected = 0;
    }

    /// Set the category filter.
    pub fn set_category(&mut self, category: impl Into<String>) {
        self.category = category.into();
        self.selected = 0;
    }

    /// Consume one typed event, updating the task store.
    pub fn apply_event(&mut self, event: &Event) {
        apply_event(&mut self.store, event);
    }

    /// Return tasks matching the active search and category filters.
    pub fn filtered_tasks(&self) -> Vec<&TaskState> {
        self.store
            .list_all()
            .into_iter()
            .filter(|task| {
                let matches_search = self.search.is_empty()
                    || task.target.to_ascii_lowercase().contains(&self.search)
                    || task
                        .filename
                        .as_deref()
                        .is_some_and(|name| name.to_ascii_lowercase().contains(&self.search));
                let matches_category = self.category.is_empty() || task.category == self.category;
                matches_search && matches_category
            })
            .collect()
    }

    /// Return the index of the selected task row.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Move the selection down by one row.
    pub fn select_next(&mut self) {
        let count = self.filtered_tasks().len();
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
    }

    /// Move the selection up by one row.
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Return the status bar text (sorted counts plus keyboard hints).
    pub fn status_text(&self) -> String {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for task in self.store.list_all() {
            *counts.entry(task.status.as_str()).or_default() += 1;
        }
        let parts: Vec<String> = counts
            .iter()
            .map(|(status, count)| format!("{status}: {count}"))
            .collect();
        let summary = if parts.is_empty() {
            "No tasks".to_string()
        } else {
            parts.join(" | ")
        };
        format!("{summary}  a:add  p:pause  r:resume  d:delete  q:quit")
    }

    /// Render the application into one frame.
    pub fn render(&self, frame: &mut Frame) {
        render_app(self, frame);
    }

    /// Run the interactive event loop, feeding typed events through `events`.
    ///
    /// Returns [`Error::usage`] on a non-interactive stdin/stdout and
    /// [`Error::Io`] for terminal setup failures.
    pub fn run(&mut self, events: Receiver<Event>) -> Result<()> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(Error::usage(
                "tui requires an interactive TTY; omit --tui for plain progress",
            ));
        }
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let outcome = self.event_loop(&mut terminal, events);

        disable_raw_mode()?;
        terminal.show_cursor()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        outcome
    }

    fn event_loop<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        events: Receiver<Event>,
    ) -> Result<()> {
        loop {
            terminal.draw(|frame| self.render(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let CrosstermEvent::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('j') | KeyCode::Down => self.select_next(),
                    KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
                    _ => {}
                }
            }

            while let Ok(event) = events.try_recv() {
                self.apply_event(&event);
            }
        }
        Ok(())
    }
}

/// Draw the full application layout into a frame.
///
/// Layout, labels and empty state mirror `PdlTuiApp.compose`/`_refresh_table`
/// from `tui.py`. It is public so tests can render into a
/// [`ratatui::backend::TestBackend`] buffer without a TTY.
pub fn render_app(app: &TuiApp, frame: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            "pdl",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::White),
        )),
        Line::from(Span::styled(
            "Parallel Download Manager",
            Style::default().fg(Color::Gray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    let filter = Paragraph::new(vec![
        Line::from(format!("Search: {}", app.search)),
        Line::from(format!("Category: {}", app.category)),
    ])
    .block(Block::default().borders(Borders::ALL).title("Filters"));
    frame.render_widget(filter, chunks[1]);

    let rows = app.filtered_tasks().into_iter().map(|task| {
        let cells = task.to_row();
        let status = cells[4].clone();
        Row::new(vec![
            Cell::from(cells[0].clone()),
            Cell::from(cells[1].clone()),
            Cell::from(cells[2].clone()),
            Cell::from(cells[3].clone()),
            Cell::from(Span::styled(
                status,
                Style::default().fg(status_color(&task.status)),
            )),
            Cell::from(cells[5].clone()),
            Cell::from(cells[6].clone()),
        ])
    });
    let widths = [
        Constraint::Length(5),
        Constraint::Length(46),
        Constraint::Length(12),
        Constraint::Length(28),
        Constraint::Length(12),
        Constraint::Length(12),
        Constraint::Length(6),
    ];
    let table = Table::new(rows, widths)
        .header(Row::new([
            "ID",
            "Target / Filename",
            "Category",
            "Progress",
            "Status",
            "Speed",
            "Conns",
        ]))
        .block(Block::default().borders(Borders::ALL).title("Downloads"));
    frame.render_widget(table, chunks[2]);

    if app.filtered_tasks().is_empty() {
        let area = centered_rect(chunks[2], 44, 5);
        frame.render_widget(Clear, area);
        let empty = Paragraph::new(vec![
            Line::from(Span::styled(
                "No downloads yet",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from("Press 'a' to add your first download"),
        ])
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        frame.render_widget(empty, area);
    }

    let actions = Paragraph::new("Add | Start Queue | Pause | Resume | Delete | Refresh | Quit")
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(actions, chunks[3]);

    frame.render_widget(Paragraph::new(app.status_text()), chunks[4]);
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: area.width.min(width),
        height: area.height.min(height),
    }
}

/// Run the ratatui TUI, consuming typed events until the user quits.
///
/// This is the library entry point used by the reference binary. It returns
/// [`Error::usage`] immediately when no interactive TTY is attached.
pub fn run_tui(events: Receiver<Event>) -> Result<()> {
    TuiApp::new().run(events)
}
