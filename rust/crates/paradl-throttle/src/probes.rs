//! Pure probe statistics extracted from the measurement modules M2–M8.
//!
//! The network-I/O portions of `src/paradl/throttle/probes.py` stay in the
//! engine/net crates; this module contains the deterministic computations that
//! turn already-collected samples into model values. All rounding uses Python
//! banker's-rounding semantics via [`py_round`].

use crate::models::{BurstBucketResult, FlowScalePoint, SingleFlowSeries};
use crate::numeric::{fmean, median, pstdev, py_round};

/// Statistics computed from a single-flow throughput series (M2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlowStatistics {
    /// Average throughput in bytes/s.
    pub avg_bps: f64,
    /// Peak sampled throughput in bytes/s.
    pub peak_bps: f64,
    /// Sustained (second-half mean) throughput in bytes/s.
    pub sustained_bps: f64,
}

/// Compute M2 average/peak/sustained statistics from sampled rates.
///
/// Matches `run_m2_single_flow`: the sustained rate is the arithmetic mean of
/// the second half of the sample vector (all samples when fewer than two).
pub fn flow_statistics(samples_bps: &[f64]) -> FlowStatistics {
    let avg = if samples_bps.is_empty() {
        0.0
    } else {
        py_round(fmean(samples_bps), 2)
    };
    let peak = samples_bps.iter().copied().fold(avg, f64::max);
    let tail = if samples_bps.len() >= 2 {
        &samples_bps[samples_bps.len() / 2..]
    } else {
        samples_bps
    };
    let sustained = if tail.is_empty() {
        avg
    } else {
        py_round(fmean(tail), 2)
    };
    FlowStatistics {
        avg_bps: avg,
        peak_bps: py_round(peak, 2),
        sustained_bps: sustained,
    }
}

/// Compute the 16x/1x and 4x/1x scaling ratios of an M3 curve.
///
/// A missing 4-stream point falls back to the 1-stream rate; a missing
/// 16-stream point falls back to the last observed point (or 0.0).
pub fn flow_scale_ratios(points: &[FlowScalePoint]) -> (f64, f64) {
    let rate = |connections: i64| -> Option<f64> {
        points
            .iter()
            .find(|p| p.connections == connections)
            .map(|p| p.aggregate_bps)
    };
    let spd_1 = rate(1).unwrap_or(0.0);
    let spd_4 = rate(4).unwrap_or(spd_1);
    let spd_16 = points
        .iter()
        .find(|p| p.connections >= 16)
        .map(|p| p.aggregate_bps)
        .or_else(|| points.last().map(|p| p.aggregate_bps))
        .unwrap_or(0.0);
    let r16 = if spd_1 > 0.0 { spd_16 / spd_1 } else { 1.0 };
    let r4 = if spd_1 > 0.0 { spd_4 / spd_1 } else { 1.0 };
    (py_round(r16, 3), py_round(r4, 3))
}

