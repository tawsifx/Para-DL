//! Golden contract tests for the single JSONL emitter.
//!
//! These tests are derived from the read-only oracle fixtures in
//! `paradl-oracle/fixtures`. They pin the event-name registry, the exact field
//! set of every event shape, the byte-exact envelope line, and the byte-exact
//! consumption of `paradl_watchdog::WatchdogActionEvent`.

use std::collections::{BTreeMap, BTreeSet};

use paradl_events::{
    Event, ProgressEvent, WatchdogAction, WatchdogActionEvent, WatchdogActionType, emit_jsonl,
    py_round,
};
use serde_json::Value;

const EVENTS_FIXTURE: &str = include_str!("../../paradl-oracle/fixtures/jsonl_events.json");

#[derive(Debug, serde::Deserialize)]
struct Fixture {
    event_count: usize,
    event_shapes: BTreeMap<String, Shape>,
    envelope_line: String,
}

#[derive(Debug, serde::Deserialize)]
struct Shape {
    fields: Vec<String>,
    #[allow(dead_code)] // provenance only, not asserted by the emitter
    sources: Vec<String>,
}

fn fixture() -> Fixture {
    serde_json::from_str(EVENTS_FIXTURE).expect("golden fixture parses")
}

#[test]
fn registry_covers_every_golden_event_name() {
    let golden = fixture();
    assert_eq!(golden.event_count, 43, "oracle fixture count changed");

    let registered: BTreeSet<&str> = Event::all().iter().map(Event::name).collect();
    for name in golden.event_shapes.keys() {
        assert!(registered.contains(name.as_str()), "missing event: {name}");
    }

    // E58 fixes the raw `json.dumps` at throttle/cli.py:296-308; E13 is the
    // resilience event source. Neither is a plain `emit_event` literal in the
    // 43-name fixture, but both must be emitted through this single emitter.
    assert!(registered.contains("throttle_launchd_plist_generated"));
    assert!(registered.contains("resilience_event"));
}

#[test]
fn every_event_matches_golden_field_set() {
    let golden = fixture();
    let mut checked = 0usize;
    for event in Event::all() {
        let Some(shape) = golden.event_shapes.get(event.name()) else {
            continue;
        };
        checked += 1;
        let line = emit_jsonl(&event);
        assert!(line.ends_with('\n'), "{} must end with a newline", event.name());
        let value: Value = serde_json::from_str(line.trim_end())
            .unwrap_or_else(|e| panic!("{} is valid JSON: {e}\n{line}", event.name()));
        let mut actual: BTreeSet<String> = value
            .as_object()
            .expect("event line is a JSON object")
            .keys()
            .cloned()
            .collect();
        assert!(actual.remove("event"));
        assert!(actual.remove("timestamp"));
        let mut expected: BTreeSet<String> = shape.fields.iter().cloned().collect();
        if event.name() == "watchdog_action" {
            // E60 is a union of three shapes. `supports_range` only appears in
            // the early aria2 guard variant; the consumed watchdog type models
            // the full variant. The short variant is covered below through
            // `WatchdogActionRangeEvent`.
            expected.remove("supports_range");
        }
        assert_eq!(actual, expected, "field set for {}", event.name());
        assert_eq!(value["event"], event.name());
        assert!(value.get("timestamp").is_some());
    }
    assert_eq!(checked, golden.event_count, "all golden shapes exercised");
}

#[test]
fn progress_envelope_is_byte_exact() {
    let golden = fixture();
    let event = ProgressEvent::new(
        1763376000.123,
        "demo.bin".to_string(),
        1024,
        Some(2048),
        Some(50.0),
        Some(1000.0),
        Some(1.0),
        Some(16),
        None, // peer_count
        Some("v2".to_string()),
        None, // file_progress
        None, // connection_speeds
    );
    assert_eq!(emit_jsonl(&event), golden.envelope_line);
}

#[test]
fn watchdog_action_event_is_consumed_byte_exactly() {
    let mut action = WatchdogAction::new(WatchdogActionType::PauseUnpause);
    action.timestamp = 1763376000.123;
    action.cycle = 3;
    action.reason = "low speed".to_string();
    action.speed_bps = 500.0;
    action.threshold_bps = 1000.0;
    action.rolling_p90_bps = 1200.0;
    action.connections = 8;

    let typed: WatchdogActionEvent = action.to_event();
    let via_emitter = emit_jsonl(&typed);
    assert_eq!(via_emitter, typed.to_jsonl_line());

    // The enum variant wraps the very same external type.
    assert_eq!(emit_jsonl(&Event::WatchdogAction(typed)), via_emitter);
}

#[test]
fn watchdog_action_short_variant_supports_range() {
    use paradl_events::{Event, WatchdogActionRangeEvent};

    let mut action = WatchdogAction::new(WatchdogActionType::None);
    action.timestamp = 1763376000.123;
    action.reason = "range unsupported".to_string();
    let short = WatchdogActionRangeEvent::new(action.to_event(), Some(false));
    let line = emit_jsonl(&Event::WatchdogActionRange(short));
    let value: Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(value["event"], "watchdog_action");
    assert_eq!(value["supports_range"], false);
}

#[test]
fn timestamp_uses_python_bankers_rounding() {
    assert_eq!(py_round(1763376000.1234, 3), 1763376000.123);
    assert_eq!(py_round(2.5, 0), 2.0);
    assert_eq!(py_round(3.5, 0), 4.0);
}
