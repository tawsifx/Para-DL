//! Typed, serde-serializable event payloads for the single JSONL emitter.
//!
//! **Single-emitter rule (master ruling 2):** this module never writes to
//! stdout. It only builds typed values whose JSON shape is
//! `{ "event": <name>, "timestamp": round(time.time(), 3), ...redacted }`.
//! W2-4's `paradl-events` crate is the sole emitter and will consume these
//! structs, apply central redaction, and write the single JSONL stream.
//!
//! In particular, the Python `throttle/cli.py:296-308`
//! `throttle_launchd_plist_generated` branch wrote raw `json.dumps` with no
//! timestamp and no redaction. [`ThrottleLaunchdPlistGenerated`] replaces that
//! raw dict with a typed payload that carries the required envelope fields.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::numeric::py_round;

fn now_timestamp() -> f64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    py_round(secs, 3)
}

/// The common `{event, timestamp, ...payload}` envelope.
///
/// `timestamp` uses Python's `round(time.time(), 3)` semantics (parity row L04).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThrottleEvent<T: Serialize> {
    /// Stable event name.
    pub event: String,
    /// Unix timestamp rounded to three decimals.
    pub timestamp: f64,
    /// Event-specific, already-redacted fields.
    #[serde(flatten)]
    pub payload: T,
}

impl<T: Serialize> ThrottleEvent<T> {
    /// Attach a caller-supplied timestamp (useful for deterministic tests).
    pub fn with_timestamp(event: impl Into<String>, timestamp: f64, payload: T) -> Self {
        Self {
            event: event.into(),
            timestamp,
            payload,
        }
    }

    /// Serialize to a JSON string without printing it.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("typed throttle event is always serializable")
    }
}

/// Payload for the `throttle_launchd_plist_generated` event (E58).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ThrottleLaunchdPlistGenerated {
    /// Path of the generated LaunchAgent plist.
    pub path: String,
    /// LaunchAgent label.
    pub label: String,
    /// `plistlint` validation message.
    pub lint: String,
}

/// Build a typed `throttle_launchd_plist_generated` event.
pub fn launchd_plist_generated(
    path: impl Into<String>,
    label: impl Into<String>,
    lint: impl Into<String>,
) -> ThrottleEvent<ThrottleLaunchdPlistGenerated> {
    ThrottleEvent::with_timestamp(
        "throttle_launchd_plist_generated",
        now_timestamp(),
        ThrottleLaunchdPlistGenerated {
            path: path.into(),
            label: label.into(),
            lint: lint.into(),
        },
    )
}

/// Payload for the `profiles` event (E44).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProfilesPayload {
    /// Learned per-host profiles.
    pub profiles: Vec<String>,
    /// Number of profiles.
    pub count: usize,
}

/// Build a typed `profiles` event.
pub fn profiles(profiles: Vec<String>) -> ThrottleEvent<ProfilesPayload> {
    let count = profiles.len();
    ThrottleEvent::with_timestamp(
        "profiles",
        now_timestamp(),
        ProfilesPayload { profiles, count },
    )
}

/// Payload for the `profiles_reset` event (E45).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ProfilesResetPayload {
    /// Host whose profile was cleared.
    pub host: String,
    /// Whether any data was cleared.
    pub cleared: bool,
}

/// Build a typed `profiles_reset` event.
pub fn profiles_reset(
    host: impl Into<String>,
    cleared: bool,
) -> ThrottleEvent<ProfilesResetPayload> {
    ThrottleEvent::with_timestamp(
        "profiles_reset",
        now_timestamp(),
        ProfilesResetPayload {
            host: host.into(),
            cleared,
        },
    )
}

/// Redact common secret-bearing substrings from a value before it is emitted.
///
/// This is a defensive helper for callers that build custom payloads; the
/// central `paradl-events` redactor remains authoritative.
pub fn redact(value: &str) -> String {
    let mut out = value.to_owned();
    for marker in [
        "token=",
        "password=",
        "authorization:",
        "proxy-authorization:",
    ] {
        let mut start = 0;
        while let Some(found) = out[start..].to_ascii_lowercase().find(marker) {
            let begin = start + found + marker.len();
            let end = out[begin..]
                .find(|c: char| c.is_whitespace() || c == ',' || c == '&' || c == '#')
                .map(|n| begin + n)
                .unwrap_or(out.len());
            out.replace_range(begin..end, "<redacted>");
            start = begin + "<redacted>".len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchd_event_has_timestamp_and_redacted_shape() {
        let event = ThrottleEvent::with_timestamp(
            "throttle_launchd_plist_generated",
            1.234,
            ThrottleLaunchdPlistGenerated {
                path: "/tmp/x".to_owned(),
                label: "label".to_owned(),
                lint: "ok".to_owned(),
            },
        );
        let json = event.to_json();
        assert!(json.contains("\"event\":\"throttle_launchd_plist_generated\""));
        assert!(json.contains("\"timestamp\":1.234"));
        assert!(json.contains("\"label\":\"label\""));
    }

    #[test]
    fn redaction_removes_token_values() {
        assert_eq!(redact("url?token=SECRET&x=1"), "url?token=<redacted>&x=1");
    }
}
