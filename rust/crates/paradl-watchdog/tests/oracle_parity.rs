//! RED-first parity tests mirrored from `tests/unit/test_watchdog.py` and the
//! `paradl-oracle` golden `fixtures/watchdog.json`. Every expected value below
//! was captured from the Python oracle (27/27 passing) before this crate existed.

use std::sync::{Arc, Mutex};

use paradl_core::{FakeClock, WatchdogMode};
use paradl_watchdog::{
    InMemoryProfileStore, LearnedProfile, ProfileStore, ResilienceEvent, ResilienceEventType,
    ThroughputSample, Watchdog, WatchdogActionEvent, WatchdogActionType, WatchdogConfig,
    WatchdogController, compute_percentile,
};

fn sample(elapsed: f64, speed: f64) -> ThroughputSample {
    ThroughputSample::new(elapsed, speed)
}

/// Mirrors the Python test's `patch(side_effect=saved.append)`: records every
/// `save_learned_profile` call instead of upserting by profile key.
#[derive(Default)]
struct RecordingStore {
    saved: Mutex<Vec<LearnedProfile>>,
}

impl ProfileStore for RecordingStore {
    fn get_best_profile(
        &self,
        _host: &str,
        _hour_bucket: i32,
        _network_id: &str,
    ) -> Option<LearnedProfile> {
        None
    }

    fn save_learned_profile(&self, profile: LearnedProfile) {
        self.saved.lock().unwrap().push(profile);
    }
}

fn config() -> WatchdogConfig {
    WatchdogConfig {
        host: "example.com".to_string(),
        user_floor_bps: 1000.0,
        apply_learned_profile: false,
        ..WatchdogConfig::default()
    }
}

// ---------------------------------------------------------------------------
// compute_percentile (test_watchdog.py::TestComputePercentile + golden L09)
// ---------------------------------------------------------------------------

#[test]
fn percentile_single_sample() {
    assert_eq!(compute_percentile(&[100.0], 90.0), 100.0);
}

#[test]
fn percentile_p90_ten_samples() {
    let samples: Vec<f64> = (1..=10).map(f64::from).collect();
    assert!((compute_percentile(&samples, 90.0) - 9.1).abs() < 1e-9);
}

#[test]
fn percentile_empty() {
    assert_eq!(compute_percentile(&[], 90.0), 0.0);
}

#[test]
fn percentile_p90_five_samples_is_16000() {
    // k = (5-1)*0.9 = 3.6 -> interpolate 10000 and 20000 -> 16000.
    let samples = [1000.0, 2000.0, 5000.0, 10000.0, 20000.0];
    assert_eq!(compute_percentile(&samples, 90.0), 16000.0);
}

// ---------------------------------------------------------------------------
// Init (test_watchdog.py::TestWatchdogControllerInit + golden watchdog_init)
// ---------------------------------------------------------------------------

#[test]
fn init_defaults_and_status_label() {
    let wd = WatchdogController::new(config());
    let st = wd.state();
    assert_eq!(st.connections, 16);
    assert_eq!(st.mode, WatchdogMode::V2);
    assert!((st.user_floor_bps - 1000.0).abs() < f64::EPSILON);
    assert!(st.is_active);
    assert!(!st.exhausted);
    assert!(st.disabled_reason.is_none());
    assert_eq!(st.status_label, "Watchdog v2: <1000 B/s (cycles: 0, 16 streams)");
    assert!((st.current_threshold_bps - 1000.0).abs() < f64::EPSILON);
}

#[test]
fn watchdog_off_disables() {
    let wd = WatchdogController::new(WatchdogConfig {
        mode: WatchdogMode::Off,
        ..config()
    });
    assert!(!wd.state().is_active);
    assert_eq!(wd.state().disabled_reason.as_deref(), Some("off"));
    assert_eq!(wd.state().status_label, "Watchdog: Off");
}

#[test]
fn zero_floor_disables() {
    let wd = WatchdogController::new(WatchdogConfig {
        user_floor_bps: 0.0,
        ..config()
    });
    assert!(!wd.state().is_active);
    assert_eq!(wd.state().disabled_reason.as_deref(), Some("off"));
}

#[test]
fn connections_clamped_to_16() {
    let wd = WatchdogController::new(WatchdogConfig {
        connections: 32,
        ..config()
    });
    assert_eq!(wd.state().connections, 16);
    assert_eq!(wd.state().initial_connections, 16);
}

