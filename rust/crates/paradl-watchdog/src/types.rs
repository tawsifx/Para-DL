//! Public data types for the watchdog: actions, decisions, telemetry samples,
//! resilience events, state snapshots, and the serializable JSONL event.
//!
//! All string literals are byte-exact ports of
//! `src/paradl/engines/watchdog.py` (parity matrix rows L07, L18, E60).

use paradl_core::WatchdogMode;
use serde::Serialize;

/// The action a watchdog decision asks the download engine to take.
///
/// Mirrors `WatchdogActionType` (`StrEnum`). The `PauseUnpause` value is the
/// exact literal `tier2_pause_unpause` required by parity row L07.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum WatchdogActionType {
    /// No action.
    #[serde(rename = "none")]
    None,
    /// Cycle all RPC sockets (in-process pause/unpause).
    #[serde(rename = "tier2_pause_unpause")]
    PauseUnpause,
    /// Recycle connections on a WAKE / NET_CHANGE / STALL event.
    #[serde(rename = "resilience_recycle")]
    ResilienceRecycle,
    /// Revert connections after a failed stale-profile re-validation.
    #[serde(rename = "adjust_connections")]
    AdjustConnections,
    /// Switch IP family (reserved; emitted by the v1 policy engine).
    #[serde(rename = "switch_ip_family")]
    SwitchIpFamily,
    /// Restart the transfer process (reserved; tier-3 escalation).
    #[serde(rename = "tier3_restart")]
    ProcessRestart,
    /// Fall back to a single stream (reserved).
    #[serde(rename = "single_stream_fallback")]
    SingleStreamFallback,
}

impl WatchdogActionType {
    /// Return the exact Python `StrEnum` string value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::PauseUnpause => "tier2_pause_unpause",
            Self::ResilienceRecycle => "resilience_recycle",
            Self::AdjustConnections => "adjust_connections",
            Self::SwitchIpFamily => "switch_ip_family",
            Self::ProcessRestart => "tier3_restart",
            Self::SingleStreamFallback => "single_stream_fallback",
        }
    }
}

impl std::fmt::Display for WatchdogActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of resilience event delivered to the watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ResilienceEventType {
    /// Wall clock jumped ahead of monotonic time (macOS sleep/wake).
    #[serde(rename = "wake")]
    Wake,
    /// Network interface / SSID changed.
    #[serde(rename = "net_change")]
    NetChange,
    /// No byte progress for the configured stall timeout.
    #[serde(rename = "stall")]
    Stall,
}

impl ResilienceEventType {
    /// Return the exact Python `StrEnum` string value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wake => "wake",
            Self::NetChange => "net_change",
            Self::Stall => "stall",
        }
    }
}

/// A WAKE, NET_CHANGE, or STALL event from the resilience event source.
#[derive(Debug, Clone, PartialEq)]
pub struct ResilienceEvent {
    /// Event kind.
    pub event_type: ResilienceEventType,
    /// Human-readable reason, included verbatim in the watchdog decision.
    pub reason: String,
    /// Wall-clock epoch seconds of the event.
    pub timestamp: f64,
}

impl ResilienceEvent {
    /// Construct a resilience event.
    pub fn new(event_type: ResilienceEventType, reason: impl Into<String>, timestamp: f64) -> Self {
        Self {
            event_type,
            reason: reason.into(),
            timestamp,
        }
    }
}

/// One telemetry tick delivered to [`crate::Watchdog::observe`].
///
/// `elapsed_sec` is the caller-provided monotonic seconds since the transfer
/// began (the watchdog never reads real time for elapsed calculations).
#[derive(Debug, Clone, PartialEq)]
pub struct ThroughputSample {
    /// Monotonic seconds since transfer start.
    pub elapsed_sec: f64,
    /// Aggregate throughput in bytes per second.
    pub speed_bps: f64,
    /// Optional per-connection speeds, used for relative straggler detection.
    pub per_conn_speeds: Option<Vec<f64>>,
    /// Optional resilience event; takes precedence over throughput processing.
    pub resilience_event: Option<ResilienceEvent>,
}

impl ThroughputSample {
    /// Construct a throughput-only sample.
    pub fn new(elapsed_sec: f64, speed_bps: f64) -> Self {
        Self {
            elapsed_sec,
            speed_bps,
            per_conn_speeds: None,
            resilience_event: None,
        }
    }

    /// Attach per-connection speeds for straggler detection.
    pub fn with_per_conn_speeds(mut self, speeds: impl Into<Vec<f64>>) -> Self {
        self.per_conn_speeds = Some(speeds.into());
        self
    }

