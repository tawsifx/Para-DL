//! Golden assertions mirrored from `tests/unit/test_watchdog.py`, including the exact
//! `watchdog_action` JSONL line and the `tier2_pause_unpause` action string.

use crate::fixtures::fixture;

#[test]
fn golden_watchdog_action_type_literals() {
    let g = fixture("watchdog.json");
    let t = &g["watchdog_action_types"];
    let expect = [
        ("NONE", "none"),
        ("PAUSE_UNPAUSE", "tier2_pause_unpause"),
        ("RESILIENCE_RECYCLE", "resilience_recycle"),
        ("ADJUST_CONNECTIONS", "adjust_connections"),
        ("SWITCH_IP_FAMILY", "switch_ip_family"),
        ("PROCESS_RESTART", "tier3_restart"),
        ("SINGLE_STREAM_FALLBACK", "single_stream_fallback"),
    ];
    for (name, val) in expect {
        assert_eq!(t[name].as_str().unwrap(), val, "WatchdogActionType::{name}");
    }
}

#[test]
fn golden_compute_percentile() {
    let g = fixture("watchdog.json");
    let p = &g["compute_percentile"];
    assert_eq!(p["single"].as_f64().unwrap(), 100.0);
    assert_eq!(p["p90_10"].as_f64().unwrap(), 9.1);
    assert_eq!(p["empty"].as_f64().unwrap(), 0.0);
    // 5 samples: k = 3.6 -> interpolate 10000 and 20000 -> 16000 (Python linear interp).
    assert_eq!(p["p90_5"].as_f64().unwrap(), 16000.0);
}

#[test]
fn golden_watchdog_low_speed_scenario() {
    let g = fixture("watchdog.json");
    let s = &g["watchdog_low_speed_scenario"];
    let first = s["first_four"].as_array().unwrap();
    assert_eq!(first.len(), 4);
    for d in first {
        assert_eq!(d["action"], "none");
        assert_eq!(d["reason"], "");
        assert_eq!(d["cycle"].as_i64().unwrap(), 0);
    }
    let fifth = &s["fifth"];
    assert_eq!(fifth["action"], "tier2_pause_unpause");
    assert_eq!(
        fifth["reason"].as_str().unwrap(),
        "Speed 500 B/s < 2.4 KB/s floor; cycling 16 RPC sockets"
    );
    assert_eq!(fifth["new_connections"].as_i64().unwrap(), 16);
    assert_eq!(fifth["threshold_bps"].as_f64().unwrap(), 2500.0);
    assert_eq!(fifth["rolling_p90_bps"].as_f64().unwrap(), 5000.0);
    assert_eq!(fifth["cycle"].as_i64().unwrap(), 1);
    assert_eq!(s["low_speed_consecutive_after_4"].as_i64().unwrap(), 4);
    assert_eq!(s["cycle_count_after_5th"].as_i64().unwrap(), 1);
}

#[test]
fn golden_watchdog_straggler_scenario() {
    let g = fixture("watchdog.json");
    let s = &g["watchdog_straggler_scenario"];
    assert_eq!(s["first"]["action"], "none");
    let second = &s["second"];
    assert_eq!(second["action"], "tier2_pause_unpause");
    assert_eq!(
        second["reason"].as_str().unwrap(),
        "Straggler connection 48.8 KB/s < 488.3 KB/s/4; cycling 16 RPC sockets"
    );
    assert_eq!(second["threshold_bps"].as_f64().unwrap(), 500000.0);
    assert_eq!(second["rolling_p90_bps"].as_f64().unwrap(), 1000000.0);
    assert_eq!(second["cycle"].as_i64().unwrap(), 1);
    assert_eq!(s["straggler_consecutive_after_first"].as_i64().unwrap(), 0);
    assert_eq!(s["cycle_count"].as_i64().unwrap(), 1);
}

#[test]
fn golden_watchdog_periodic_5s_recycle() {
    let g = fixture("watchdog.json");
    let s = &g["watchdog_periodic_scenario"];
    let steps = s["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 10);
    let recycle_reason = "Periodic 5s cycle (speed 4.9 KB/s); cycling 16 RPC sockets";
    for (i, step) in steps.iter().enumerate() {
        let t = step["t"].as_f64().unwrap();
        let want_action = if i == 4 || i == 9 {
            "tier2_pause_unpause"
        } else {
            "none"
        };
        assert_eq!(step["action"], want_action, "periodic step t={t}");
        if i == 4 {
            assert_eq!(step["reason"], recycle_reason);
            assert_eq!(step["cycle"].as_i64().unwrap(), 1);
        }
        if i == 9 {
            assert_eq!(step["reason"], recycle_reason);
            assert_eq!(step["cycle"].as_i64().unwrap(), 2);
        }
        if i == 0 || i == 1 {
            assert_eq!(step["threshold_bps"].as_f64().unwrap(), 1000.0);
        }
    }
    assert_eq!(s["cycle_count"].as_i64().unwrap(), 2);
}

#[test]
fn golden_watchdog_resilience_recycle() {
    let g = fixture("watchdog.json");
    let d = &g["watchdog_resilience_scenario"];
    assert_eq!(d["action"], "resilience_recycle");
    assert_eq!(d["reason"], "[STALL] no progress for 30s");
    assert_eq!(d["event_type"], "stall");
    assert_eq!(d["cycle"].as_i64().unwrap(), 1);
    assert_eq!(d["threshold_bps"].as_f64().unwrap(), 1000.0);
}

#[test]
fn golden_watchdog_initial_state_and_status_label() {
    let g = fixture("watchdog.json");
    let d = &g["watchdog_init"];
    assert_eq!(d["connections"].as_i64().unwrap(), 16);
    assert_eq!(d["mode"], "v2");
    assert_eq!(d["user_floor_bps"].as_i64().unwrap(), 1000);
    assert!(d["is_active"].as_bool().unwrap());
    assert!(!d["exhausted"].as_bool().unwrap());
    assert!(d["disabled_reason"].is_null());
    assert_eq!(
        d["status_label"].as_str().unwrap(),
        "Watchdog v2: <1000 B/s (cycles: 0, 16 streams)"
    );
    assert_eq!(d["current_threshold_bps"].as_i64().unwrap(), 1000);
}

#[test]
fn golden_watchdog_action_jsonl_line() {
    let g = fixture("watchdog.json");
    let line = g["watchdog_action_event_line"].as_str().unwrap();
    assert_eq!(
        line,
        "{\"event\": \"watchdog_action\", \"timestamp\": 1763376000.123, \"cycle\": 1, \"mode\": \"v2\", \"action\": \"tier2_pause_unpause\", \"resilience_event\": null, \"reason\": \"5 low samples\", \"speed_bps\": 500.0, \"threshold_bps\": 1000.0, \"rolling_p90_bps\": 2000.0, \"connections\": 16}\n"
    );
    let fields: Vec<&str> = g["watchdog_action_event_fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        vec![
            "event",
            "timestamp",
            "cycle",
            "mode",
            "action",
            "resilience_event",
            "reason",
            "speed_bps",
            "threshold_bps",
            "rolling_p90_bps",
            "connections",
        ]
    );
}

/// Contract test for the future `paradl-watchdog` crate (W1-3).
#[test]
#[ignore = "depends on paradl-watchdog (W1-3): remove ignore once the crate exists"]
fn contract_watchdog_decisions_match_golden() {
    let g = fixture("watchdog.json");
    assert_eq!(
        g["watchdog_action_types"]["PAUSE_UNPAUSE"],
        "tier2_pause_unpause"
    );
    unimplemented!("paradl-watchdog (W1-3) not yet implemented");
}