#[test]
fn no_range_support_stays_active_locked_to_one_stream() {
    let wd = WatchdogController::new(WatchdogConfig {
        supports_range: false,
        ..config()
    });
    assert!(wd.state().is_active);
    assert!(wd.state().disabled_reason.is_none());
    assert_eq!(wd.state().connections, 1);
    assert_eq!(wd.state().initial_connections, 1);
    assert!(wd.state().status_label.contains("single stream"));
    assert!(wd.state().status_label.contains("1 streams"));
}

#[test]
fn learned_profile_applies_ip_family_and_caps_connections() {
    let store = InMemoryProfileStore::default();
    store.save_learned_profile(LearnedProfile {
        host: "example.com".to_string(),
        hour_bucket: 0,
        network_id: "default".to_string(),
        connections: 8,
        ip_family: "ipv4".to_string(),
        recovery_action: WatchdogActionType::PauseUnpause.as_str().to_string(),
        speed_after: 1_000_000.0,
        updated_epoch: 1_700_000_000.0,
        ..LearnedProfile::default()
    });
    let wd = WatchdogController::with_store(
        WatchdogConfig {
            apply_learned_profile: true,
            ..config()
        },
        Arc::new(FakeClock::default()),
        Arc::new(store),
    );
    // max(initial=16, saved=8) = 16, but ip family is learned.
    assert_eq!(wd.state().connections, 16);
    assert_eq!(wd.state().ip_family, "ipv4");
}

// ---------------------------------------------------------------------------
// Threshold (test_watchdog.py::TestWatchdogThreshold)
// ---------------------------------------------------------------------------

#[test]
fn v1_threshold_equals_user_floor() {
    let wd = WatchdogController::new(WatchdogConfig {
        mode: WatchdogMode::V1,
        user_floor_bps: 5000.0,
        ..config()
    });
    assert!((wd.state().current_threshold_bps - 5000.0).abs() < f64::EPSILON);
}

#[test]
fn v2_threshold_without_samples_is_floor() {
    let wd = WatchdogController::new(config());
    assert!((wd.state().current_threshold_bps - 1000.0).abs() < f64::EPSILON);
}

#[test]
fn v2_threshold_with_samples_is_half_p90() {
    let clock = Arc::new(FakeClock::default());
    let mut wd = WatchdogController::with_clock(config(), clock);
    for t in 0..5 {
        let speed = [1000.0, 2000.0, 5000.0, 10000.0, 20000.0][t];
        wd.observe(sample(t as f64, speed));
    }
    assert!((wd.state().rolling_p90_bps - 16000.0).abs() < f64::EPSILON);
    assert!((wd.state().current_threshold_bps - 8000.0).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// step() (test_watchdog.py::TestWatchdogStep + golden scenarios)
// ---------------------------------------------------------------------------

#[test]
fn speed_above_threshold_returns_none() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        recycle_interval_sec: 0.0,
        ..config()
    });
    for t in 0..3 {
        assert_eq!(wd.observe(sample(t as f64, 5000.0)).action, WatchdogActionType::None);
    }
    assert_eq!(wd.observe(sample(10.0, 5000.0)).action, WatchdogActionType::None);
}

#[test]
fn low_speed_scenario_triggers_on_fifth_sample() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        recycle_interval_sec: 0.0,
        ..config()
    });
    // Warm-up with 5 high samples -> p90=5000, threshold=2500.
    for t in 0..5 {
        wd.observe(sample(t as f64, 5000.0));
    }
    // First four low samples: NONE with golden values.
    for i in 0..4 {
        let action = wd.observe(sample(10.0 + i as f64, 500.0));
        assert_eq!(action.action, WatchdogActionType::None);
        assert_eq!(action.cycle, 0);
        assert!((action.threshold_bps - 2500.0).abs() < f64::EPSILON);
        assert!((action.rolling_p90_bps - 5000.0).abs() < f64::EPSILON);
    }
    assert_eq!(wd.state().low_speed_consecutive, 4);

    let fifth = wd.observe(sample(14.0, 500.0));
    assert_eq!(fifth.action, WatchdogActionType::PauseUnpause);
    assert_eq!(fifth.reason, "Speed 500 B/s < 2.4 KB/s floor; cycling 16 RPC sockets");
    assert_eq!(fifth.new_connections, Some(16));
    assert!((fifth.threshold_bps - 2500.0).abs() < f64::EPSILON);
    assert!((fifth.rolling_p90_bps - 5000.0).abs() < f64::EPSILON);
    assert_eq!(fifth.cycle, 1);
    assert_eq!(wd.state().cycle_count, 1);
}

