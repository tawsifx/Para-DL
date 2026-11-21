//! # paradl-events
//!
//! The **only** JSONL emitter in the paraDL framework (framework charter §4,
//! parity matrix §3). Every observable event is serialized here as
//! `{ "event": <name>, "timestamp": round(time.time(), 3), ...redacted }`
//! with Python `json.dumps` separators and `ensure_ascii=True`.
//!
//! This crate never writes to stdout: [`emit_jsonl`] returns one newline-
//! terminated line. The reference binary / API is the sole owner of stdout and
//! writes those lines verbatim. Worker progress frames are re-emitted through
//! this crate, so a chatty worker can never corrupt `--json`.
//!
//! ```
//! use paradl_events::{Event, ProgressEvent, emit_jsonl, now_timestamp};
//!
//! let event = ProgressEvent::new(
//!     now_timestamp(), "demo.bin".into(), 1024,
//!     Some(2048), Some(50.0), Some(1000.0), Some(1.0), Some(16),
//!     None, Some("v2".into()), None, None,
//! );
//! let line = emit_jsonl(&event);
//! assert!(line.starts_with("{\"event\": \"progress\", \"timestamp\": "));
//! assert!(line.ends_with("\"watchdog_state\": \"v2\"}\n"));
//!
//! // The typed registry covers all 44 oracle names plus `resilience_event`.
//! assert_eq!(Event::all().len(), 45);
//! ```

#![warn(missing_docs)]

mod adopt;
mod json;
mod redaction;
mod schema;

pub use adopt::{
    AdoptableEvent, AdoptedEvent, from_watchdog, from_watchdog_event, redactor_for_event,
};
pub use redaction::{REDACTED, Redactor, SECRET_ENV_KEYS};
pub use schema::{
    ApiServerStartedEvent, AuthRemoveEvent, AuthSetEvent, AuthStatusEvent, BenchEvent, CleanEvent,
    ConfigEvent, ConfigResetEvent, ConfigUpdatedEvent, DeferredEvent, DiagnoseEvent, DoctorEvent,
    ErrorEvent, Event, HashesEvent, HistoryEvent, HookErrorEvent, HookExecutedEvent,
    HookFailedEvent, HookTimeoutEvent, HostsResetEvent, InfoEvent, InstallSecurityCheckEvent,
    InterruptedEvent, LaunchdPlistGeneratedEvent, MirrorDroppedEvent, MirrorRejectedEvent,
    NetprobeEvent, ProfilesEvent, ProfilesResetEvent, ProgressEvent, QueueCrashRecoveredEvent,
    QueueJobAddedEvent, QueueJobDeferredEvent, QueueJobRow, QueueJobStartedEvent, QueueListEvent,
    QueuePausedEvent, QueueRemovedEvent, QueueResumedEvent, ResilienceEventEvent, ResultEvent,
    TargetClassifiedEvent, ThrottleLaunchdPlistGeneratedEvent, VerificationEvent,
    WatchdogActionRangeEvent, WhenFastProbeEvent, names,
};

// The watchdog crate is complete (W1-3); consume its serializable event type
// and banker's-rounding helper instead of redefining them.
pub use paradl_watchdog::{
    WatchdogAction, WatchdogActionEvent, WatchdogActionType, WatchdogMode, py_round,
};

use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::json::PySerializer;

/// Version of the JSONL event schema produced by this crate.
///
/// This is a public capability constant, **not** a field in each line: the
/// oracle envelope has exactly `{event, timestamp, ...payload}` and adding a
/// version key would break byte-exactness. Protocol versioning for the media
/// worker is independent (`paradl-media/1`).
pub const EVENT_SCHEMA_VERSION: &str = "paradl-events/1";

/// Alias used by callers that negotiate emitter compatibility.
pub const JSONL_SCHEMA_VERSION: &str = EVENT_SCHEMA_VERSION;

static SECRETS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn secret_registry() -> &'static Mutex<Vec<String>> {
    SECRETS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Register a runtime secret (e.g. a token read from stdin) so that it is
/// redacted from every subsequent event line.
///
/// Mirrors `UI.register_secret` in `ui/display.py:87`. Short values (< 4
/// characters) are ignored, matching `redact_secrets`.
pub fn register_secret(secret: impl Into<String>) {
    let secret = secret.into();
    if secret.len() < 4 {
        return;
    }
    let mut guard = secret_registry().lock().unwrap_or_else(|error| error.into_inner());
    if !guard.contains(&secret) {
        guard.push(secret);
    }
}

/// Snapshot of all secrets registered so far.
pub(crate) fn registered_secrets() -> Vec<String> {
    secret_registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Return `round(time.time(), 3)` using Python's banker's rounding.
pub fn now_timestamp() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    py_round(secs, 3)
}

/// Serialize an event into one JSONL line, including the trailing newline.
///
/// Redaction is applied from the process environment (the four known token
/// variables) and from secrets passed to [`register_secret`]. The returned
/// string is ready to be written to stdout by the binary/API.
///
/// ```
/// use paradl_events::{HookExecutedEvent, emit_jsonl};
/// let line = emit_jsonl(&HookExecutedEvent::new(1.0, "/hooks/ok".into(), true));
/// assert_eq!(line, "{\"event\": \"hook_executed\", \"timestamp\": 1.0, \"hook\": \"/hooks/ok\", \"success\": true}\n");
/// ```
pub fn emit_jsonl<T: Serialize>(event: &T) -> String {
    emit_jsonl_with_secrets(event, &registered_secrets())
}

/// Serialize an event with an explicit set of additional secrets.
pub fn emit_jsonl_with_secrets<T: Serialize>(event: &T, secrets: &[String]) -> String {
    let mut line = to_json_line_with_secrets(event, secrets);
    line.push('\n');
    line
}

/// Serialize an event without a trailing newline (used for hook stdin JSON).
pub fn to_json_line<T: Serialize>(event: &T) -> String {
    to_json_line_with_secrets(event, &registered_secrets())
}

/// Serialize without a newline, with explicit additional secrets.
pub fn to_json_line_with_secrets<T: Serialize>(event: &T, secrets: &[String]) -> String {
    let redactor = Redactor::from_env_and(secrets);
    let mut serializer = PySerializer::new(redactor);
    // The serializer is infallible for all `Serialize` implementations that
    // produce JSON-compatible values; `Never` cannot be constructed.
    event.serialize(&mut serializer).unwrap_or_else(|never| match never {});
    serializer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_constant_is_public_and_stable() {
        assert_eq!(EVENT_SCHEMA_VERSION, "paradl-events/1");
        assert_eq!(JSONL_SCHEMA_VERSION, EVENT_SCHEMA_VERSION);
    }

    #[test]
    fn emitter_never_writes_a_newline_into_hook_payload() {
        let line = to_json_line(&HookExecutedEvent::new(1.0, "/h".into(), true));
        assert!(!line.ends_with('\n'));
        assert_eq!(emit_jsonl(&HookExecutedEvent::new(1.0, "/h".into(), true)), format!("{line}\n"));
    }
}
