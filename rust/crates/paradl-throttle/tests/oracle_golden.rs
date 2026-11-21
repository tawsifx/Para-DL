//! Byte-exact assertions against the W0-2 read-only oracle fixtures.
//!
//! The two fixtures below are goldens produced from the Python oracle.  They
//! are never modified by this crate; they are included read-only at test time.

use paradl_throttle::classifier::{classify_dataset, compute_next_best_hours};
use paradl_throttle::models::{
    MeasurementDataset, SampleRecord, ThrottleReport, TimeVolumeUploadResult,
};
use paradl_throttle::report::generate_report_json;
use serde_json::Value;

const CLASSIFIER_FIXTURE: &str =
    include_str!("../../paradl-oracle/fixtures/classifier_seeded.json");
const REPORT_GOLDEN: &str = include_str!("../../paradl-oracle/fixtures/report_schema.json");

fn fixture() -> Value {
    serde_json::from_str(CLASSIFIER_FIXTURE).expect("classifier fixture is valid JSON")
}

/// Build a `SampleRecord` the same way `tests/unit/test_classifier.py::_sample` does.
fn sample(hour: i32, is_weekend: bool, source: &str, download_bps: f64) -> SampleRecord {
    SampleRecord {
        timestamp: format!("2026-11-17T{:02}:00:00", hour.rem_euclid(24)),
        epoch: 1_763_337_600.0 + f64::from(hour) * 3600.0,
        hour,
        is_weekend,
        interface: "en0".to_owned(),
        ssid: String::new(),
        network_id: "net0".to_owned(),
        source: source.to_owned(),
        download_bps,
        ..SampleRecord::default()
    }
}

#[test]
fn l13_twelve_seeded_cases_are_byte_exact() {
    let doc = fixture();
    for seed in 0..12 {
        let case = &doc["seeded"][seed.to_string()];
        let samples: Vec<SampleRecord> =
            serde_json::from_value(case["samples"].clone()).expect("samples deserialize");
        let expected: Vec<String> =
            serde_json::from_value(case["next_best_hours"].clone()).expect("strings deserialize");
        assert_eq!(
            samples.len() as u64,
            case["n_samples"].as_u64().unwrap(),
            "seed {seed}: sample count"
        );
        assert_eq!(
            compute_next_best_hours(&samples),
            expected,
            "seed {seed}: next_best_hours mismatch"
        );
    }
}

#[test]
fn empty_samples_match_golden() {
    let expected: Vec<String> = serde_json::from_value(fixture()["empty"].clone()).unwrap();
    assert_eq!(compute_next_best_hours(&[]), expected);
}

#[test]
fn edge_cases_match_golden() {
    let cases = [
        vec![
            sample(21, false, "active", 0.0),
            sample(9, false, "active", 0.0),
        ],
        (19..24)
            .flat_map(|h| (0..2).map(move |_| sample(h, false, "passive", 1_000_000.0)))
            .collect(),
        (1..11)
            .flat_map(|h| (0..4).map(move |_| sample(h, false, "active", 1_000_000.0)))
            .chain(
                (11..19).flat_map(|h| (0..4).map(move |_| sample(h, false, "active", 1_000_000.0))),
            )
            .chain(
                (19..24).flat_map(|h| (0..4).map(move |_| sample(h, false, "active", 1_000_000.0))),
            )
            .collect(),
        (19..24)
            .flat_map(|h| (0..3).map(move |_| sample(h, true, "active", 1_000_000.0)))
            .collect(),
        [21, 22, 23, 9, 15]
            .into_iter()
            .map(|h| sample(h, false, "passive", 1_000_000.0))
            .collect(),
    ];
    let goldens: Vec<Vec<String>> =
        serde_json::from_value(fixture()["edge_cases"].clone()).unwrap();
    assert_eq!(cases.len(), goldens.len());
    for (i, (samples, expected)) in cases.iter().zip(goldens.iter()).enumerate() {
        assert_eq!(compute_next_best_hours(samples), *expected, "edge case {i}");
    }
}

#[test]
fn classify_dataset_propagates_next_best_hours_byte_exact() {
    let samples: Vec<SampleRecord> = [21, 22, 9, 15]
        .into_iter()
        .map(|h| sample(h, h % 2 == 0, "active", 1_000_000.0))
        .collect();
    let expected: Vec<String> = serde_json::from_value(fixture()["dataset_case"].clone()).unwrap();
    let dataset = MeasurementDataset {
        m8_time_volume_upload: TimeVolumeUploadResult {
            samples: samples.clone(),
            ..TimeVolumeUploadResult::default()
        },
        ..MeasurementDataset::default()
    };
    assert_eq!(compute_next_best_hours(&samples), expected);
    let result = classify_dataset(&dataset);
    assert_eq!(result.next_best_hours, expected);
    assert_eq!(result.time_of_day_status, "insufficient time data");
    assert_eq!(result.primary_evidence_mode, "paired_same_moment");
}

#[test]
fn l12_report_json_schema_1_0_is_byte_exact() {
    let generated = generate_report_json(&ThrottleReport::default());
    assert_eq!(
        format!("{generated}\n"),
        REPORT_GOLDEN,
        "report.json must byte-match the 1.0 schema golden"
    );
}