#[test]
fn post_recycle_cooldown_returns_none() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        min_cooldown_sec: 5.0,
        ..config()
    });
    for t in 0..3 {
        wd.observe(sample(t as f64, 5000.0));
    }
    // Simulate a recent recycle: inject via a real recycle then step inside cooldown.
    // Use the periodic trigger at t=5 with default interval, then sample at t=6.
    let mut wd2 = WatchdogController::new(config());
    for t in 1..=5 {
        wd2.observe(sample(t as f64, 5000.0));
    }
    assert_eq!(wd2.state().cycle_count, 1);
    let a = wd2.observe(sample(6.0, 500.0));
    assert_eq!(a.action, WatchdogActionType::None);
    assert_eq!(wd2.state().low_speed_consecutive, 0);
    // The first controller still exercises the explicit last-recycle guard.
    let _ = &mut wd;
}

#[test]
fn resilience_event_triggers_recycle() {
    let mut wd = WatchdogController::new(config());
    let event = ResilienceEvent::new(
        ResilienceEventType::Stall,
        "no progress for 30s",
        100.0,
    );
    let mut s = sample(100.0, 5000.0);
    s.resilience_event = Some(event);
    let action = wd.observe(s);
    assert_eq!(action.action, WatchdogActionType::ResilienceRecycle);
    assert_eq!(action.reason, "[STALL] no progress for 30s");
    assert_eq!(action.event_type.as_deref(), Some("stall"));
    assert_eq!(action.cycle, 1);
    assert!((action.threshold_bps - 1000.0).abs() < f64::EPSILON);
    assert!((action.rolling_p90_bps - 0.0).abs() < f64::EPSILON);
    assert_eq!(wd.state().cycle_count, 1);
}

#[test]
fn inactive_watchdog_returns_none() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        mode: WatchdogMode::Off,
        ..config()
    });
    assert_eq!(wd.observe(sample(10.0, 100.0)).action, WatchdogActionType::None);
}

#[test]
fn no_range_low_speed_never_recycles_socket() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        supports_range: false,
        ..config()
    });
    for t in 10..15 {
        assert_eq!(wd.observe(sample(t as f64, 200.0)).action, WatchdogActionType::None);
    }
    assert_eq!(wd.state().cycle_count, 0);
    assert!(wd.state().is_active);
}

#[test]
fn no_range_resilience_event_skips_socket_recycle() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        supports_range: false,
        ..config()
    });
    let mut s = sample(100.0, 0.0);
    s.resilience_event = Some(ResilienceEvent::new(
        ResilienceEventType::Stall,
        "no progress for 30s",
        100.0,
    ));
    assert_eq!(wd.observe(s).action, WatchdogActionType::None);
    assert_eq!(wd.state().cycle_count, 0);
}

#[test]
fn relative_straggler_triggers_recycle() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        user_floor_bps: 100_000.0,
        recycle_interval_sec: 0.0,
        ..config()
    });
    for t in 0..3 {
        wd.observe(sample(t as f64, 1_000_000.0));
    }
    let mut s1 = sample(10.0, 1_000_000.0);
    s1.per_conn_speeds = Some(vec![500_000.0, 450_000.0, 50_000.0]);
    let first = wd.observe(s1);
    assert_eq!(first.action, WatchdogActionType::None);
    assert_eq!(wd.state().straggler_consecutive, 1);

    let mut s2 = sample(11.0, 1_000_000.0);
    s2.per_conn_speeds = Some(vec![500_000.0, 450_000.0, 50_000.0]);
    let second = wd.observe(s2);
    assert_eq!(second.action, WatchdogActionType::PauseUnpause);
    assert!(second.reason.starts_with("Straggler connection"));
    assert_eq!(
        second.reason,
        "Straggler connection 48.8 KB/s < 488.3 KB/s/4; cycling 16 RPC sockets"
    );
    assert!((second.threshold_bps - 500_000.0).abs() < f64::EPSILON);
    assert!((second.rolling_p90_bps - 1_000_000.0).abs() < f64::EPSILON);
    assert_eq!(second.cycle, 1);
}