/// Analyse a burst/sustained throughput series (M5).
///
/// Pure port of `analyze_burst_series`: estimates the initial burst rate and
/// duration, sustained rate, token-bucket size, and the drop shape.
///
/// ```
/// use paradl_throttle::probes::analyze_burst_series;
/// let samples = [10e6; 4].into_iter().chain([2e6; 8]).collect::<Vec<_>>();
/// let result = analyze_burst_series(&samples, 0.25);
/// assert_eq!(result.drop_shape, "step");
/// assert_eq!(result.burst_to_sustained_ratio, 5.0);
/// ```
pub fn analyze_burst_series(samples_bps: &[f64], sample_interval_sec: f64) -> BurstBucketResult {
    let clean: Vec<f64> = samples_bps
        .iter()
        .copied()
        .filter(|&x| x >= 0.0)
        .map(|x| x.max(0.0))
        .collect();
    if clean.is_empty() {
        return BurstBucketResult::default();
    }
    if clean.len() < 4 {
        let avg = fmean(&clean);
        return BurstBucketResult {
            initial_burst_bps: py_round(avg, 2),
            burst_duration_sec: py_round(clean.len() as f64 * sample_interval_sec, 2),
            sustained_bps: py_round(avg, 2),
            burst_to_sustained_ratio: 1.0,
            estimated_bucket_bytes: 0,
            drop_shape: "flat".to_owned(),
            ..BurstBucketResult::default()
        };
    }

    let mid = (clean.len() / 2).max(2);
    let tail = &clean[mid..];
    let sustained = tail
        .first()
        .map(|_| median(tail))
        .unwrap_or_else(|| clean[clean.len() - 1]);

    let head_limit = ((clean.len() as f64 * 0.6) as usize).max(2);
    let peak_head = clean[..head_limit]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let threshold = sustained + 0.45 * (peak_head - sustained).max(0.0);
    let mut burst_samples = Vec::new();
    for &s in &clean {
        if s >= threshold && peak_head > sustained * 1.45 {
            burst_samples.push(s);
        } else {
            break;
        }
    }
    if burst_samples.is_empty() {
        let n = (clean.len() / 4).max(1);
        burst_samples.extend_from_slice(&clean[..n]);
    }

    let initial_burst = fmean(&burst_samples);
    let burst_dur = burst_samples.len() as f64 * sample_interval_sec;
    let ratio = initial_burst / sustained.max(1.0);

    let overall_med = median(&clean);
    let overall_mean = fmean(&clean);
    let overall_stdev = if clean.len() > 1 { pstdev(&clean) } else { 0.0 };
    let cov = overall_stdev / overall_mean.max(1.0);
    let mut crossings = 0_i64;
    for window in clean.windows(2) {
        if (window[0] >= overall_med) != (window[1] >= overall_med) {
            crossings += 1;
        }
    }

    let (shape, bucket_bytes) = if ratio < 1.40 {
        let periodic = cov >= 0.28 && crossings >= (clean.len() / 3).max(4) as i64;
        (if periodic { "periodic" } else { "flat" }, 0_i64)
    } else {
        let bucket = ((initial_burst - sustained).max(0.0) * burst_dur) as i64;
        let end_burst_idx = burst_samples.len();
        let low_band = sustained * 1.25;
        let mut settle_idx = end_burst_idx;
        while settle_idx < clean.len() && clean[settle_idx] > low_band {
            settle_idx += 1;
        }
        let transition_samples = settle_idx - end_burst_idx;
        let shape = if crossings >= (clean.len() / 3).max(4) as i64 {
            "periodic"
        } else if transition_samples <= 2 {
            "step"
        } else {
            "decay"
        };
        (shape, bucket)
    };

    BurstBucketResult {
        initial_burst_bps: py_round(initial_burst, 2),
        burst_duration_sec: py_round(burst_dur, 3),
        sustained_bps: py_round(sustained, 2),
        burst_to_sustained_ratio: py_round(ratio, 3),
        estimated_bucket_bytes: bucket_bytes,
        drop_shape: shape.to_owned(),
        ..BurstBucketResult::default()
    }
}

/// Compute the mean and population coefficient of variation of a rate series.
///
/// Used by M6 to distinguish smooth shaping from sawtooth policing.
pub fn throughput_cov(samples_bps: &[f64]) -> (f64, f64) {
    let mean = fmean(samples_bps);
    let stdev = pstdev(samples_bps);
    let cov = stdev / mean.max(1.0);
    (py_round(mean, 4), py_round(cov, 4))
}

/// Build M2 series statistics from a byte count and elapsed duration.
pub fn rate_from_bytes(bytes: i64, duration_sec: f64) -> f64 {
    if duration_sec > 0.0 {
        py_round(bytes as f64 / duration_sec, 2)
    } else {
        0.0
    }
}

/// Apply the M2 rounding contract to a [`SingleFlowSeries`] computed elsewhere.
pub fn finalize_single_flow(mut series: SingleFlowSeries) -> SingleFlowSeries {
    series.avg_bps = py_round(series.avg_bps, 2);
    series.peak_bps = py_round(series.peak_bps, 2);
    series.sustained_bps = py_round(series.sustained_bps, 2);
    series.duration_sec = py_round(series.duration_sec, 3);
    series.samples_bps = series.samples_bps.iter().map(|s| py_round(*s, 2)).collect();
    series
}
