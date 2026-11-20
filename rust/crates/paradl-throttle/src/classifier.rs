//! Pure rule-based ISP throttling classifier and statistical significance engine.
//!
//! Direct port of `src/paradl/throttle/classifier.py`. The module performs no
//! I/O and reads no clock, so it is fully deterministic for a given dataset.
//! All numeric formatting mirrors Python's `round()` (banker's rounding),
//! `statistics.median`, and f-string fixed-point formatting.

use std::collections::{BTreeSet, HashMap};

use paradl_core::{format_size, format_speed};

use crate::models::{
    ClassificationResult, DestinationIndependenceResult, ISPEvidenceSummary, MeasurementDataset,
    RecommendedProfile, SampleRecord, Verdict, VerdictClass, VerdictNote,
};
use crate::numeric::{erfc, median, py_round};

/// Compute the Mann-Whitney U statistic and two-sided normal-approximation
/// p-value with tie correction.
///
/// Returns `(U, p)` where both values use Python's `round(..., 3)` and
/// `round(..., 6)` semantics respectively. Empty inputs return `(0.0, 1.0)`.
///
/// ```
/// use paradl_throttle::classifier::mann_whitney_u_test;
/// assert_eq!(mann_whitney_u_test(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]), (0.0, 0.080856));
/// ```
pub fn mann_whitney_u_test(sample_a: &[f64], sample_b: &[f64]) -> (f64, f64) {
    let n1 = sample_a.len();
    let n2 = sample_b.len();
    if n1 == 0 || n2 == 0 {
        return (0.0, 1.0);
    }

    let mut combined: Vec<(f64, usize)> = sample_a
        .iter()
        .map(|&x| (x, 0usize))
        .chain(sample_b.iter().map(|&y| (y, 1usize)))
        .collect();
    // `sort_by` is stable, matching Python's `list.sort`.
    combined.sort_by(|a, b| a.0.total_cmp(&b.0));

    let n_total = n1 + n2;
    let mut ranks = vec![0.0_f64; n_total];
    let mut tie_counts: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < n_total {
        let mut j = i + 1;
        while j < n_total && combined[j].0 == combined[i].0 {
            j += 1;
        }
        let avg_rank = (i as f64 + 1.0 + j as f64) / 2.0;
        for rank in ranks.iter_mut().take(j).skip(i) {
            *rank = avg_rank;
        }
        if j - i > 1 {
            tie_counts.push(j - i);
        }
        i = j;
    }

    let r1: f64 = (0..n_total)
        .filter(|&idx| combined[idx].1 == 0)
        .map(|idx| ranks[idx])
        .sum();
    let n1f = n1 as f64;
    let n2f = n2 as f64;
    let u1 = r1 - (n1f * (n1f + 1.0)) / 2.0;
    let u2 = n1f * n2f - u1;
    let u_stat = u1.min(u2);

    let mean_u = (n1f * n2f) / 2.0;
    let tie_term = if n_total > 1 {
        tie_counts
            .iter()
            .map(|&t| {
                let tf = t as f64;
                tf * tf * tf - tf
            })
            .sum::<f64>()
            / ((n_total as f64) * (n_total as f64 - 1.0))
    } else {
        0.0
    };
    let var_u = (n1f * n2f / 12.0) * ((n_total as f64 + 1.0) - tie_term);
    if var_u <= 0.0 {
        return (u_stat, 1.0);
    }

    let z = ((u1 - mean_u).abs() - 0.5) / var_u.sqrt();
    if z <= 0.0 {
        return (u_stat, 1.0);
    }
    let p_val = erfc(z / std::f64::consts::SQRT_2);
    (py_round(u_stat, 3), py_round(p_val.clamp(0.0, 1.0), 6))
}

/// Compute next-best-hour suggestions directly from `samples`.
///
/// This reproduces, byte-for-byte, the suggestion list produced by the
/// discarded 24-hour x weekday/weekend coverage matrix. Per-cell effective
/// weights use the same `round(..., 2)` and are summed in the same order so the
/// formatted `{:.1}` suggestion strings are identical (parity rows L13/L24).
///
/// ```
/// use paradl_throttle::classifier::compute_next_best_hours;
/// let suggestions = compute_next_best_hours(&[]);
/// assert_eq!(suggestions.len(), 4);
/// assert!(suggestions[0].contains("21:00 on a weekday"));
/// ```
pub fn compute_next_best_hours(samples: &[SampleRecord]) -> Vec<String> {
    let mut cells: HashMap<(i32, bool), Vec<&SampleRecord>> = HashMap::new();
    for wk in [false, true] {
        for h in 0..24 {
            cells.insert((h, wk), Vec::new());
        }
    }
    for s in samples {
        if s.download_bps > 0.0 {
            cells
                .entry((s.hour.rem_euclid(24), s.is_weekend))
                .or_default()
                .push(s);
        }
    }

    let mut eff_weight: HashMap<(i32, bool), f64> = HashMap::new();
    let mut has_data: HashMap<(i32, bool), bool> = HashMap::new();
    for wk in [false, true] {
        for h in 0..24 {
            let bucket = &cells[&(h, wk)];
            let act = bucket.iter().filter(|x| x.source == "active").count() as f64;
            let pas = bucket.iter().filter(|x| x.source != "active").count() as f64;
            eff_weight.insert((h, wk), py_round(act * 1.0 + pas * 0.35, 2));
            has_data.insert((h, wk), act + pas > 0.0);
        }
    }

    let window_weight = |hours: std::ops::Range<i32>, is_wk: bool| -> f64 {
        hours.fold(0.0, |acc, h| acc + eff_weight[&(h, is_wk)])
    };

    let wd_peak_w = window_weight(19..24, false);
    let wd_offpeak_w = window_weight(1..11, false);
    let wd_afternoon_w = window_weight(11..19, false);
    let wk_peak_w = window_weight(19..24, true);

    let mut suggestions: Vec<String> = Vec::new();
    if wd_peak_w < 3.0 {
        suggestions.push(format!(
            "run one sample around 21:00 on a weekday (evening peak window has weight {wd_peak_w:.1}/3.0)"
        ));
    }
    if wd_offpeak_w < 3.0 {
        suggestions.push(format!(
            "run one sample around 09:00 on a weekday (morning/off-peak baseline has weight {wd_offpeak_w:.1}/3.0)"
        ));
    }
    if wd_afternoon_w < 2.0 {
        suggestions.push(
            "run one sample around 15:00 on a weekday (daytime comparison window)".to_owned(),
        );
    }
    if wk_peak_w < 2.0 {
        suggestions
            .push("run one sample around 21:00 on a weekend (weekend peak comparison)".to_owned());
    }

    if suggestions.is_empty() {
        for h_rep in [21, 9, 15, 3] {
            if !has_data[&(h_rep, false)] {
                suggestions.push(format!("run one sample around {h_rep:02}:00 on a weekday"));
                break;
            }
        }
    }

    suggestions
}