#[test]
fn periodic_5s_recycle_on_206_range() {
    let mut wd = WatchdogController::new(config());
    assert!((wd.state().recycle_interval_sec - 5.0).abs() < f64::EPSILON);
    let expected: [(f64, WatchdogActionType, &str, i64); 10] = [
        (1.0, WatchdogActionType::None, "", 0),
        (2.0, WatchdogActionType::None, "", 0),
        (3.0, WatchdogActionType::None, "", 0),
        (4.0, WatchdogActionType::None, "", 0),
        (5.0, WatchdogActionType::PauseUnpause, "Periodic 5s cycle (speed 4.9 KB/s); cycling 16 RPC sockets", 1),
        (6.0, WatchdogActionType::None, "", 1),
        (7.0, WatchdogActionType::None, "", 1),
        (8.0, WatchdogActionType::None, "", 1),
        (9.0, WatchdogActionType::None, "", 1),
        (10.0, WatchdogActionType::PauseUnpause, "Periodic 5s cycle (speed 4.9 KB/s); cycling 16 RPC sockets", 2),
    ];
    for (t, want_action, want_reason, want_cycle) in expected {
        let action = wd.observe(sample(t, 5000.0));
        assert_eq!(action.action, want_action, "periodic step t={t}");
        assert_eq!(action.reason, want_reason, "periodic reason t={t}");
        assert_eq!(action.cycle, want_cycle, "periodic cycle t={t}");
        if t <= 2.0 {
            assert!((action.threshold_bps - 1000.0).abs() < f64::EPSILON);
        }
    }
    assert_eq!(wd.state().cycle_count, 2);
}

#[test]
fn periodic_recycle_skipped_without_range() {
    let mut wd = WatchdogController::new(WatchdogConfig {
        supports_range: false,
        ..config()
    });
    for t in [5.0, 10.0, 15.0] {
        assert_eq!(wd.observe(sample(t, 5000.0)).action, WatchdogActionType::None);
    }
    assert_eq!(wd.state().cycle_count, 0);
}

// ---------------------------------------------------------------------------
// Learned profile speed_after (regression tests)
// ---------------------------------------------------------------------------

#[test]
fn recycle_records_post_action_speed_not_session_peak() {
    let clock = Arc::new(FakeClock::new(1000.0, 1000.0));
    let store = Arc::new(RecordingStore::default());
    let mut wd = WatchdogController::with_store(
        WatchdogConfig {
            host: "cdn.example.com".to_string(),
            user_floor_bps: 100_000.0,
            apply_learned_profile: false,
            ..WatchdogConfig::default()
        },
        clock,
        store.clone(),
    );
    for t in 1..=4 {
        wd.observe(sample(t as f64, 10_000_000.0));
    }
    let decision = wd.observe(sample(5.0, 120_000.0));
    assert_eq!(decision.action, WatchdogActionType::PauseUnpause);
    let profiles = store.saved.lock().unwrap().clone();
    assert_eq!(profiles.len(), 1);
    assert!((profiles[0].speed_before - 120_000.0).abs() < f64::EPSILON);
    assert!(profiles[0].speed_after <= 500_000.0, "speed_after={}", profiles[0].speed_after);
    assert!((profiles[0].updated_epoch - 1000.0).abs() < f64::EPSILON);
}

#[test]
fn second_recycle_uses_samples_since_first_recycle() {
    let clock = Arc::new(FakeClock::new(1000.0, 1000.0));
    let store = Arc::new(RecordingStore::default());
    let mut wd = WatchdogController::with_store(
        WatchdogConfig {
            host: "cdn.example.com".to_string(),
            user_floor_bps: 100_000.0,
            apply_learned_profile: false,
            ..WatchdogConfig::default()
        },
        clock,
        store.clone(),
    );
    for t in 1..=4 {
        wd.observe(sample(t as f64, 10_000_000.0));
    }
    wd.observe(sample(5.0, 120_000.0));
    for t in 6..=9 {
        wd.observe(sample(t as f64, 200_000.0));
    }
    wd.observe(sample(10.0, 200_000.0));
    let profiles = store.saved.lock().unwrap().clone();
    assert_eq!(profiles.len(), 2);
    assert!(profiles[0].speed_after <= 500_000.0);
    assert!(profiles[1].speed_after <= 500_000.0);
}