    /// Attach a resilience event.
    pub fn with_resilience_event(mut self, event: ResilienceEvent) -> Self {
        self.resilience_event = Some(event);
        self
    }
}

/// The complete result of one watchdog observation.
///
/// This is the public return value of [`crate::Watchdog::observe`] and carries
/// everything needed to build a byte-exact `watchdog_action` JSONL event via
/// [`WatchdogAction::to_event`].
#[derive(Debug, Clone, PartialEq)]
pub struct WatchdogAction {
    /// The action to take.
    pub action: WatchdogActionType,
    /// Human-readable reason (empty when no action is taken).
    pub reason: String,
    /// Proposed connection count, when the action changes it.
    pub new_connections: Option<i32>,
    /// Proposed IP family, when the action changes it.
    pub new_ip_family: Option<String>,
    /// Effective threshold in bytes per second at this tick.
    pub threshold_bps: f64,
    /// Rolling p90 at this tick (0.0 for v1 / fewer than 3 positive samples).
    pub rolling_p90_bps: f64,
    /// Recycle cycle counter after this tick.
    pub cycle: i64,
    /// Resilience event type string, when one was handled.
    pub event_type: Option<String>,
    /// Observed speed in bytes per second at this tick.
    pub speed_bps: f64,
    /// Current connection count.
    pub connections: i32,
    /// Watchdog mode at this tick.
    pub mode: WatchdogMode,
    /// Event timestamp, rounded with Python `round(time.time(), 3)` semantics.
    pub timestamp: f64,
}

impl WatchdogAction {
    /// Construct a no-op action with the supplied telemetry context.
    pub fn new(action: WatchdogActionType) -> Self {
        Self {
            action,
            reason: String::new(),
            new_connections: None,
            new_ip_family: None,
            threshold_bps: 0.0,
            rolling_p90_bps: 0.0,
            cycle: 0,
            event_type: None,
            speed_bps: 0.0,
            connections: 0,
            mode: WatchdogMode::V2,
            timestamp: 0.0,
        }
    }

    /// Return `true` when this action resets per-flow sockets.
    ///
    /// Mirrors Python `RECYCLE_ACTIONS`: resilience recycle, pause/unpause, and
    /// adjust-connections all map to one `recycle_connections()` call.
    pub fn is_recycle(&self) -> bool {
        matches!(
            self.action,
            WatchdogActionType::ResilienceRecycle
                | WatchdogActionType::PauseUnpause
                | WatchdogActionType::AdjustConnections
        )
    }

    /// Convert this action into the serializable JSONL event consumed by
    /// `paradl-events` (W2-4).
    pub fn to_event(&self) -> WatchdogActionEvent {
        WatchdogActionEvent {
            event: "watchdog_action",
            timestamp: self.timestamp,
            cycle: self.cycle,
            mode: self.mode,
            action: self.action,
            resilience_event: self.event_type.clone(),
            reason: self.reason.clone(),
            speed_bps: self.speed_bps,
            threshold_bps: self.threshold_bps,
            rolling_p90_bps: self.rolling_p90_bps,
            connections: self.connections,
        }
    }
}

/// Serializable `watchdog_action` event shape (parity rows E03, E60, L04).
///
/// Field declaration order is the byte-exact JSON field order asserted by the
/// oracle. `paradl-events` will consume this type; this crate never writes to
/// stdout itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatchdogActionEvent {
    /// Always the literal `"watchdog_action"`.
    pub event: &'static str,
    /// `round(time.time(), 3)` at the tick that produced the action.
    pub timestamp: f64,
    /// Recycle cycle number after this tick.
    pub cycle: i64,
    /// Watchdog mode string (`v1`, `v2`, `off`).
    pub mode: WatchdogMode,
    /// Action string (`tier2_pause_unpause`, etc.).
    pub action: WatchdogActionType,
    /// Resilience event type string, or `null`.
    pub resilience_event: Option<String>,
    /// Human-readable reason.
    pub reason: String,
    /// Observed speed in bytes per second.
    pub speed_bps: f64,
    /// Effective threshold in bytes per second.
    pub threshold_bps: f64,
    /// Rolling p90 at this tick.
    pub rolling_p90_bps: f64,
    /// Current connection count.
    pub connections: i32,
}