/// Result of the M8 time-of-day evaluation.
struct TimeOfDayOutcome {
    verdict: Option<Verdict>,
    note: Option<VerdictNote>,
    status: String,
    next_hours: Vec<String>,
}

fn evaluate_time_of_day(samples: &[SampleRecord]) -> TimeOfDayOutcome {
    let next_best_hours = compute_next_best_hours(samples);
    if samples.is_empty() {
        return TimeOfDayOutcome {
            verdict: None,
            note: Some(VerdictNote {
                code: "insufficient_time_data".to_owned(),
                title: "insufficient time data".to_owned(),
                text:
                    "No multi-hour samples recorded yet; relying on paired same-moment comparisons."
                        .to_owned(),
                recommendations: next_best_hours.clone(),
            }),
            status: "insufficient time data".to_owned(),
            next_hours: next_best_hours,
        };
    }

    let mut buckets: HashMap<(bool, i32), Vec<&SampleRecord>> = HashMap::new();
    for s in samples {
        if s.download_bps > 0.0 {
            let win_id = s.hour.rem_euclid(24) / 6;
            buckets.entry((s.is_weekend, win_id)).or_default().push(s);
        }
    }

    let mut qualifying: HashMap<bool, Vec<(i32, Vec<&SampleRecord>)>> = HashMap::new();
    qualifying.insert(false, Vec::new());
    qualifying.insert(true, Vec::new());
    for ((is_wk, win_id), list) in &buckets {
        let eff_w: f64 = list.iter().map(|x| x.weight()).sum();
        if list.len() >= 3 && eff_w >= 2.5 {
            qualifying
                .get_mut(is_wk)
                .expect("day-type key exists")
                .push((*win_id, list.clone()));
        }
    }

    let best_daytype_windows = if qualifying[&false].len() >= 2 {
        qualifying[&false].clone()
    } else {
        Vec::new()
    };

    if best_daytype_windows.len() < 2 {
        return TimeOfDayOutcome {
            verdict: None,
            note: Some(VerdictNote {
                code: "insufficient_time_data".to_owned(),
                title: "insufficient time data".to_owned(),
                text: "Time-of-day samples do not yet cover at least two distinct time-of-day windows with >= 3 samples each; refusing to guess time-of-day congestion.".to_owned(),
                recommendations: next_best_hours.clone(),
            }),
            status: "insufficient time data".to_owned(),
            next_hours: next_best_hours,
        };
    }

    let mut window_medians: Vec<(f64, i32, Vec<&SampleRecord>)> = best_daytype_windows
        .into_iter()
        .map(|(win_id, list)| {
            let values: Vec<f64> = list.iter().map(|x| x.download_bps).collect();
            (median(&values), win_id, list)
        })
        .collect();
    window_medians.sort_by(|a, b| b.0.total_cmp(&a.0));

    let (fast_med, fast_win, fast_list) = &window_medians[0];
    let (slow_med, slow_win, slow_list) = &window_medians[window_medians.len() - 1];
    let drop_ratio = *slow_med / (*fast_med).max(1.0);
    let (u_stat, p_val) = mann_whitney_u_test(
        &fast_list.iter().map(|x| x.download_bps).collect::<Vec<_>>(),
        &slow_list.iter().map(|x| x.download_bps).collect::<Vec<_>>(),
    );

    let win_names = |win: i32| -> &'static str {
        match win {
            0 => "00:00-05:59",
            1 => "06:00-11:59",
            2 => "12:00-17:59",
            3 => "18:00-23:59",
            _ => "unknown",
        }
    };
    let fast_name = win_names(*fast_win);
    let slow_name = win_names(*slow_win);

    if drop_ratio <= 0.65 && p_val <= 0.05 {
        let conf = py_round((0.78 + (1.0 - drop_ratio) * 0.25).min(0.94), 2);
        let verdict = Verdict {
            verdict_class: VerdictClass::TimeOfDayCongestion.as_str().to_owned(),
            confidence: conf,
            summary: format!(
                "Statistically significant throughput drop during {} ({}) compared to {} ({}).",
                slow_name,
                format_speed(*slow_med),
                fast_name,
                format_speed(*fast_med)
            ),
            evidence: vec![
                format!(
                    "Off-peak window ({}) median: {} (n={})",
                    fast_name,
                    format_speed(*fast_med),
                    fast_list.len()
                ),
                format!(
                    "Peak window ({}) median: {} (n={})",
                    slow_name,
                    format_speed(*slow_med),
                    slow_list.len()
                ),
                format!(
                    "Mann-Whitney U={u_stat:?}, two-sided p={p_val:.4} (effect size {:.0}% drop)",
                    (1.0 - drop_ratio) * 100.0
                ),
            ],
        };
        return TimeOfDayOutcome {
            verdict: Some(verdict),
            note: None,
            status: "significant time-of-day variation detected".to_owned(),
            next_hours: next_best_hours,
        };
    }

    TimeOfDayOutcome {
        verdict: None,
        note: None,
        status: "no significant time-of-day slowdown".to_owned(),
        next_hours: next_best_hours,
    }
}

