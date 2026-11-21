//! Plain-text progress rendering driven by typed `paradl-events` values.
//!
//! The Python oracle is `LiveTransferDisplay` in `src/paradl/ui/display.py`.
//! This module reproduces its two pure renderings byte-for-byte:
//! [`ProgressDisplay::simple_line`] (`_format_simple_line`) and
//! [`ProgressDisplay::renderable`] (`_build_renderable`).
//!
//! This module only *reads* [`ProgressEvent`] values. It never serializes
//! JSONL: [`paradl_events::emit_jsonl`] remains the framework's single emitter.

use paradl_events::ProgressEvent;
use serde_json::Value;

use crate::format::{format_size, format_speed};

const TITLE_MAX: usize = 32;
const BAR_WIDTH: usize = 24;
const FILE_WIDTH: usize = 16;
const MAX_FILE_ROWS: usize = 6;

/// Renders typed progress events to plain strings for non-interactive output.
///
/// The renderer is stateless and deterministic: given the same event it always
/// returns the same string. The one oracle behavior that depends on wall-clock
/// time (the animated bar when the total size is unknown) is exposed through
/// [`ProgressDisplay::renderable_at`] so callers and golden tests can inject a
/// fixed time source.
///
/// ```
/// use paradl_events::ProgressEvent;
/// use paradl_ui::ProgressDisplay;
///
/// let event = ProgressEvent::new(
///     1.0, "demo.bin".into(), 1024, Some(2048), Some(50.0), Some(1000.0),
///     Some(1.0), None, None, None, None, None,
/// );
/// let display = ProgressDisplay::new();
/// assert!(display.simple_line(&event).contains("50.0%"));
/// assert!(display.renderable(&event).contains("[████████████"));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressDisplay {
    /// Width of the main progress bar (oracle constant: 24).
    bar_width: usize,
    /// Width of each per-file progress bar (oracle constant: 16).
    file_bar_width: usize,
}

impl ProgressDisplay {
    /// Construct a renderer with the oracle's default bar widths.
    pub fn new() -> Self {
        Self {
            bar_width: BAR_WIDTH,
            file_bar_width: FILE_WIDTH,
        }
    }

    /// Construct a renderer with custom bar widths (useful for narrow terminals).
    pub fn with_bar_widths(bar_width: usize, file_bar_width: usize) -> Self {
        Self {
            bar_width,
            file_bar_width,
        }
    }

    /// Return the compact, single-line progress string (`_format_simple_line`).
    ///
    /// The percentage, size and ETA are recomputed from `downloaded_bytes`,
    /// `total_bytes` and `speed_bps`, matching the oracle (which ignores the
    /// event's precomputed `percent`/`eta_seconds` for this rendering).
    pub fn simple_line(&self, event: &ProgressEvent) -> String {
        let total = positive_total(event);
        let downloaded = event.downloaded_bytes;
        let speed = event.speed_bps.unwrap_or(0.0);
        let connections = event.connections.unwrap_or(0);

        let pct = percent(downloaded, total);
        let eta = eta_string(total, downloaded, speed);
        let pct_str = if total > 0 {
            format!("{pct:5.1}%")
        } else {
            " ---.-%".to_string()
        };
        let size_part = size_part(downloaded, total);

        let mut parts = vec![
            format!("  {}", truncate(&event.title, TITLE_MAX)),
            pct_str,
            size_part,
            format_speed(speed),
            format!("ETA {eta}"),
        ];
        if connections > 1 {
            parts.push(format!("[{connections} conns]"));
        }
        parts.join("  ")
    }

