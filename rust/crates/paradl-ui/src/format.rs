//! Byte/speed formatting ported verbatim from `paradl.core.models`.
//!
//! These helpers are shared by the plain progress display and the optional
//! TUI so their numbers match the Python oracle exactly.

/// Format a byte count like `paradl.core.models.format_size`.
///
/// `None` becomes `"Unknown size"`; non-positive values become `"0 B"`;
/// positive values are rendered with one decimal in the largest fitting
/// binary unit up to `TB` (with `PB` as the final fallback).
///
/// ```
/// use paradl_ui::format_size;
/// assert_eq!(format_size(None), "Unknown size");
/// assert_eq!(format_size(Some(1536.0)), "1.5 KB");
/// ```
pub fn format_size(bytes: Option<f64>) -> String {
    let Some(mut value) = bytes else {
        return "Unknown size".to_string();
    };
    if value <= 0.0 {
        return "0 B".to_string();
    }
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1} PB")
}

/// Format a throughput in bytes/s like `paradl.core.models.format_speed`.
///
/// Values at or above 1 MiB use `MB/s`, at or above 1 KiB use `KB/s`, and
/// everything else uses a rounded integer `B/s`.
///
/// ```
/// use paradl_ui::format_speed;
/// assert_eq!(format_speed(12.4), "12 B/s");
/// assert_eq!(format_speed(1536.0), "1.5 KB/s");
/// assert_eq!(format_speed(1_048_576.0), "1.0 MB/s");
/// ```
pub fn format_speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / (1024.0 * 1024.0))
    } else if bps >= 1024.0 {
        format!("{:.1} KB/s", bps / 1024.0)
    } else {
        format!("{bps:.0} B/s")
    }
}
