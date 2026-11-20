//! Progress delivery through the single emitter.
//!
//! A worker `progress` frame is converted into a `paradl-events`
//! [`ProgressEvent`] and serialized with that crate's redactor. This crate
//! never writes to stdout: the redacted line is handed to a [`ProgressSink`]
//! owned by the reference binary/API.

use paradl_events::{ProgressEvent, emit_jsonl, now_timestamp};

use crate::protocol::ProgressFrame;

/// Receives one already-serialized, redacted JSONL event line.
///
/// Implementations normally write the line to stdout; this crate only
/// produces it. A closure `FnMut(&str)` is automatically a sink.
pub trait ProgressSink: Send {
    /// Accept one redacted `paradl-events` line (with trailing newline).
    fn on_progress(&mut self, line: &str);
}

/// A [`ProgressSink`] that discards every event line.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn on_progress(&mut self, _line: &str) {}
}

impl<F> ProgressSink for F
where
    F: FnMut(&str) + Send,
{
    fn on_progress(&mut self, line: &str) {
        self(line);
    }
}

/// Convert a worker progress frame into a redacted event line and deliver it.
pub fn reemit_progress(frame: &ProgressFrame, title: &str, sink: &mut dyn ProgressSink) {
    let percent = frame.total_bytes.filter(|total| *total > 0).map(|total| {
        (frame.downloaded_bytes as f64 / total as f64) * 100.0
    });
    let event = ProgressEvent::new(
        now_timestamp(),
        title.to_owned(),
        frame.downloaded_bytes as i64,
        frame.total_bytes.map(|total| total as i64),
        percent,
        Some(frame.speed_bps as f64),
        frame.eta_seconds,
        None,
        None,
        None,
        None,
        None,
    );
    sink.on_progress(&emit_jsonl(&event));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_line_is_a_redacted_progress_event() {
        paradl_events::register_secret("s3cr3t-token-42");
        let frame = ProgressFrame {
            downloaded_bytes: 50,
            total_bytes: Some(100),
            speed_bps: 10,
            eta_seconds: Some(5.0),
        };
        let mut lines = Vec::new();
        reemit_progress(&frame, "s3cr3t-token-42", &mut |line: &str| lines.push(line.to_owned()));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("{\"event\": \"progress\", \"timestamp\": "));
        assert!(lines[0].contains("\"percent\": 50.0"));
        assert!(!lines[0].contains("s3cr3t-token-42"));
    }
}