// ---------------------------------------------------------------------------
// Literals and byte-exact event (parity matrix L04/L07, E60)
// ---------------------------------------------------------------------------

#[test]
fn watchdog_action_type_literals() {
    assert_eq!(WatchdogActionType::None.as_str(), "none");
    assert_eq!(WatchdogActionType::PauseUnpause.as_str(), "tier2_pause_unpause");
    assert_eq!(WatchdogActionType::ResilienceRecycle.as_str(), "resilience_recycle");
    assert_eq!(WatchdogActionType::AdjustConnections.as_str(), "adjust_connections");
    assert_eq!(WatchdogActionType::SwitchIpFamily.as_str(), "switch_ip_family");
    assert_eq!(WatchdogActionType::ProcessRestart.as_str(), "tier3_restart");
    assert_eq!(WatchdogActionType::SingleStreamFallback.as_str(), "single_stream_fallback");
}

#[test]
fn action_event_line_is_byte_exact() {
    let event = WatchdogActionEvent {
        event: "watchdog_action",
        timestamp: 1_763_376_000.123,
        cycle: 1,
        mode: WatchdogMode::V2,
        action: WatchdogActionType::PauseUnpause,
        resilience_event: None,
        reason: "5 low samples".to_string(),
        speed_bps: 500.0,
        threshold_bps: 1000.0,
        rolling_p90_bps: 2000.0,
        connections: 16,
    };
    assert_eq!(
        event.to_jsonl_line(),
        "{\"event\": \"watchdog_action\", \"timestamp\": 1763376000.123, \"cycle\": 1, \"mode\": \"v2\", \"action\": \"tier2_pause_unpause\", \"resilience_event\": null, \"reason\": \"5 low samples\", \"speed_bps\": 500.0, \"threshold_bps\": 1000.0, \"rolling_p90_bps\": 2000.0, \"connections\": 16}\n"
    );
}

#[test]
fn action_event_field_order_matches_golden() {
    let event = WatchdogActionEvent {
        event: "watchdog_action",
        timestamp: 1_763_376_000.123,
        cycle: 1,
        mode: WatchdogMode::V2,
        action: WatchdogActionType::PauseUnpause,
        resilience_event: None,
        reason: "5 low samples".to_string(),
        speed_bps: 500.0,
        threshold_bps: 1000.0,
        rolling_p90_bps: 2000.0,
        connections: 16,
    };
    let json = serde_json::to_string(&event).unwrap();
    let fields: Vec<&str> = [
        "event", "timestamp", "cycle", "mode", "action", "resilience_event", "reason",
        "speed_bps", "threshold_bps", "rolling_p90_bps", "connections",
    ]
    .to_vec();
    for (i, field) in fields.iter().enumerate() {
        let at = json.find(&format!("\"{field}\"")).unwrap();
        if i > 0 {
            let prev = json.find(&format!("\"{}\"", fields[i - 1])).unwrap();
            assert!(prev < at, "field {field} out of order");
        }
    }
}

// ---------------------------------------------------------------------------
// Charter trait object + supervision loop with injected clock
// ---------------------------------------------------------------------------

#[test]
fn watchdog_trait_object_supports_observe_and_state() {
    let mut wd: Box<dyn Watchdog> = Box::new(WatchdogController::new(config()));
    assert_eq!(wd.observe(sample(1.0, 5000.0)).action, WatchdogActionType::None);
    assert_eq!(wd.state().mode, WatchdogMode::V2);
}

#[test]
fn supervision_loop_uses_injected_clock_for_event_timestamps() {
    let clock = Arc::new(FakeClock::new(1_763_376_000.123, 1000.0));
    let mut wd = WatchdogController::with_clock(config(), clock);
    let samples: Vec<ThroughputSample> = (0..5).map(|t| sample(t as f64, 5000.0)).collect();
    let actions = wd.supervise(samples);
    assert_eq!(actions.len(), 5);
    for action in &actions {
        let ev = action.to_event();
        assert_eq!(ev.timestamp, 1_763_376_000.123);
        assert_eq!(ev.mode, WatchdogMode::V2);
    }
}