/// Map a complete [`MeasurementDataset`] to a [`ClassificationResult`].
///
/// Port of `classify_dataset`: ten independent rule families produce verdicts
/// and notes, verdicts are sorted by confidence, then a recommended profile,
/// neutral ISP evidence summary, and next-best-hour list are assembled.
pub fn classify_dataset(dataset: &MeasurementDataset) -> ClassificationResult {
    let mut verdicts: Vec<Verdict> = Vec::new();
    let mut notes: Vec<VerdictNote> = Vec::new();

    let contaminated = dataset.m0_sanity.contaminated;
    let penalty = if contaminated { 0.22 } else { 0.0 };
    if contaminated {
        notes.push(VerdictNote {
            code: "contaminated_environment".to_owned(),
            title: "contaminated local environment".to_owned(),
            text: "Local environment warnings were triggered during M0 sanity checks; confidence scores are reduced.".to_owned(),
            recommendations: dataset.m0_sanity.warnings.clone(),
        });
    }

    // 1. HTTP 200-only (no Range) targets.
    let mut no_range_hosts: Vec<String> = dataset
        .m2_single_flow
        .iter()
        .filter(|s| !s.supports_range)
        .map(|s| s.host.clone())
        .chain(
            dataset
                .m3_flow_scaling
                .iter()
                .filter(|f| !f.supports_range)
                .map(|f| f.host.clone()),
        )
        .collect();
    let all_m2_no_range = dataset
        .m2_single_flow
        .iter()
        .filter(|s| !s.host.is_empty())
        .all(|s| !s.supports_range);
    if !no_range_hosts.is_empty() && all_m2_no_range {
        no_range_hosts.sort();
        no_range_hosts.dedup();
        let h_str = no_range_hosts.join(", ");
        verdicts.push(Verdict {
            verdict_class: VerdictClass::NoRangeSupport.as_str().to_owned(),
            confidence: py_round((0.92_f64 - penalty).max(0.55), 2),
            summary: format!(
                "Target server ({h_str}) returned HTTP 200 OK only and ignored byte-range requests."
            ),
            evidence: vec![
                format!("Range probe on {h_str} returned HTTP 200 instead of 206 Partial Content"),
                "Multi-connection segmented downloads and socket recycling are disabled for 200-only servers".to_owned(),
            ],
        });
    }

    // 2. M3 flow scaling -> per-flow cap.
    let mut intl_flow_scaled = false;
    let mut dom_flow_scaled = false;
    let mut max_scale_16 = 1.0_f64;
    let mut best_m3: Option<&crate::models::FlowScalingResult> = None;
    for f in &dataset.m3_flow_scaling {
        if !f.supports_range || f.points.is_empty() {
            continue;
        }
        let r16 = f.scaling_ratio_16_to_1;
        let r4 = f.scaling_ratio_4_to_1;
        if r16 >= 2.0 || r4 >= 1.75 {
            if f.locality == "domestic" {
                dom_flow_scaled = true;
            } else {
                intl_flow_scaled = true;
            }
            if r16 >= max_scale_16 || best_m3.is_none() {
                max_scale_16 = r16.max(r4);
                best_m3 = Some(f);
            }
        }
    }
    if let Some(f) = best_m3 {
        let p1 = f.points[0].aggregate_bps;
        let p_last = f.points[f.points.len() - 1].aggregate_bps;
        let n_last = f.points[f.points.len() - 1].connections;
        let conf = (0.72 + 0.04 * max_scale_16.min(6.0)).min(0.95) - penalty;
        let scope_note = if dom_flow_scaled && intl_flow_scaled {
            "across both domestic and international paths".to_owned()
        } else {
            format!("on {}", f.host)
        };
        verdicts.push(Verdict {
            verdict_class: VerdictClass::PerFlowCap.as_str().to_owned(),
            confidence: py_round(conf.max(0.45), 2),
            summary: format!(
                "Each TCP connection is capped (~{}), while {n_last} parallel connections scale aggregate speed {max_scale_16:.1}x to {} ({scope_note}).",
                format_speed(p1),
                format_speed(p_last)
            ),
            evidence: vec![
                format!("1 connection on {}: {}", f.host, format_speed(p1)),
                format!(
                    "{n_last} connections on {}: {} ({max_scale_16:.2}x scaling)",
                    f.host,
                    format_speed(p_last)
                ),
                format!(
                    "Per-flow rate at {n_last} streams: {}",
                    format_speed(f.points[f.points.len() - 1].per_flow_bps)
                ),
            ],
        });
    }

    // 3. M5 burst-then-throttle.
    for b in &dataset.m5_burst_bucket {
        if b.initial_burst_bps > 0.0
            && b.sustained_bps > 0.0
            && b.burst_to_sustained_ratio >= 1.75
            && (b.drop_shape == "step" || b.drop_shape == "decay")
        {
            let conf = (0.76 + 0.05 * b.burst_to_sustained_ratio.min(4.0)).min(0.95) - penalty;
            let host = if b.host.is_empty() {
                &b.target_id
            } else {
                &b.host
            };
            let mut evidence = vec![
                format!(
                    "Initial burst on {host}: {} for {:.2}s",
                    format_speed(b.initial_burst_bps),
                    b.burst_duration_sec
                ),
                format!(
                    "Sustained rate after drop ({}): {} ({:.1}x collapse)",
                    b.drop_shape,
                    format_speed(b.sustained_bps),
                    b.burst_to_sustained_ratio
                ),
                format!(
                    "Estimated token bucket size: {}",
                    format_size(Some(b.estimated_bucket_bytes as f64))
                ),
            ];
            if let Some(refill) = b.refill_30s_ratio {
                evidence.push(format!(
                    "30s idle refill ratio: {:.0}% of initial burst",
                    refill * 100.0
                ));
            }
            if let Some(refill) = b.refill_120s_ratio {
                evidence.push(format!(
                    "120s idle refill ratio: {:.0}% of initial burst",
                    refill * 100.0
                ));
            }
            verdicts.push(Verdict {
                verdict_class: VerdictClass::BurstThenThrottle.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Connections start with a fast burst ({}) for ~{:.1}s before dropping ({}) to {}.",
                    format_speed(b.initial_burst_bps),
                    b.burst_duration_sec,
                    b.drop_shape,
                    format_speed(b.sustained_bps)
                ),
                evidence,
            });
            break;
        }
    }

    // 4. M4 destination independence + flat M3 -> per-destination limit.
    let m4: &DestinationIndependenceResult = &dataset.m4_destination_independence;
    let flat_m3: Vec<&crate::models::FlowScalingResult> = dataset
        .m3_flow_scaling
        .iter()
        .filter(|f| {
            f.supports_range
                && f.points.len() >= 2
                && f.scaling_ratio_16_to_1 < 1.48
                && f.scaling_ratio_4_to_1 < 1.45
        })
        .collect();
    if let Some(f0) = flat_m3.first() {
        if m4.independence_ratio >= 1.45 {
            let ceil_spd = f0.points[f0.points.len() - 1].aggregate_bps;
            let conf =
                (0.75 + 0.20 * ((m4.independence_ratio - 1.4) / 0.6).min(1.0)).min(0.93) - penalty;
            verdicts.push(Verdict {
                verdict_class: VerdictClass::PerDestinationLimit.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Aggregate throughput to a single host is capped around {} regardless of connection count, but two concurrent hosts achieve {} ({:.2}x).",
                    format_speed(ceil_spd),
                    format_speed(m4.concurrent_total_bps),
                    m4.independence_ratio
                ),
                evidence: vec![
                    format!(
                        "Flow scaling on {} plateaued at {} (16x/1x ratio = {:.2})",
                        f0.host,
                        format_speed(ceil_spd),
                        f0.scaling_ratio_16_to_1
                    ),
                    format!(
                        "Solo {}: {}, Solo {}: {}",
                        m4.host_a,
                        format_speed(m4.solo_a_bps),
                        m4.host_b,
                        format_speed(m4.solo_b_bps)
                    ),
                    format!(
                        "Concurrent {} + {}: {} (independence ratio {:.2}x)",
                        m4.host_a,
                        m4.host_b,
                        format_speed(m4.concurrent_total_bps),
                        m4.independence_ratio
                    ),
                ],
            });
        }
    }

    // 5. M6 policing vs shaping.
    for p6 in &dataset.m6_policing_shaping {
        if p6.mechanism_hint == "policing"
            || ((p6.retransmit_rate_pct >= 1.5 || p6.connection_resets > 0)
                && p6.rtt_inflation_ms < 25.0)
        {
            let conf = if p6.retransmit_rate_pct >= 2.0 || p6.connection_resets > 0 {
                0.86
            } else {
                0.72
            } - penalty;
            verdicts.push(Verdict {
                verdict_class: VerdictClass::LossBasedPolicing.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Hard packet-drop/reset policing detected on {} (retransmit rate {:.2}%, resets={}, minimal RTT growth +{:.1} ms).",
                    p6.host,
                    p6.retransmit_rate_pct,
                    p6.connection_resets,
                    p6.rtt_inflation_ms
                ),
                evidence: vec![
                    format!(
                        "TCP retransmit rate delta: {:.2}% ({} pkts, {} resets)",
                        p6.retransmit_rate_pct, p6.tcp_retransmits_delta, p6.connection_resets
                    ),
                    format!(
                        "RTT under load ({:.1} ms) vs idle ({:.1} ms): +{:.1} ms",
                        p6.loaded_rtt_ms, p6.idle_rtt_ms, p6.rtt_inflation_ms
                    ),
                    format!("Throughput CoV: {:.2}", p6.throughput_cov),
                ],
            });
            break;
        }
        if p6.mechanism_hint == "shaping"
            || (p6.rtt_inflation_ms >= 35.0
                && p6.retransmit_rate_pct < 1.0
                && p6.throughput_cov < 0.25)
        {
            let conf = (if p6.rtt_inflation_ms >= 45.0 {
                0.87
            } else {
                0.76
            }) - penalty;
            verdicts.push(Verdict {
                verdict_class: VerdictClass::QueueBasedShaping.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Queue-based traffic shaping detected on {}: smooth rate cap (CoV={:.2}) with significant buffer queueing delay (+{:.1} ms RTT growth under load).",
                    p6.host, p6.throughput_cov, p6.rtt_inflation_ms
                ),
                evidence: vec![
                    format!(
                        "Idle RTT: {:.1} ms -> Loaded RTT: {:.1} ms (+{:.1} ms)",
                        p6.idle_rtt_ms, p6.loaded_rtt_ms, p6.rtt_inflation_ms
                    ),
                    format!(
                        "Low packet retransmission ({:.2}%) with smooth rate (CoV {:.2})",
                        p6.retransmit_rate_pct, p6.throughput_cov
                    ),
                ],
            });
            break;
        }
    }

    // 6. M7 protocol & port differentiation.
    for p7 in &dataset.m7_protocol_port {
        let mut diffs: Vec<(String, f64, f64, f64)> = Vec::new();
        if let (Some(h3), Some(h11)) = (p7.http3_bps, p7.http1_1_bps) {
            if h3 > 0.0 && h11 > 0.0 {
                let r = h3.max(h11) / h11.min(h3).max(1.0);
                if r >= 1.75 {
                    diffs.push(("HTTP/3 vs HTTP/1.1".to_owned(), h3, h11, r));
                }
            }
        }
        if let (Some(https), Some(http)) = (p7.https_bps, p7.http_plain_bps) {
            if https > 0.0 && http > 0.0 && https.min(http) > 0.0 {
                let r = https.max(http) / https.min(http);
                if r >= 1.75 {
                    diffs.push(("HTTPS vs plain HTTP".to_owned(), https, http, r));
                }
            }
        }
        if let (Some(p443), Some(palt)) = (p7.port_443_bps, p7.alt_port_bps) {
            if p443 > 0.0 && palt > 0.0 && p443.min(palt) > 0.0 {
                let r = p443.max(palt) / p443.min(palt);
                if r >= 1.75 {
                    diffs.push((
                        format!("Port 443 vs Port {}", p7.alt_port.unwrap_or(0)),
                        p443,
                        palt,
                        r,
                    ));
                }
            }
        }
        if let (Some(v4), Some(v6)) = (p7.ipv4_bps, p7.ipv6_bps) {
            if v4 > 0.0 && v6 > 0.0 && v4.min(v6) > 0.0 {
                let r = v4.max(v6) / v4.min(v6);
                if r >= 1.75 {
                    diffs.push(("IPv4 vs IPv6".to_owned(), v4, v6, r));
                }
            }
        }
        if let Some((label, sa, sb, ratio)) = diffs.first() {
            let conf = (0.78 + 0.05 * ratio.min(4.0)).min(0.93) - penalty;
            verdicts.push(Verdict {
                verdict_class: VerdictClass::ProtocolPortDiff.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Throughput differs by {ratio:.1}x across {label} on {} ({} vs {}).",
                    p7.host,
                    format_speed(*sa),
                    format_speed(*sb)
                ),
                evidence: diffs
                    .iter()
                    .map(|d| {
                        format!(
                            "{} on {}: {} vs {} ({:.2}x)",
                            d.0,
                            p7.host,
                            format_speed(d.1),
                            format_speed(d.2),
                            d.3
                        )
                    })
                    .collect(),
            });
            break;
        }
    }

    // 7. M7 SNI or M9 proxy / payload-type differentiation.
    let m9 = &dataset.m9_ground_truth;
    let mut sni_triggered = false;
    for p7 in &dataset.m7_protocol_port {
        if let (Some(def), Some(alt)) = (p7.sni_default_bps, p7.sni_alt_bps) {
            if def > 0.0 {
                let sni_ratio = alt / def;
                if sni_ratio >= 1.80 {
                    sni_triggered = true;
                    verdicts.push(Verdict {
                        verdict_class: VerdictClass::SniServiceDiff.as_str().to_owned(),
                        confidence: py_round((0.72_f64 - penalty).max(0.45), 2),
                        summary: format!(
                            "Best-effort TLS SNI probe on {} achieved {sni_ratio:.1}x higher speed with alternate SNI ({} vs {}).",
                            p7.host,
                            format_speed(alt),
                            format_speed(def)
                        ),
                        evidence: vec![
                            format!("Default SNI on {}: {}", p7.host, format_speed(def)),
                            format!(
                                "Alternate SNI ({}): {} ({sni_ratio:.2}x)",
                                p7.sni_label,
                                format_speed(alt)
                            ),
                        ],
                    });
                    break;
                }
            }
        }
    }
    if !sni_triggered {
        if let Some(speedup) = m9.proxy_speedup_ratio {
            if speedup >= 1.85 {
                let conf = (0.85 + 0.03 * speedup.min(4.0)).min(0.95) - penalty;
                verdicts.push(Verdict {
                    verdict_class: VerdictClass::SniServiceDiff.as_str().to_owned(),
                    confidence: py_round(conf.max(0.45), 2),
                    summary: format!(
                        "Paired tunnel comparison (--via-proxy) is {speedup:.1}x faster than direct ({} vs {}), indicating destination or DPI-based throttling rather than line capacity.",
                        format_speed(m9.proxy_1_bps.unwrap_or(0.0)),
                        format_speed(m9.direct_1_bps.unwrap_or(0.0))
                    ),
                    evidence: vec![
                        format!(
                            "Direct 1-stream: {} vs Proxy 1-stream: {}",
                            format_speed(m9.direct_1_bps.unwrap_or(0.0)),
                            format_speed(m9.proxy_1_bps.unwrap_or(0.0))
                        ),
                        format!("Proxy speedup ratio: {speedup:.2}x"),
                    ],
                });
            }
        } else if let (Some(rand), Some(video)) =
            (m9.own_server_random_bps, m9.own_server_video_bps)
        {
            if video > 0.0 && rand / video >= 1.80 {
                let r_pay = rand / video;
                let conf = (0.84 + 0.03 * r_pay.min(4.0)).min(0.94) - penalty;
                verdicts.push(Verdict {
                    verdict_class: VerdictClass::SniServiceDiff.as_str().to_owned(),
                    confidence: py_round(conf.max(0.45), 2),
                    summary: format!(
                        "VPS own-server payload comparison shows video-labeled bytes are throttled {r_pay:.1}x relative to random bytes ({} vs {}).",
                        format_speed(video),
                        format_speed(rand)
                    ),
                    evidence: vec![
                        format!("Own-server random payload: {}", format_speed(rand)),
                        format!(
                            "Own-server video/mp4 payload: {} ({r_pay:.2}x slower)",
                            format_speed(video)
                        ),
                    ],
                });
            }
        }
    }

    // 8. M8 time-of-day congestion.
    let tod = evaluate_time_of_day(&dataset.m8_time_volume_upload.samples);
    if let Some(mut verdict) = tod.verdict {
        verdict.confidence = py_round((verdict.confidence - penalty).max(0.45), 2);
        verdicts.push(verdict);
    }
    if let Some(note) = tod.note {
        notes.push(note);
    }
    let tod_status = tod.status;
    let next_hours = tod.next_hours;

    // 9. M8 volume-based FUP throttling.
    let m8 = &dataset.m8_time_volume_upload;
    if let Some(step_bytes) = m8.volume_step_down_at_bytes {
        if m8.volume_pre_step_bps > 0.0 && m8.volume_post_step_bps <= m8.volume_pre_step_bps * 0.55
        {
            let drop_pct = (1.0 - m8.volume_post_step_bps / m8.volume_pre_step_bps) * 100.0;
            let conf = (0.86_f64 - penalty).max(0.45);
            verdicts.push(Verdict {
                verdict_class: VerdictClass::VolumeFupThrottling.as_str().to_owned(),
                confidence: py_round(conf, 2),
                summary: format!(
                    "Throughput stepped down by {drop_pct:.0}% (from {} to {}) after transferring {} in one session.",
                    format_speed(m8.volume_pre_step_bps),
                    format_speed(m8.volume_post_step_bps),
                    format_size(Some(step_bytes as f64))
                ),
                evidence: vec![
                    format!(
                        "Pre-threshold throughput: {}",
                        format_speed(m8.volume_pre_step_bps)
                    ),
                    format!(
                        "Post-threshold throughput after {}: {}",
                        format_size(Some(step_bytes as f64)),
                        format_speed(m8.volume_post_step_bps)
                    ),
                ],
            });
        }
    }

    // 10. M10 local vs international vs ISP-cached.
    let m10 = &dataset.m10_local_vs_intl;
    let dom_1 = m10.domestic_median_bps;
    let intl_1 = m10.international_median_bps;
    let cache_1 = m10.isp_cached_median_bps;
    let dom_16 = m10.domestic_16_bps.or(dom_1);
    let intl_16 = m10.international_16_bps.or(intl_1);

    if let (Some(cache), Some(intl)) = (cache_1, intl_1) {
        if intl > 0.0 && cache / intl >= 2.0 {
            let c_ratio = cache / intl;
            notes.push(VerdictNote {
                code: "cache_effect".to_owned(),
                title: "cache effect".to_owned(),
                text: format!(
                    "ISP-cached services ({}) transferred {c_ratio:.1}x faster than comparable uncached international hosts ({}). This reflects local edge cache peering (e.g., GGC/FNA) vs international transit capacity, not service-specific throttling.",
                    format_speed(cache),
                    format_speed(intl)
                ),
                recommendations: vec![
                    "For uncached international files, prefer CDN mirrors with local or regional PoPs when available.".to_owned(),
                ],
            });
        }
    }

    let both_per_flow_capped = dom_flow_scaled && intl_flow_scaled;
    let intl_16_catches_dom = match (dom_16, intl_16) {
        (Some(dom), Some(intl)) => dom > 0.0 && intl / dom >= 0.60,
        _ => false,
    };
    let has_slow_and_fast =
        !m10.slow_international_asns.is_empty() && !m10.fast_international_asns.is_empty();

    if let (Some(dom), Some(intl)) = (dom_1, intl_1) {
        if intl > 0.0 && dom / intl >= 2.2 && !both_per_flow_capped && !intl_16_catches_dom {
            if has_slow_and_fast {
                let conf = (if m10.shared_slow_upstream_asn.is_some() {
                    0.84_f64
                } else {
                    0.74_f64
                })
                .max(0.45)
                    - penalty;
                verdicts.push(Verdict {
                    verdict_class: VerdictClass::PeeringTransitCongestion.as_str().to_owned(),
                    confidence: py_round(conf.max(0.45), 2),
                    summary: format!(
                        "Specific international routes ({}) are slow ({}) via upstream {}, while other international routes ({}) and domestic ({}) remain fast.",
                        m10.slow_international_asns.join(", "),
                        format_speed(intl),
                        m10.shared_slow_upstream_asn.as_deref().unwrap_or("transit"),
                        m10.fast_international_asns.join(", "),
                        format_speed(dom)
                    ),
                    evidence: vec![
                        format!("Domestic median: {}", format_speed(dom)),
                        format!(
                            "Slow international ASNs: {}",
                            m10.slow_international_asns.join(", ")
                        ),
                        format!(
                            "Fast international ASNs: {}",
                            m10.fast_international_asns.join(", ")
                        ),
                        format!(
                            "Shared upstream transit ASN: {}",
                            m10.shared_slow_upstream_asn.as_deref().unwrap_or("N/A")
                        ),
                    ],
                });
            } else {
                let dom_intl_ratio = dom / intl;
                let conf = (0.84 + 0.02 * dom_intl_ratio.min(5.0)).min(0.95) - penalty;
                let mut evidence = vec![
                    format!(
                        "Domestic (BDIX/local) median throughput: {} (16-stream: {})",
                        format_speed(dom),
                        format_speed(dom_16.unwrap_or(dom))
                    ),
                    format!(
                        "International uncached median throughput: {} (16-stream: {})",
                        format_speed(intl),
                        format_speed(intl_16.unwrap_or(intl))
                    ),
                    format!(
                        "Domestic vs international ratio: {dom_intl_ratio:.1}x; flow scaling on international stays capped at {}",
                        format_speed(intl_16.unwrap_or(intl))
                    ),
                ];
                if let Some(asn) = &m10.shared_slow_upstream_asn {
                    evidence.push(format!(
                        "International paths share upstream transit ASN {asn}"
                    ));
                }
                verdicts.push(Verdict {
                    verdict_class: VerdictClass::InternationalBwLimit.as_str().to_owned(),
                    confidence: py_round(conf.max(0.45), 2),
                    summary: format!(
                        "Domestic transfers reach {}, while all uncached international targets share a {} ceiling ({dom_intl_ratio:.1}x slower) that multi-connection scaling does not bypass.",
                        format_speed(dom_16.unwrap_or(dom)),
                        format_speed(intl_16.unwrap_or(intl))
                    ),
                    evidence,
                });
            }
        } else if has_slow_and_fast {
            let conf = (if m10.shared_slow_upstream_asn.is_some() {
                0.83_f64
            } else {
                0.72_f64
            })
            .max(0.45)
                - penalty;
            verdicts.push(Verdict {
                verdict_class: VerdictClass::PeeringTransitCongestion.as_str().to_owned(),
                confidence: py_round(conf.max(0.45), 2),
                summary: format!(
                    "Selective peering/transit congestion on international ASNs {} (sharing upstream {}), while {} remain fast.",
                    m10.slow_international_asns.join(", "),
                    m10.shared_slow_upstream_asn.as_deref().unwrap_or("path"),
                    m10.fast_international_asns.join(", ")
                ),
                evidence: vec![
                    format!(
                        "Slow international ASNs: {}",
                        m10.slow_international_asns.join(", ")
                    ),
                    format!(
                        "Fast international ASNs: {}",
                        m10.fast_international_asns.join(", ")
                    ),
                    format!(
                        "Shared upstream transit ASN: {}",
                        m10.shared_slow_upstream_asn.as_deref().unwrap_or("N/A")
                    ),
                ],
            });
        }
    }

    // 11. Line-capacity / no-throttling fallback.
    let specific_classes: BTreeSet<&str> =
        verdicts.iter().map(|v| v.verdict_class.as_str()).collect();
    let peak_observed_bps = dataset
        .m2_single_flow
        .iter()
        .map(|s| s.peak_bps)
        .chain(
            dataset
                .m3_flow_scaling
                .iter()
                .flat_map(|f| f.points.iter().map(|p| p.aggregate_bps)),
        )
        .chain([dom_16.unwrap_or(0.0), intl_16.unwrap_or(0.0)])
        .fold(0.0_f64, f64::max);

    let plan_bps = match dataset.plan_mbps {
        Some(mbps) if mbps > 0.0 => Some(mbps * 1_000_000.0 / 8.0),
        _ => None,
    };

    if specific_classes.is_empty() {
        let first_fired = plan_bps.is_some()
            && peak_observed_bps >= plan_bps.unwrap_or(1.0) * 0.82
            && !contaminated;
        if first_fired {
            let plan = plan_bps.unwrap_or(1.0);
            let conf = (0.85 + 0.10 * (peak_observed_bps / plan).min(1.0)).min(0.95);
            verdicts.push(Verdict {
                verdict_class: VerdictClass::NoThrottling.as_str().to_owned(),
                confidence: py_round(conf, 2),
                summary: format!(
                    "Peak throughput ({}) reaches {:.0}% of the configured {:.0} Mbps plan with no burst collapse or flow/destination policing.",
                    format_speed(peak_observed_bps),
                    (peak_observed_bps / plan) * 100.0,
                    dataset.plan_mbps.unwrap_or(0.0)
                ),
                evidence: vec![
                    format!(
                        "Peak observed speed: {} vs plan {}",
                        format_speed(peak_observed_bps),
                        format_speed(plan)
                    ),
                    "No burst step-down, per-flow cap, or protocol differentiation detected".to_owned(),
                ],
            });
        } else if !flat_m3.is_empty()
            && m4.independence_ratio < 1.35
            && peak_observed_bps > 0.0
            && (plan_bps.is_none() || peak_observed_bps < plan_bps.unwrap_or(0.0) * 0.75)
            && !contaminated
        {
            let conf = (0.80_f64 - penalty).max(0.45);
            verdicts.push(Verdict {
                verdict_class: VerdictClass::LineCapacityLimit.as_str().to_owned(),
                confidence: py_round(conf, 2),
                summary: format!(
                    "All single-host, multi-connection, and concurrent multi-host transfers share a uniform line ceiling around {}.",
                    format_speed(peak_observed_bps)
                ),
                evidence: vec![
                    format!(
                        "1-stream and 16-stream transfers plateau at {}",
                        format_speed(peak_observed_bps)
                    ),
                    format!(
                        "Concurrent two-host test achieved {} (ratio {:.2}x <= 1.35x)",
                        format_speed(m4.concurrent_total_bps),
                        m4.independence_ratio
                    ),
                ],
            });
        } else if peak_observed_bps >= 10.0 * 1024.0 * 1024.0 && !contaminated {
            verdicts.push(Verdict {
                verdict_class: VerdictClass::NoThrottling.as_str().to_owned(),
                confidence: 0.84,
                summary: format!(
                    "High sustained throughput ({}) with no throttling signatures.",
                    format_speed(peak_observed_bps)
                ),
                evidence: vec![format!(
                    "Peak sustained speed: {}",
                    format_speed(peak_observed_bps)
                )],
            });
        } else {
            verdicts.push(Verdict {
                verdict_class: VerdictClass::Inconclusive.as_str().to_owned(),
                confidence: py_round(if contaminated { 0.42 } else { 0.50 }, 2),
                summary: "Measurements are noisy, sparse, or contaminated; no single throttling policy passed confidence thresholds.".to_owned(),
                evidence: vec![
                    format!("Contaminated environment: {contaminated}"),
                    format!("Peak observed throughput: {}", format_speed(peak_observed_bps)),
                ],
            });
        }
    }

    // Sort verdicts by confidence descending (stable, like Python's `sort`).
    verdicts.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

    // Build the recommended profile.
    let v_set: BTreeSet<&str> = verdicts.iter().map(|v| v.verdict_class.as_str()).collect();
    let mut rec = RecommendedProfile::default();
    if v_set.contains(VerdictClass::NoRangeSupport.as_str()) {
        rec.default_connections = 1;
        rec.international_connections = 1;
        rec.recovery_action = "single_stream".to_owned();
        rec.explanation
            .push("Target server does not support HTTP Range (206); use 1 stream.".to_owned());
    } else if v_set.contains(VerdictClass::BurstThenThrottle.as_str()) {
        rec.default_connections = 16;
        rec.international_connections = 16;
        rec.domestic_connections = 16;
        rec.recycle_interval_sec = 10.0;
        rec.recovery_action = "tier2_pause_unpause".to_owned();
        rec.explanation.push(
            "Burst-then-throttle detected: use 16 streams with Watchdog v2 in-process socket cycling every ~10-15s.".to_owned(),
        );
    } else if v_set.contains(VerdictClass::PerFlowCap.as_str()) {
        rec.default_connections = 16;
        rec.international_connections = 16;
        rec.domestic_connections = 16;
        rec.recovery_action = "tier1_native".to_owned();
        rec.explanation.push(
            "Per-flow cap detected: use 16 parallel connections (`-c 16`) to multiply throughput."
                .to_owned(),
        );
    } else if v_set.contains(VerdictClass::PerDestinationLimit.as_str()) {
        rec.default_connections = 4;
        rec.international_connections = 4;
        rec.use_mirrors = true;
        rec.recovery_action = "tier1_native".to_owned();
        rec.explanation.push(
            "Per-destination cap detected: keep per-host connections modest (4) and add alternate mirrors (`--mirror`) so aria2 pulls from multiple hosts concurrently.".to_owned(),
        );
    } else if v_set.contains(VerdictClass::InternationalBwLimit.as_str()) {
        rec.domestic_connections = 16;
        rec.international_connections = 8;
        rec.default_connections = 8;
        rec.use_mirrors = true;
        rec.explanation.push(
            "International subscriber cap detected: use 16 connections for domestic/BDIX servers, and use mirrors/regional CDN edges for international downloads.".to_owned(),
        );
    } else {
        rec.default_connections = 16;
        rec.domestic_connections = 16;
        rec.international_connections = 8;
        rec.explanation
            .push("Use standard 8-16 parallel streams with Watchdog v2 enabled.".to_owned());
    }
    if v_set.contains(VerdictClass::ProtocolPortDiff.as_str()) {
        rec.try_http3 = true;
        rec.explanation.push(
            "Protocol/port differentiation observed: test HTTP/3 or alternate ports when supported.".to_owned(),
        );
    }
    if v_set.contains(VerdictClass::SniServiceDiff.as_str()) {
        rec.try_tunnel = true;
        rec.explanation.push(
            "SNI/DPI or destination policy observed: routing large transfers via a proxy/tunnel bypasses the cap.".to_owned(),
        );
    }

    // Build the neutral ISP evidence summary.
    let mut dates: BTreeSet<String> = dataset
        .m8_time_volume_upload
        .samples
        .iter()
        .filter(|s| s.timestamp.len() >= 10)
        .map(|s| s.timestamp[..10].to_owned())
        .collect();
    if dates.is_empty() {
        dates.insert("2026-11-07".to_owned());
    }
    let mut tested_hosts: Vec<String> = dataset
        .m2_single_flow
        .iter()
        .filter(|s| !s.host.is_empty())
        .map(|s| s.host.clone())
        .collect();
    tested_hosts.sort();
    tested_hosts.dedup();

    let mut measured_lines: Vec<String> = Vec::new();
    if let Some(dom) = dom_1 {
        measured_lines.push(format!(
            "Domestic / BDIX targets: median 1-stream {}, 16-stream {}",
            format_speed(dom),
            format_speed(dom_16.unwrap_or(dom))
        ));
    }
    if let Some(intl) = intl_1 {
        measured_lines.push(format!(
            "Uncached international targets: median 1-stream {}, 16-stream {}",
            format_speed(intl),
            format_speed(intl_16.unwrap_or(intl))
        ));
    }
    if let Some(cache) = cache_1 {
        measured_lines.push(format!(
            "ISP-cached targets: median 1-stream {}",
            format_speed(cache)
        ));
    }
    for s in dataset.m2_single_flow.iter().take(3) {
        measured_lines.push(format!(
            "Host {} ({}): avg {}, peak {}, sustained {}",
            s.host,
            s.locality,
            format_speed(s.avg_bps),
            format_speed(s.peak_bps),
            format_speed(s.sustained_bps)
        ));
    }
    for p6 in dataset.m6_policing_shaping.iter().take(2) {
        measured_lines.push(format!(
            "Path latency to {}: idle RTT {:.1} ms, loaded RTT {:.1} ms, TCP retransmit delta {:.2}%",
            p6.host, p6.idle_rtt_ms, p6.loaded_rtt_ms, p6.retransmit_rate_pct
        ));
    }

    let questions = vec![
        format!(
            "What is the provisioned international transit bandwidth vs domestic/BDIX bandwidth per subscriber on my {} package?",
            dataset.isp
        ),
        "Which upstream international transit providers (ASNs) and Singapore/European peering links carry traffic for this subnet?".to_owned(),
        "Are there any per-TCP-flow rate shapers, token-bucket burst limits, or session/daily Fair Usage Policy (FUP) thresholds configured on my subscriber profile?".to_owned(),
    ];

    let isp_summary = ISPEvidenceSummary {
        isp: dataset.isp.clone(),
        dates: dates.into_iter().collect(),
        targets_tested: tested_hosts,
        methodology: vec![
            "Controlled HTTP/HTTPS range transfers using 1, 2, 4, 8, and 16 parallel TCP streams (250 ms sampling).".to_owned(),
            "Paired same-moment comparisons across domestic (BDIX), ISP-cached, and uncached international endpoints.".to_owned(),
            "Idle vs under-load ICMP/TCP RTT and kernel TCP retransmission counter deltas (`netstat -s -p tcp`).".to_owned(),
        ],
        measured_numbers: measured_lines,
        neutral_questions: questions,
    };

    let primary_mode = if tod_status == "insufficient time data" {
        "paired_same_moment"
    } else {
        "paired_and_time_of_day"
    };

    ClassificationResult {
        verdicts,
        notes,
        recommended_profile: rec,
        isp_evidence: isp_summary,
        time_of_day_status: tod_status,
        next_best_hours: next_hours,
        primary_evidence_mode: primary_mode.to_owned(),
    }
}
