//! Byte-exact effect-size substring for the M8 time-of-day verdict.
//!
//! Review round 1 found that the effect-size percentage used `as i64`
//! (truncation) where the Python authority uses `:.0f` (banker's rounding).
//! Every expected substring below was produced by running the Python oracle
//! (`src/paradl/throttle/classifier.py:215`) on the identical sample set.

use paradl_throttle::classifier::classify_dataset;
use paradl_throttle::models::{MeasurementDataset, SampleRecord, TimeVolumeUploadResult};

fn sample(hour: i32, download_bps: f64) -> SampleRecord {
    SampleRecord {
        timestamp: format!("2026-11-17T{hour:02}:00:00"),
        hour,
        source: "active".to_owned(),
        download_bps,
        ..SampleRecord::default()
    }
}

fn evidence_for(slow_median_bps: f64) -> String {
    // Fast window (hours 0-3) has median 100_000_000; slow window (hours
    // 18-21) has the supplied median. Four active samples per window clear
    // the >=3 samples / >=2.5 effective-weight coverage gates, and the
    // Mann-Whitney U gate passes (p = 0.0284 from the Python oracle).
    let fast = [90_000_000.0, 100_000_000.0, 100_000_000.0, 110_000_000.0];
    let slow = [
        slow_median_bps - 4_500_000.0,
        slow_median_bps,
        slow_median_bps,
        slow_median_bps + 4_500_000.0,
    ];
    let samples: Vec<SampleRecord> = fast
        .into_iter()
        .enumerate()
        .map(|(i, bps)| sample(i as i32, bps))
        .chain(
            slow.into_iter()
                .enumerate()
                .map(|(i, bps)| sample(18 + i as i32, bps)),
        )
        .collect();
    let dataset = MeasurementDataset {
        m8_time_volume_upload: TimeVolumeUploadResult {
            samples,
            ..TimeVolumeUploadResult::default()
        },
        ..MeasurementDataset::default()
    };
    let result = classify_dataset(&dataset);
    let verdict = result
        .verdicts
        .iter()
        .find(|v| v.verdict_class == "time-of-day congestion")
        .expect("time-of-day congestion verdict");
    verdict.evidence[2].clone()
}

/// Evidence with the exact Mann-Whitney U rendering from the Python oracle.
///
/// Python interpolates `u_stat` (a float) with `str()`, so integral values
/// keep their `.0`. Rust's `Display` for `f64` would print `0` instead of
/// `0.0`; `{:?}` reproduces Python's rendering for these values.
fn evidence_with_counts(fast: &[f64], slow: &[f64]) -> String {
    let samples: Vec<SampleRecord> = fast
        .iter()
        .enumerate()
        .map(|(i, bps)| sample(i as i32, *bps))
        .chain(
            slow.iter()
                .enumerate()
                .map(|(i, bps)| sample(18 + i as i32, *bps)),
        )
        .collect();
    let dataset = MeasurementDataset {
        m8_time_volume_upload: TimeVolumeUploadResult {
            samples,
            ..TimeVolumeUploadResult::default()
        },
        ..MeasurementDataset::default()
    };
    let result = classify_dataset(&dataset);
    result
        .verdicts
        .iter()
        .find(|v| v.verdict_class == "time-of-day congestion")
        .expect("time-of-day congestion verdict")
        .evidence[2]
        .clone()
}

#[test]
fn mann_whitney_u_renders_like_python_str() {
    // Oracle: U=0.0, p=0.0284, effect 35.5 -> `:.0f` = 36.
    assert_eq!(
        evidence_with_counts(
            &[90_000_000.0, 100_000_000.0, 100_000_000.0, 110_000_000.0],
            &[60_000_000.0, 64_500_000.0, 64_500_000.0, 69_000_000.0]
        ),
        "Mann-Whitney U=0.0, two-sided p=0.0284 (effect size 36% drop)"
    );
    // Oracle: a tied 65e6 sample in each window yields non-integral U=2.5,
    // p=0.0369, effect 35% -> "2.5" must keep its fractional part.
    assert_eq!(
        evidence_with_counts(
            &[100_000_000.0, 100_000_000.0, 100_000_000.0, 65_000_000.0],
            &[
                65_000_000.0,
                65_000_000.0,
                65_000_000.0,
                65_000_000.0,
                65_000_000.0
            ]
        ),
        "Mann-Whitney U=2.5, two-sided p=0.0369 (effect size 35% drop)"
    );
}

#[test]
fn fractional_effect_size_rounds_like_python_not_truncates() {
    // drop_ratio = 0.645 -> effect 35.5 -> Python `:.0f` = 36, `as i64` = 35.
    assert!(evidence_for(64_500_000.0).contains("(effect size 36% drop)"));
    // drop_ratio = 0.625 -> effect 37.5 -> Python `:.0f` = 38, `as i64` = 37.
    assert!(evidence_for(62_500_000.0).contains("(effect size 38% drop)"));
    // drop_ratio = 0.635 -> effect 36.5 -> Python banker's = 36; a naive
    // half-away-from-zero `round()` would emit 37.
    assert!(evidence_for(63_500_000.0).contains("(effect size 36% drop)"));
}
