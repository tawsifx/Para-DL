//! Golden assertions for the JSONL event corpus (`paradl-events` single emitter) and redaction.

use crate::fixtures::fixture;

#[test]
fn golden_jsonl_envelope_exact_line() {
    let g = fixture("jsonl_events.json");
    assert_eq!(
        g["envelope_line"].as_str().unwrap(),
        "{\"event\": \"progress\", \"timestamp\": 1763376000.123, \"title\": \"demo.bin\", \"downloaded_bytes\": 1024, \"total_bytes\": 2048, \"percent\": 50.0, \"speed_bps\": 1000.0, \"eta_seconds\": 1.0, \"connections\": 16, \"watchdog_state\": \"v2\"}\n"
    );
    // timestamp = round(time.time(), 3): Python banker's rounding of 1763376000.1234 -> .123
    assert!(
        g["envelope_line"]
            .as_str()
            .unwrap()
            .contains("\"timestamp\": 1763376000.123")
    );
}

#[test]
fn golden_all_event_shapes_and_field_order() {
    let g = fixture("jsonl_events.json");
    let shapes = &g["event_shapes"];
    assert_eq!(g["event_count"].as_i64().unwrap(), 43);
    assert_eq!(shapes.as_object().unwrap().len(), 43);

    // Every emitted event's payload field list, captured by AST from the oracle call sites.
    let expected: &[(&str, &[&str])] = &[
        ("api_server_started", &["host", "port"]),
        ("auth_remove", &["service", "success"]),
        ("auth_set", &["service", "success"]),
        ("auth_status", &["services"]),
        ("bench", &["url", "duration_limit_sec", "results"]),
        ("clean", &["directory", "removed"]),
        ("config", &["config"]),
        ("config_reset", &["config"]),
        ("config_updated", &["key", "value"]),
        ("deferred", &["reason", "start_at", "wait_seconds"]),
        ("diagnose", &[]),
        ("doctor", &[]),
        (
            "error",
            &[
                "message",
                "reason",
                "status_code",
                "target",
                "measured_bps",
                "threshold_bps",
            ],
        ),
        (
            "hashes",
            &["file", "filename", "size", "duration", "hashes"],
        ),
        (
            "history",
            &["entries", "count", "search", "target_type", "success"],
        ),
        ("hook_error", &["hook", "error"]),
        ("hook_executed", &["hook", "success"]),
        ("hook_failed", &["hook", "exit_code", "stderr"]),
        ("hook_timeout", &["hook", "timeout_seconds"]),
        ("hosts_reset", &["cleared"]),
        ("info", &["target", "properties"]),
        (
            "install_security_check",
            &[
                "file",
                "codesign_valid",
                "codesign_detail",
                "gatekeeper_accepted",
                "gatekeeper_detail",
                "verified_by_pdl",
            ],
        ),
        ("interrupted", &["signal", "exit_code"]),
        (
            "launchd_plist_generated",
            &["path", "label", "logs_dir", "lint"],
        ),
        ("mirror_dropped", &["mirror_url", "failures", "reason"]),
        (
            "mirror_rejected",
            &["mirror_url", "reason", "expected_length", "mirror_length"],
        ),
        (
            "netprobe",
            &[
                "url",
                "host",
                "supports_range",
                "ipv6_DNS",
                "throttle_class",
                "recommended_connections",
                "recommended_action",
                "verdict",
                "http3_supported",
                "http3_detail",
                "passes",
            ],
        ),
        ("profiles", &["profiles", "count"]),
        ("profiles_reset", &["host", "cleared"]),
        (
            "progress",
            &[
                "title",
                "downloaded_bytes",
                "total_bytes",
                "percent",
                "speed_bps",
                "eta_seconds",
                "connections",
                "peer_count",
                "watchdog_state",
                "file_progress",
                "connection_speeds",
            ],
        ),
        ("queue_crash_recovered", &["recovered_count"]),
        (
            "queue_job_added",
            &[
                "job_id",
                "target",
                "from_history",
                "priority",
                "require_ac",
                "when_fast_bps",
                "start_at",
            ],
        ),
        (
            "queue_job_deferred",
            &[
                "job_id",
                "reason",
                "start_at",
                "measured_bps",
                "threshold_bps",
            ],
        ),
        (
            "queue_job_started",
            &["job_id", "target", "priority", "connections"],
        ),
        ("queue_list", &["jobs"]),
        ("queue_paused", &["paused"]),
        ("queue_removed", &["removed"]),
        ("queue_resumed", &["resumed", "all"]),
        (
            "result",
            &[
                "subcommand",
                "target",
                "success",
                "file_path",
                "checksum_verified",
                "error",
                "exit_code",
                "target_type",
                "bytes_downloaded",
                "duration",
                "avg_speed_bps",
                "recycle_count",
                "audio_format_produced",
                "checksum_algo",
                "checksum_digest",
                "adb_synced",
            ],
        ),
        (
            "target_classified",
            &[
                "target",
                "target_type",
                "clean_url",
                "filename_hint",
                "content_length",
                "accept_ranges",
            ],
        ),
        (
            "verification",
            &[
                "file",
                "algorithm",
                "expected",
                "computed",
                "verified",
                "source",
                "error",
            ],
        ),
        (
            "watchdog_action",
            &[
                "action",
                "reason",
                "supports_range",
                "cycle",
                "mode",
                "resilience_event",
                "speed_bps",
                "threshold_bps",
                "rolling_p90_bps",
                "connections",
            ],
        ),
        (
            "when_fast_probe",
            &["url", "attempt", "measured_bps", "threshold_bps", "passed"],
        ),
    ];
    assert_eq!(expected.len(), 43);
    for (name, want) in expected {
        let got: Vec<&str> = shapes[name]["fields"]
            .as_array()
            .unwrap_or_else(|| panic!("missing event shape for {name}"))
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(got, *want, "event {name} field order");
    }
}

