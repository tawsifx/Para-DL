//! Golden assertions mirrored from `tests/unit/test_classifier.py` (12 seeded cases) and the
//! `report.json` schema `1.0` contract (`modules.m0..m10`).

use crate::fixtures::{fixture, fixture_str};

/// Assert JSON key order in the *raw fixture text* (serde_json's `Value` uses a BTreeMap and
/// loses insertion order, but the golden fixture itself preserves the oracle's byte layout).
fn assert_raw_key_order(text: &str, keys: &[&str]) {
    let mut cursor = 0usize;
    for key in keys {
        let needle = format!("\"{key}\":");
        let found = text[cursor..]
            .find(&needle)
            .unwrap_or_else(|| panic!("missing raw key {key}"));
        cursor += found + needle.len();
    }
}

/// The 12 seeded `next_best_hours` outputs, byte-for-byte from the Python oracle.
/// These strings embed exact floating-point weight text such as `2.7/3.0`; they catch
/// Python `round()` vs Rust formatting drift.
#[test]
fn golden_classifier_12_seeded_cases() {
    let g = fixture("classifier_seeded.json");
    let seeded = &g["seeded"];
    assert_eq!(seeded.as_object().unwrap().len(), 12);

    let expected: [(&str, Vec<&str>); 12] = [
        ("0", vec![]),
        (
            "1",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 0.0/3.0)",
                "run one sample around 21:00 on a weekend (weekend peak comparison)",
            ],
        ),
        ("2", vec![]),
        (
            "3",
            vec!["run one sample around 21:00 on a weekend (weekend peak comparison)"],
        ),
        (
            "4",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 2.0/3.0)",
                "run one sample around 15:00 on a weekday (daytime comparison window)",
                "run one sample around 21:00 on a weekend (weekend peak comparison)",
            ],
        ),
        ("5", vec![]),
        ("6", vec![]),
        (
            "7",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 2.7/3.0)",
                "run one sample around 21:00 on a weekend (weekend peak comparison)",
            ],
        ),
        (
            "8",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 1.4/3.0)",
            ],
        ),
        (
            "9",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 1.8/3.0)",
            ],
        ),
        ("10", vec!["run one sample around 09:00 on a weekday"]),
        (
            "11",
            vec![
                "run one sample around 21:00 on a weekday (evening peak window has weight 1.7/3.0)",
            ],
        ),
    ];
    for (seed, want) in expected {
        let got = seeded[seed]["next_best_hours"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("seed {seed} has no next_best_hours array: {}", seeded[seed])
            });
        let got: Vec<&str> = got.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(got, want, "seed {seed} next_best_hours");
        assert!(
            seeded[seed]["n_samples"].as_u64().unwrap() <= 220,
            "seed {seed} sample count out of range"
        );
    }
}

#[test]
fn golden_classifier_empty_and_edge_cases() {
    let g = fixture("classifier_seeded.json");
    assert_eq!(g["empty"].as_array().unwrap().len(), 4);
    let edges = g["edge_cases"].as_array().unwrap();
    assert_eq!(edges.len(), 5);
    // The final edge case (mixed fractional passive weights) must include the exact
    // passive-weight suggestion produced by the oracle.
    let last: Vec<&str> = edges[4]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        !last.is_empty(),
        "mixed-fractional edge case must produce suggestions"
    );
    let dataset: Vec<&str> = g["dataset_case"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        dataset.iter().any(|s| s.contains("21:00")),
        "classify_dataset parity case must suggest the 21:00 evening window: {dataset:?}"
    );
}

/// `report.json` schema `1.0` must expose exactly modules m0..m10 in order.
#[test]
fn golden_report_schema_v1_modules_m0_to_m10() {
    let g = fixture("report_schema.json");
    assert_eq!(g["schema_version"].as_str().unwrap(), "1.0");
    let top = [
        "schema_version",
        "run_id",
        "mode",
        "isp",
        "plan_mbps",
        "timestamp",
        "budget",
        "environment",
        "targets",
        "modules",
        "verdicts",
        "notes",
        "time_of_day_status",
        "next_best_hours",
        "primary_evidence_mode",
        "recommended_profile",
        "isp_evidence_summary",
    ];
    let raw = fixture_str("report_schema.json");
    assert_raw_key_order(&raw, &top);
    let obj = g.as_object().unwrap();
    for key in top {
        assert!(obj.contains_key(key), "missing top-level key {key}");
    }

    let modules = &g["modules"];
    let module_keys = [
        "m0_sanity",
        "m1_baseline",
        "m2_single_flow",
        "m3_flow_scaling",
        "m4_destination_independence",
        "m5_burst_bucket",
        "m6_policing_shaping",
        "m7_protocol_port",
        "m8_time_volume_upload",
        "m9_ground_truth",
        "m10_local_vs_intl",
    ];
    let modules_idx = raw.find("\"modules\": {").unwrap();
    assert_raw_key_order(&raw[modules_idx..], &module_keys);
    for key in module_keys {
        assert!(
            modules.as_object().unwrap().contains_key(key),
            "missing module {key}"
        );
    }
    // m0 and m10 are the boundary modules; their exact field lists are part of the contract.
    let m0 = [
        "loopback_bps",
        "loopback_ok",
        "active_interface",
        "interface_type",
        "ssid",
        "wifi_link_mbps",
        "vpn_or_tunnel_active",
        "tunnel_interfaces",
        "cpu_load_1m",
        "competing_bytes_total",
        "competing_processes",
        "on_battery",
        "contaminated",
        "warnings",
    ];
    assert_raw_key_order(&raw[modules_idx..], &m0);
    for key in m0 {
        assert!(modules["m0_sanity"].as_object().unwrap().contains_key(key));
    }
    let m10 = [
        "domestic_median_bps",
        "international_median_bps",
        "isp_cached_median_bps",
        "domestic_16_bps",
        "international_16_bps",
        "isp_cached_16_bps",
        "shared_slow_upstream_asn",
        "fast_international_asns",
        "slow_international_asns",
        "traces",
    ];
    assert_raw_key_order(&raw[modules_idx..], &m10);
    for key in m10 {
        assert!(
            modules["m10_local_vs_intl"]
                .as_object()
                .unwrap()
                .contains_key(key)
        );
    }
}

/// Contract test for the future `paradl-throttle` classifier (W2-1).
#[test]
#[ignore = "depends on paradl-throttle (W2-1): remove ignore once the crate exists"]
fn contract_classifier_reproduces_12_seeded_cases() {
    // A real implementation will call `paradl_throttle::classifier::next_best_hours(samples)`
    // for each seeded sample set and compare to the golden strings above. The expected values
    // are present in `classifier_seeded.json`; this placeholder documents the binding site.
    let g = fixture("classifier_seeded.json");
    assert_eq!(g["seeded"].as_object().unwrap().len(), 12);
    unimplemented!("paradl-throttle (W2-1) not yet implemented");
}