    /// Return the full plain-text renderable (`_build_renderable`) using the
    /// current wall clock for the unknown-total animation.
    pub fn renderable(&self, event: &ProgressEvent) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs_f64())
            .unwrap_or_default();
        self.renderable_at(event, now)
    }

    /// Return the full plain-text renderable with an injected time source.
    ///
    /// `now_secs` is only used when the total size is unknown (the oracle
    /// animates a four-cell window with `int((time.time() * 8) % 24)`).
    #[allow(clippy::too_many_lines)]
    pub fn renderable_at(&self, event: &ProgressEvent, now_secs: f64) -> String {
        let total = positive_total(event);
        let downloaded = event.downloaded_bytes;
        let speed = event.speed_bps.unwrap_or(0.0);
        let pct = percent(downloaded, total);
        let eta = eta_string(total, downloaded, speed);

        let bar = if total > 0 {
            let filled = ((pct / 100.0).clamp(0.0, 1.0) * self.bar_width as f64) as usize;
            block_bar(filled, self.bar_width)
        } else {
            let position = ((now_secs * 8.0) % self.bar_width as f64) as usize;
            animated_bar(position, 4, self.bar_width)
        };
        let pct_str = if total > 0 {
            format!("{pct:5.1}%  ")
        } else {
            " ---.-%  ".to_string()
        };

        let mut text = String::new();
        text.push_str("  ");
        text.push_str(&truncate(&event.title, TITLE_MAX));
        text.push(' ');
        text.push('[');
        text.push_str(&bar);
        text.push_str("] ");
        text.push_str(&pct_str);
        text.push_str(&size_part(downloaded, total));
        text.push_str("  ");
        text.push_str(&format_speed(speed));
        text.push_str("  ETA ");
        text.push_str(&eta);
        text.push_str("  ");

        let mut connection_parts = Vec::new();
        let connections = event.connections.unwrap_or(0);
        if connections > 1 {
            connection_parts.push(format!("{connections} conns"));
        }
        if let Some(peers) = event.peer_count
            && peers > 0
        {
            connection_parts.push(format!("{peers} peers"));
        }
        if let Some(speeds) = connection_speeds(event.connection_speeds.as_ref())
            && !speeds.is_empty()
        {
            let average = speeds.iter().sum::<f64>() / speeds.len() as f64;
            connection_parts.push(format!("{}/conn", format_speed(average)));
        }
        if !connection_parts.is_empty() {
            text.push('[');
            text.push_str(&connection_parts.join(" | "));
            text.push_str("]  ");
        }

        if let Some(files) = per_file_lines(
            event.file_progress.as_ref(),
            self.file_bar_width,
            MAX_FILE_ROWS,
        ) {
            text.push('\n');
            for file in files {
                text.push_str(&file);
            }
        }

        if let Some(state) = event.watchdog_state.as_deref()
            && !state.is_empty()
            && !matches!(state.to_ascii_lowercase().as_str(), "active" | "none")
        {
            text.push('[');
            text.push_str(state);
            text.push(']');
        }

        text
    }
}

/// `self.total_bytes = total_bytes or 0` plus the `update` guard `> 0`.
fn positive_total(event: &ProgressEvent) -> i64 {
    event.total_bytes.filter(|&total| total > 0).unwrap_or(0)
}

fn percent(downloaded: i64, total: i64) -> f64 {
    if total > 0 {
        downloaded as f64 * 100.0 / total as f64
    } else {
        0.0
    }
}

fn eta_string(total: i64, downloaded: i64, speed: f64) -> String {
    let remaining = (total - downloaded).max(0);
    if speed > 0.0 && total > 0 {
        let seconds = (remaining as f64 / speed) as i64;
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    } else {
        "--:--".to_string()
    }
}

fn size_part(downloaded: i64, total: i64) -> String {
    let down = format_size(Some(downloaded as f64));
    if total > 0 {
        format!("{down} / {}", format_size(Some(total as f64)))
    } else {
        down
    }
}

/// Python slice semantics for a string (`title[:32]`) operate on Unicode
/// scalars, not bytes.
fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn block_bar(filled: usize, width: usize) -> String {
    let filled = filled.min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn animated_bar(position: usize, animated: usize, width: usize) -> String {
    let mut cells = vec!['░'; width];
    for offset in 0..animated {
        if width > 0 {
            cells[(position + offset) % width] = '█';
        }
    }
    cells.into_iter().collect()
}

fn connection_speeds(value: Option<&Value>) -> Option<Vec<f64>> {
    let array = value?.as_array()?;
    Some(array.iter().filter_map(Value::as_f64).collect())
}

fn per_file_lines(value: Option<&Value>, bar_width: usize, max_rows: usize) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    if array.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for item in array.iter().take(max_rows) {
        let name = item
            .get("name")
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            })
            .unwrap_or_else(|| "file".to_string());
        let downloaded = item.get("downloaded").and_then(Value::as_i64).unwrap_or(0);
        let total = item.get("total").and_then(Value::as_i64).unwrap_or(0);
        let pct = percent(downloaded, total);
        let bar = if total > 0 {
            let filled = ((pct / 100.0).clamp(0.0, 1.0) * bar_width as f64) as usize;
            block_bar(filled, bar_width)
        } else {
            block_bar(0, bar_width)
        };
        let name = truncate(&name, 28);
        let mut line = format!("    {name:<28} [{bar}] {pct:5.1}%  ");
        if total > 0 {
            line.push_str(&format!(
                "{}/{}",
                format_size(Some(downloaded as f64)),
                format_size(Some(total as f64))
            ));
        } else {
            line.push_str(&format_size(Some(downloaded as f64)));
        }
        line.push('\n');
        lines.push(line);
    }
    Some(lines)
}
