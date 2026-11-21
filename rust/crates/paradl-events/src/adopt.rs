//! Adoption adapters for typed events defined in other crates.
//!
//! `paradl-events` must not depend on `paradl-throttle` (W2-1 is developed in
//! parallel). Instead, any `Serialize` payload from another crate can be
//! adopted with [`AdoptedEvent`], which supplies the `event` name and rounded
//! `timestamp` envelope fields. The payload is then serialized by the *single*
//! emitter and receives the same redaction as locally defined events.
//!
//! `paradl-watchdog` is already complete, so its [`WatchdogActionEvent`] is
//! consumed directly (see [`crate::from_watchdog`]) and needs no adapter.

use serde::Serialize;

use crate::Redactor;

/// A typed event originating in another crate.
///
/// Implementors guarantee a stable `event` name; the emitter adds `timestamp`.
pub trait AdoptableEvent: Serialize {
    /// Canonical event name written into the envelope.
    fn event_name(&self) -> &'static str;
}

/// Envelope wrapper that adopts a payload from another crate.
///
/// ```
/// use paradl_events::{AdoptedEvent, emit_jsonl};
///
/// #[derive(serde::Serialize)]
/// struct ThrottleTyped { label: &'static str }
///
/// let adopted = AdoptedEvent::new("throttle_launchd_plist_generated", 1763376000.123,
///                                 ThrottleTyped { label: "sample" });
/// let line = emit_jsonl(&adopted);
/// assert!(line.contains("\"event\": \"throttle_launchd_plist_generated\""));
/// assert!(line.contains("\"label\": \"sample\""));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptedEvent<T: Serialize> {
    /// Canonical event name.
    pub event: &'static str,
    /// `round(time.time(), 3)` at emission time.
    pub timestamp: f64,
    /// Foreign typed payload, flattened into the same JSON object.
    #[serde(flatten)]
    pub payload: T,
}

impl<T: Serialize> AdoptedEvent<T> {
    /// Adopt a foreign payload with an explicit event name and timestamp.
    pub fn new(event: &'static str, timestamp: f64, payload: T) -> Self {
        Self {
            event,
            timestamp,
            payload,
        }
    }

    /// Adopt a foreign payload using the current wall-clock timestamp.
    pub fn now(event: &'static str, payload: T) -> Self {
        Self::new(event, crate::now_timestamp(), payload)
    }
}

impl<T: Serialize> AdoptableEvent for AdoptedEvent<T> {
    fn event_name(&self) -> &'static str {
        self.event
    }
}

/// Serialize a `paradl-watchdog` action through the single emitter.
///
/// This consumes the public [`paradl_watchdog::WatchdogActionEvent`] type; no
/// code in `paradl-watchdog` writes to stdout.
pub fn from_watchdog(action: &paradl_watchdog::WatchdogAction) -> String {
    crate::emit_jsonl(&action.to_event())
}

/// Serialize an already-built watchdog event without re-deriving it.
pub fn from_watchdog_event(event: &paradl_watchdog::WatchdogActionEvent) -> String {
    crate::emit_jsonl(event)
}

/// Expose the redactor so an adoption adapter can pre-check a payload.
pub fn redactor_for_event() -> Redactor {
    Redactor::from_env_and(&crate::registered_secrets())
}