impl WatchdogActionEvent {
    /// Serialize to the exact JSONL line emitted by the Python oracle.
    ///
    /// Uses Python `json.dumps` default separators (`", "` / `": "`) and a
    /// trailing newline, rather than serde_json's compact separators.
    pub fn to_jsonl_line(&self) -> String {
        fn q(value: &str) -> String {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
        }
        fn n(value: f64) -> String {
            serde_json::to_string(&value).unwrap_or_else(|_| "0.0".to_string())
        }
        let resilience = match &self.resilience_event {
            Some(value) => q(value),
            None => "null".to_string(),
        };
        format!(
            "{{\"event\": {}, \"timestamp\": {}, \"cycle\": {}, \"mode\": {}, \"action\": {}, \"resilience_event\": {}, \"reason\": {}, \"speed_bps\": {}, \"threshold_bps\": {}, \"rolling_p90_bps\": {}, \"connections\": {}}}\n",
            q(self.event),
            n(self.timestamp),
            self.cycle,
            q(self.mode.as_str()),
            q(self.action.as_str()),
            resilience,
            q(&self.reason),
            n(self.speed_bps),
            n(self.threshold_bps),
            n(self.rolling_p90_bps),
            self.connections,
        )
    }
}

/// Immutable snapshot of the controller's runtime state.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchdogState {
    /// Configured mode.
    pub mode: WatchdogMode,
    /// User speed floor in bytes per second.
    pub user_floor_bps: f64,
    /// Active connection count.
    pub connections: i32,
    /// Connection count after clamping the configured value.
    pub initial_connections: i32,
    /// Whether the controller will act on samples.
    pub is_active: bool,
    /// Whether the recycle budget is exhausted (reserved for tier escalation).
    pub exhausted: bool,
    /// Why the controller was disabled, if it was.
    pub disabled_reason: Option<String>,
    /// Number of socket-recycle cycles executed.
    pub cycle_count: i64,
    /// Whether the initial ramp-up finished.
    pub warm_up_complete: bool,
    /// Consecutive low-speed ticks.
    pub low_speed_consecutive: i32,
    /// Consecutive straggler ticks.
    pub straggler_consecutive: i32,
    /// Effective threshold in bytes per second.
    pub current_threshold_bps: f64,
    /// Rolling p90 in bytes per second.
    pub rolling_p90_bps: f64,
    /// Current IP family (`auto`, `ipv4`, `ipv6`).
    pub ip_family: String,
    /// Periodic recycle interval in seconds.
    pub recycle_interval_sec: f64,
    /// Human-readable status label from the Python oracle.
    pub status_label: String,
}

/// Compute a percentile (0..100) by linear interpolation, matching Python
/// `compute_percentile` exactly.
pub fn compute_percentile(samples: &[f64], pct: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let k = (sorted.len() - 1) as f64 * (pct / 100.0);
    let f = k.floor();
    let c = k.ceil();
    if f == c {
        sorted[k as usize]
    } else {
        sorted[f as usize] * (c - k) + sorted[c as usize] * (k - f)
    }
}

/// Python-compatible `round(value, ndigits)` using round-half-to-even.
///
/// Rust's `f64::round_ties_even` gives banker's rounding at exact .5 ties,
/// which is what CPython uses; this matters for the event timestamp at parity
/// row L05.
pub fn py_round(value: f64, ndigits: i32) -> f64 {
    let factor = 10_f64.powi(ndigits);
    (value * factor).round_ties_even() / factor
}

/// Compute a 4-hour bucket (0..5) from an hour of day (0..23), matching Python
/// `compute_hour_bucket(hour)`.
pub fn compute_hour_bucket(hour: i32) -> i32 {
    hour.rem_euclid(24) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_and_rounding_helpers() {
        assert_eq!(compute_percentile(&[], 90.0), 0.0);
        assert_eq!(compute_percentile(&[100.0], 90.0), 100.0);
        assert_eq!(compute_percentile(&[1000.0, 2000.0, 5000.0, 10000.0, 20000.0], 90.0), 16000.0);
        assert_eq!(py_round(2.5, 0), 2.0);
        assert_eq!(py_round(3.5, 0), 4.0);
        assert_eq!(py_round(1763376000.1234, 3), 1763376000.123);
        assert_eq!(compute_hour_bucket(23), 5);
        assert_eq!(compute_hour_bucket(-1), 5);
    }

    #[test]
    fn recycle_actions_are_detected() {
        assert!(WatchdogAction::new(WatchdogActionType::PauseUnpause).is_recycle());
        assert!(WatchdogAction::new(WatchdogActionType::ResilienceRecycle).is_recycle());
        assert!(WatchdogAction::new(WatchdogActionType::AdjustConnections).is_recycle());
        assert!(!WatchdogAction::new(WatchdogActionType::None).is_recycle());
    }
}