#[test]
fn golden_redaction_transformations() {
    let g = fixture("redaction.json");
    let r = &g["redact_secrets"];
    assert_eq!(
        r["Bearer abc123SECRETtoken.XYZ-_+/=="],
        "Bearer ***REDACTED***"
    );
    assert_eq!(r["token=supersecretvalue123"], "token=***REDACTED***");
    assert_eq!(r["token: abcdefghijkl"], "token: ***REDACTED***");
    assert_eq!(r["plain text no secrets"], "plain text no secrets");
    assert_eq!(g["redact_env_token"], "using ***REDACTED*** here");

    let obj = &g["redact_obj"];
    assert_eq!(obj["hf_token"], "***REDACTED***");
    assert_eq!(obj["Authorization"], "***REDACTED***");
    assert_eq!(obj["password"], "***REDACTED***");
    assert_eq!(obj["safe"], "hello");
    assert_eq!(obj["nested"]["github_token"], "***REDACTED***");
    assert_eq!(obj["nested"]["url"], "https://example.com");
    assert_eq!(obj["list"][0]["secret"], "***REDACTED***");
    assert_eq!(obj["list"][1], "ok");
}

#[test]
fn golden_redacted_event_line() {
    let g = fixture("redaction.json");
    assert_eq!(
        g["redact_event_line"].as_str().unwrap(),
        "{\"event\": \"result\", \"timestamp\": 1763376000.123, \"target\": \"https://example.com/f\", \"authorization\": \"***REDACTED***\"}\n"
    );
}

/// Contract test for the future `paradl-events` emitter (W2-4).
#[test]
#[ignore = "depends on paradl-events (W2-4): remove ignore once the crate exists"]
fn contract_event_emitter_matches_golden_shapes() {
    let g = fixture("jsonl_events.json");
    assert_eq!(g["event_count"].as_i64().unwrap(), 43);
    unimplemented!("paradl-events (W2-4) not yet implemented");
}
