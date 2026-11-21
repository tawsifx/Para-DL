//! Read-only display views over the typed `paradl-events` registry.
//!
//! Every function here consumes an already-built [`Event`]; none of them
//! serializes or writes JSONL. The single-emitter rule is therefore enforced
//! structurally: this crate has no path back to an emitter.

use paradl_events::Event;

use crate::ProgressDisplay;

/// Render any typed event as one human-readable plain-text line.
///
/// `progress` events use [`ProgressDisplay::simple_line`] so the compact
/// progress rendering has exactly one implementation. Other events with a
/// useful one-line payload get a short label; everything else falls back to
/// the canonical event name.
///
/// The result is never JSON: it contains no `event`/`timestamp` envelope and
/// is safe to print on stdout without corrupting `--json` output.
///
/// ```
/// use paradl_events::{ErrorEvent, Event, ProgressEvent};
/// use paradl_ui::event_summary;
///
/// let progress = Event::Progress(ProgressEvent::new(
///     1.0, "f.bin".into(), 10, Some(100), Some(10.0), Some(512.0),
///     None, None, None, None, None, None,
/// ));
/// assert!(event_summary(&progress).contains("10.0%"));
///
/// let error = Event::Error(ErrorEvent::new(1.0, "boom".into(),
///     None, None, None, None, None));
/// assert_eq!(event_summary(&error), "error: boom");
/// ```
pub fn event_summary(event: &Event) -> String {
    match event {
        Event::Progress(progress) => ProgressDisplay::new().simple_line(progress),
        Event::Error(error) => format!("error: {}", error.message),
        Event::Result(result) => {
            let target = result.target.as_deref().unwrap_or("unknown");
            if result.success {
                format!("result: success — {target}")
            } else {
                let reason = result.error.as_deref().unwrap_or("unknown error");
                format!("result: failure — {target}: {reason}")
            }
        }
        Event::TargetClassified(target) => {
            format!("target: {} ({})", target.target, target.target_type)
        }
        Event::Verification(verification) => {
            let outcome = if verification.verified {
                "verified"
            } else {
                "failed"
            };
            format!("verification: {outcome} — {}", verification.file)
        }
        other => other.name().to_string(),
    }
}
