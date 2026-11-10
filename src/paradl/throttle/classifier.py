"""Pure rule-based ISP throttling classifier and statistical significance engine.

Contains zero I/O and zero clock calls so it is 100% unit-testable with factory datasets.
"""

from __future__ import annotations

import math
import statistics
from typing import Any

from paradl.models import format_size, format_speed
from paradl.throttle.models import (
    ClassificationResult,
    ISPEvidenceSummary,
    MeasurementDataset,
    RecommendedProfile,
    SampleRecord,
    Verdict,
    VerdictClass,
    VerdictNote,
)
from paradl.throttle.store import ThrottleStore


def mann_whitney_u_test(sample_a: list[float], sample_b: list[float]) -> tuple[float, float]:
    """Compute Mann-Whitney U statistic and two-sided normal-approximate p-value (with tie correction).

    Pure standard-library implementation requiring no external scipy dependency.
    """
    n1 = len(sample_a)
    n2 = len(sample_b)
    if n1 == 0 or n2 == 0:
        return 0.0, 1.0

    combined: list[tuple[float, int]] = [(float(x), 0) for x in sample_a] + [
        (float(y), 1) for y in sample_b
    ]
    combined.sort(key=lambda pair: pair[0])

    n_total = n1 + n2
    ranks = [0.0] * n_total
    tie_counts: list[int] = []

    i = 0
    while i < n_total:
        j = i + 1
        while j < n_total and combined[j][0] == combined[i][0]:
            j += 1
        # Average rank for 1-based indices (i+1 .. j)
        avg_rank = (i + 1 + j) / 2.0
        for k in range(i, j):
            ranks[k] = avg_rank
        if (j - i) > 1:
            tie_counts.append(j - i)
        i = j

    r1 = sum(ranks[idx] for idx in range(n_total) if combined[idx][1] == 0)
    u1 = r1 - (n1 * (n1 + 1)) / 2.0
    u2 = (n1 * n2) - u1
    u_stat = min(u1, u2)

    mean_u = (n1 * n2) / 2.0
    tie_term = sum(t**3 - t for t in tie_counts) / (n_total * (n_total - 1)) if n_total > 1 else 0.0
    var_u = (n1 * n2 / 12.0) * ((n_total + 1) - tie_term)
    if var_u <= 0:
        return u_stat, 1.0

    # Continuity correction
    z = (abs(u1 - mean_u) - 0.5) / math.sqrt(var_u)
    if z <= 0:
        return u_stat, 1.0
    # Two-sided p-value from standard normal survival function via math.erfc
    p_val = math.erfc(z / math.sqrt(2.0))
    return round(u_stat, 3), round(min(1.0, max(0.0, p_val)), 6)


def _evaluate_time_of_day(
    samples: list[SampleRecord],
) -> tuple[Verdict | None, VerdictNote | None, str, list[str]]:
    """Evaluate M8 time-of-day samples with strict coverage and Mann-Whitney U significance gates."""
    _matrix, next_best_hours = ThrottleStore.build_coverage_matrix(samples)
    if not samples:
        note = VerdictNote(
            code="insufficient_time_data",
            title="insufficient time data",
            text="No multi-hour samples recorded yet; relying on paired same-moment comparisons.",
            recommendations=next_best_hours,
        )
        return None, note, "insufficient time data", next_best_hours

    # Group samples into 4 time-of-day windows per day-type (weekday vs weekend):
    # W0: Night (00..05), W1: Morning (06..11), W2: Afternoon (12..17), W3: Evening Peak (18..23)
    buckets: dict[tuple[bool, int], list[SampleRecord]] = {}
    for s in samples:
        if s.download_bps > 0:
            win_id = (int(s.hour) % 24) // 6
            buckets.setdefault((bool(s.is_weekend), win_id), []).append(s)

    # Check if any single day-type (weekday or weekend) has at least 2 distinct time windows
    # each meeting the sample floor (count >= 3 and effective_weight >= 2.5)
    qualifying_by_daytype: dict[bool, list[tuple[int, list[SampleRecord]]]] = {
        False: [],
        True: [],
    }
    for (is_wk, win_id), b_list in buckets.items():
        eff_w = sum(x.weight for x in b_list)
        if len(b_list) >= 3 and eff_w >= 2.5:
            qualifying_by_daytype[is_wk].append((win_id, b_list))

    best_daytype_windows = (
        qualifying_by_daytype[False] if len(qualifying_by_daytype[False]) >= 2 else []
    )

    if len(best_daytype_windows) < 2:
        note = VerdictNote(
            code="insufficient_time_data",
            title="insufficient time data",
            text=(
                "Time-of-day samples do not yet cover at least two distinct time-of-day windows "
                "with >= 3 samples each; refusing to guess time-of-day congestion."
            ),
            recommendations=next_best_hours,
        )
        return None, note, "insufficient time data", next_best_hours

    # Find fastest and slowest qualifying windows
    window_medians: list[tuple[float, int, list[SampleRecord]]] = []
    for win_id, b_list in best_daytype_windows:
        med = float(statistics.median(x.download_bps for x in b_list))
        window_medians.append((med, win_id, b_list))
    window_medians.sort(key=lambda item: item[0], reverse=True)

    fast_med, fast_win, fast_list = window_medians[0]
    slow_med, slow_win, slow_list = window_medians[-1]

    drop_ratio = slow_med / max(fast_med, 1.0)
    u_stat, p_val = mann_whitney_u_test(
        [x.download_bps for x in fast_list],
        [x.download_bps for x in slow_list],
    )

    win_names = {0: "00:00-05:59", 1: "06:00-11:59", 2: "12:00-17:59", 3: "18:00-23:59"}
    if drop_ratio <= 0.65 and p_val <= 0.05:
        conf = min(0.94, 0.78 + (1.0 - drop_ratio) * 0.25)
        verdict = Verdict(
            verdict_class=VerdictClass.TIME_OF_DAY_CONGESTION.value,
            confidence=round(conf, 2),
            summary=(
                f"Statistically significant throughput drop during {win_names.get(slow_win, 'peak')} "
                f"({format_speed(slow_med)}) compared to {win_names.get(fast_win, 'off-peak')} "
                f"({format_speed(fast_med)})."
            ),
            evidence=[
                f"Off-peak window ({win_names.get(fast_win)}) median: {format_speed(fast_med)} (n={len(fast_list)})",
                f"Peak window ({win_names.get(slow_win)}) median: {format_speed(slow_med)} (n={len(slow_list)})",
                f"Mann-Whitney U={u_stat}, two-sided p={p_val:.4f} (effect size {(1.0 - drop_ratio) * 100:.0f}% drop)",
            ],
        )
        return verdict, None, "significant time-of-day variation detected", next_best_hours

    return None, None, "no significant time-of-day slowdown", next_best_hours


def classify_dataset(dataset: MeasurementDataset) -> ClassificationResult:
    """Pure rule-based classifier mapping a `MeasurementDataset` to `ClassificationResult`."""
    verdicts: list[Verdict] = []
    notes: list[VerdictNote] = []

    contaminated = bool(dataset.m0_sanity.contaminated)
    penalty = 0.22 if contaminated else 0.0
    if contaminated:
        notes.append(
            VerdictNote(
                code="contaminated_environment",
                title="contaminated local environment",
                text="Local environment warnings were triggered during M0 sanity checks; confidence scores are reduced.",
                recommendations=list(dataset.m0_sanity.warnings),
            )
        )

    # 1. Check 200-only (no Range support)
    no_range_hosts = [s.host for s in dataset.m2_single_flow if not s.supports_range] + [
        f.host for f in dataset.m3_flow_scaling if not f.supports_range
    ]
    if no_range_hosts and all(not s.supports_range for s in dataset.m2_single_flow if s.host):
        h_str = ", ".join(sorted(set(no_range_hosts)))
        verdicts.append(
            Verdict(
                verdict_class=VerdictClass.NO_RANGE_SUPPORT.value,
                confidence=round(max(0.55, 0.92 - penalty), 2),
                summary=f"Target server ({h_str}) returned HTTP 200 OK only and ignored byte-range requests.",
                evidence=[
                    f"Range probe on {h_str} returned HTTP 200 instead of 206 Partial Content",
                    "Multi-connection segmented downloads and socket recycling are disabled for 200-only servers",
                ],
            )
        )

    # 2. Check M3 Flow Scaling -> per-flow cap
    intl_flow_scaled = False
    dom_flow_scaled = False
    max_scale_16 = 1.0
    best_m3: Any = None
    for f in dataset.m3_flow_scaling:
        if not f.supports_range or not f.points:
            continue
        r16 = f.scaling_ratio_16_to_1
        r4 = f.scaling_ratio_4_to_1
        if r16 >= 2.0 or r4 >= 1.75:
            if f.locality == "domestic":
                dom_flow_scaled = True
            else:
                intl_flow_scaled = True
            if r16 >= max_scale_16 or best_m3 is None:
                max_scale_16 = max(r16, r4)
                best_m3 = f

    if best_m3 is not None:
        p1 = best_m3.points[0].aggregate_bps
        p_last = best_m3.points[-1].aggregate_bps
        n_last = best_m3.points[-1].connections
        conf = min(0.95, 0.72 + 0.04 * min(max_scale_16, 6.0)) - penalty
        scope_note = (
            "across both domestic and international paths"
            if (dom_flow_scaled and intl_flow_scaled)
            else f"on {best_m3.host}"
        )
        verdicts.append(
            Verdict(
                verdict_class=VerdictClass.PER_FLOW_CAP.value,
                confidence=round(max(0.45, conf), 2),
                summary=(
                    f"Each TCP connection is capped (~{format_speed(p1)}), while {n_last} parallel connections "
                    f"scale aggregate speed {max_scale_16:.1f}x to {format_speed(p_last)} ({scope_note})."
                ),
                evidence=[
                    f"1 connection on {best_m3.host}: {format_speed(p1)}",
                    f"{n_last} connections on {best_m3.host}: {format_speed(p_last)} ({max_scale_16:.2f}x scaling)",
                    f"Per-flow rate at {n_last} streams: {format_speed(best_m3.points[-1].per_flow_bps)}",
                ],
            )
        )

    # 3. Check M5 Burst-then-Throttle
    for b in dataset.m5_burst_bucket:
        if (
            b.initial_burst_bps > 0
            and b.sustained_bps > 0
            and b.burst_to_sustained_ratio >= 1.75
            and b.drop_shape in ("step", "decay")
        ):
            conf = min(0.95, 0.76 + 0.05 * min(b.burst_to_sustained_ratio, 4.0)) - penalty
            ev = [
                f"Initial burst on {b.host or b.target_id}: {format_speed(b.initial_burst_bps)} for {b.burst_duration_sec:.2f}s",
                f"Sustained rate after drop ({b.drop_shape}): {format_speed(b.sustained_bps)} ({b.burst_to_sustained_ratio:.1f}x collapse)",
                f"Estimated token bucket size: {format_size(b.estimated_bucket_bytes)}",
            ]
            if b.refill_30s_ratio is not None:
                ev.append(
                    f"30s idle refill ratio: {b.refill_30s_ratio * 100:.0f}% of initial burst"
                )
            if b.refill_120s_ratio is not None:
                ev.append(
                    f"120s idle refill ratio: {b.refill_120s_ratio * 100:.0f}% of initial burst"
                )
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.BURST_THEN_THROTTLE.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"Connections start with a fast burst ({format_speed(b.initial_burst_bps)}) for "
                        f"~{b.burst_duration_sec:.1f}s before dropping ({b.drop_shape}) to {format_speed(b.sustained_bps)}."
                    ),
                    evidence=ev,
                )
            )
            break

    # 4. Check M4 Destination Independence & Flat M3 -> per-destination limit (shared host cap)
    m4 = dataset.m4_destination_independence
    flat_m3 = [
        f
        for f in dataset.m3_flow_scaling
        if f.supports_range
        and len(f.points) >= 2
        and f.scaling_ratio_16_to_1 < 1.48
        and f.scaling_ratio_4_to_1 < 1.45
    ]
    if flat_m3 and m4.independence_ratio >= 1.45:
        f0 = flat_m3[0]
        ceil_spd = f0.points[-1].aggregate_bps
        conf = min(0.93, 0.75 + 0.20 * min(1.0, (m4.independence_ratio - 1.4) / 0.6)) - penalty
        verdicts.append(
            Verdict(
                verdict_class=VerdictClass.PER_DESTINATION_LIMIT.value,
                confidence=round(max(0.45, conf), 2),
                summary=(
                    f"Aggregate throughput to a single host is capped around {format_speed(ceil_spd)} regardless of "
                    f"connection count, but two concurrent hosts achieve {format_speed(m4.concurrent_total_bps)} "
                    f"({m4.independence_ratio:.2f}x)."
                ),
                evidence=[
                    f"Flow scaling on {f0.host} plateaued at {format_speed(ceil_spd)} (16x/1x ratio = {f0.scaling_ratio_16_to_1:.2f})",
                    f"Solo {m4.host_a}: {format_speed(m4.solo_a_bps)}, Solo {m4.host_b}: {format_speed(m4.solo_b_bps)}",
                    f"Concurrent {m4.host_a} + {m4.host_b}: {format_speed(m4.concurrent_total_bps)} (independence ratio {m4.independence_ratio:.2f}x)",
                ],
            )
        )

    # 5. Check M6 Policing vs Shaping
    for p6 in dataset.m6_policing_shaping:
        if p6.mechanism_hint == "policing" or (
            (p6.retransmit_rate_pct >= 1.5 or p6.connection_resets > 0)
            and p6.rtt_inflation_ms < 25.0
        ):
            conf = (
                0.86 if (p6.retransmit_rate_pct >= 2.0 or p6.connection_resets > 0) else 0.72
            ) - penalty
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.LOSS_BASED_POLICING.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"Hard packet-drop/reset policing detected on {p6.host} "
                        f"(retransmit rate {p6.retransmit_rate_pct:.2f}%, resets={p6.connection_resets}, "
                        f"minimal RTT growth +{p6.rtt_inflation_ms:.1f} ms)."
                    ),
                    evidence=[
                        f"TCP retransmit rate delta: {p6.retransmit_rate_pct:.2f}% ({p6.tcp_retransmits_delta} pkts, {p6.connection_resets} resets)",
                        f"RTT under load ({p6.loaded_rtt_ms:.1f} ms) vs idle ({p6.idle_rtt_ms:.1f} ms): +{p6.rtt_inflation_ms:.1f} ms",
                        f"Throughput CoV: {p6.throughput_cov:.2f}",
                    ],
                )
            )
            break
        if p6.mechanism_hint == "shaping" or (
            p6.rtt_inflation_ms >= 35.0
            and p6.retransmit_rate_pct < 1.0
            and p6.throughput_cov < 0.25
        ):
            conf = (0.87 if p6.rtt_inflation_ms >= 45.0 else 0.76) - penalty
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.QUEUE_BASED_SHAPING.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"Queue-based traffic shaping detected on {p6.host}: smooth rate cap (CoV={p6.throughput_cov:.2f}) "
                        f"with significant buffer queueing delay (+{p6.rtt_inflation_ms:.1f} ms RTT growth under load)."
                    ),
                    evidence=[
                        f"Idle RTT: {p6.idle_rtt_ms:.1f} ms -> Loaded RTT: {p6.loaded_rtt_ms:.1f} ms (+{p6.rtt_inflation_ms:.1f} ms)",
                        f"Low packet retransmission ({p6.retransmit_rate_pct:.2f}%) with smooth rate (CoV {p6.throughput_cov:.2f})",
                    ],
                )
            )
            break

    # 6. Check M7 Protocol & Port Differentiation
    for p7 in dataset.m7_protocol_port:
        diffs: list[tuple[str, float, float, float]] = []
        if p7.http3_bps and p7.http1_1_bps and p7.http1_1_bps > 0:
            r = max(p7.http3_bps, p7.http1_1_bps) / max(min(p7.http3_bps, p7.http1_1_bps), 1.0)
            if r >= 1.75:
                diffs.append(("HTTP/3 vs HTTP/1.1", p7.http3_bps, p7.http1_1_bps, r))
        if p7.https_bps and p7.http_plain_bps and min(p7.https_bps, p7.http_plain_bps) > 0:
            r = max(p7.https_bps, p7.http_plain_bps) / min(p7.https_bps, p7.http_plain_bps)
            if r >= 1.75:
                diffs.append(("HTTPS vs plain HTTP", p7.https_bps, p7.http_plain_bps, r))
        if p7.port_443_bps and p7.alt_port_bps and min(p7.port_443_bps, p7.alt_port_bps) > 0:
            r = max(p7.port_443_bps, p7.alt_port_bps) / min(p7.port_443_bps, p7.alt_port_bps)
            if r >= 1.75:
                diffs.append(
                    (f"Port 443 vs Port {p7.alt_port}", p7.port_443_bps, p7.alt_port_bps, r)
                )
        if p7.ipv4_bps and p7.ipv6_bps and min(p7.ipv4_bps, p7.ipv6_bps) > 0:
            r = max(p7.ipv4_bps, p7.ipv6_bps) / min(p7.ipv4_bps, p7.ipv6_bps)
            if r >= 1.75:
                diffs.append(("IPv4 vs IPv6", p7.ipv4_bps, p7.ipv6_bps, r))

        if diffs:
            label, s_a, s_b, ratio = diffs[0]
            conf = min(0.93, 0.78 + 0.05 * min(ratio, 4.0)) - penalty
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.PROTOCOL_PORT_DIFF.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=f"Throughput differs by {ratio:.1f}x across {label} on {p7.host} ({format_speed(s_a)} vs {format_speed(s_b)}).",
                    evidence=[
                        f"{d[0]} on {p7.host}: {format_speed(d[1])} vs {format_speed(d[2])} ({d[3]:.2f}x)"
                        for d in diffs
                    ],
                )
            )
            break

    # 7. Check M7 SNI or M9 Proxy / Payload-type Differentiation
    m9 = dataset.m9_ground_truth
    sni_triggered = False
    for p7 in dataset.m7_protocol_port:
        if p7.sni_default_bps and p7.sni_alt_bps and p7.sni_default_bps > 0:
            sni_ratio = p7.sni_alt_bps / p7.sni_default_bps
            if sni_ratio >= 1.80:
                sni_triggered = True
                conf = max(0.45, 0.72 - penalty)  # labeled best-effort
                verdicts.append(
                    Verdict(
                        verdict_class=VerdictClass.SNI_SERVICE_DIFF.value,
                        confidence=round(conf, 2),
                        summary=(
                            f"Best-effort TLS SNI probe on {p7.host} achieved {sni_ratio:.1f}x higher speed "
                            f"with alternate SNI ({format_speed(p7.sni_alt_bps)} vs {format_speed(p7.sni_default_bps)})."
                        ),
                        evidence=[
                            f"Default SNI on {p7.host}: {format_speed(p7.sni_default_bps)}",
                            f"Alternate SNI ({p7.sni_label}): {format_speed(p7.sni_alt_bps)} ({sni_ratio:.2f}x)",
                        ],
                    )
                )
                break

    if not sni_triggered:
        if m9.proxy_speedup_ratio is not None and m9.proxy_speedup_ratio >= 1.85:
            conf = min(0.95, 0.85 + 0.03 * min(m9.proxy_speedup_ratio, 4.0)) - penalty
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.SNI_SERVICE_DIFF.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"Paired tunnel comparison (--via-proxy) is {m9.proxy_speedup_ratio:.1f}x faster than direct "
                        f"({format_speed(m9.proxy_1_bps or 0)} vs {format_speed(m9.direct_1_bps or 0)}), "
                        "indicating destination or DPI-based throttling rather than line capacity."
                    ),
                    evidence=[
                        f"Direct 1-stream: {format_speed(m9.direct_1_bps or 0)} vs Proxy 1-stream: {format_speed(m9.proxy_1_bps or 0)}",
                        f"Proxy speedup ratio: {m9.proxy_speedup_ratio:.2f}x",
                    ],
                )
            )
        elif (
            m9.own_server_random_bps
            and m9.own_server_video_bps
            and m9.own_server_video_bps > 0
            and (m9.own_server_random_bps / m9.own_server_video_bps) >= 1.80
        ):
            r_pay = m9.own_server_random_bps / m9.own_server_video_bps
            conf = min(0.94, 0.84 + 0.03 * min(r_pay, 4.0)) - penalty
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.SNI_SERVICE_DIFF.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"VPS own-server payload comparison shows video-labeled bytes are throttled {r_pay:.1f}x "
                        f"relative to random bytes ({format_speed(m9.own_server_video_bps)} vs {format_speed(m9.own_server_random_bps)})."
                    ),
                    evidence=[
                        f"Own-server random payload: {format_speed(m9.own_server_random_bps)}",
                        f"Own-server video/mp4 payload: {format_speed(m9.own_server_video_bps)} ({r_pay:.2f}x slower)",
                    ],
                )
            )

    # 8. Check M8 Time-of-Day Congestion (with strict coverage & Mann-Whitney U gate)
    tod_verdict, tod_note, tod_status, next_hours = _evaluate_time_of_day(
        dataset.m8_time_volume_upload.samples
    )
    if tod_verdict is not None:
        tod_verdict.confidence = round(max(0.45, tod_verdict.confidence - penalty), 2)
        verdicts.append(tod_verdict)
    if tod_note is not None:
        notes.append(tod_note)

    # 9. Check M8 Volume-Based (FUP) Throttling
    m8 = dataset.m8_time_volume_upload
    if (
        m8.volume_step_down_at_bytes is not None
        and m8.volume_pre_step_bps > 0
        and m8.volume_post_step_bps <= m8.volume_pre_step_bps * 0.55
    ):
        drop_pct = (1.0 - (m8.volume_post_step_bps / m8.volume_pre_step_bps)) * 100.0
        conf = max(0.45, 0.86 - penalty)
        verdicts.append(
            Verdict(
                verdict_class=VerdictClass.VOLUME_FUP_THROTTLING.value,
                confidence=round(conf, 2),
                summary=(
                    f"Throughput stepped down by {drop_pct:.0f}% (from {format_speed(m8.volume_pre_step_bps)} "
                    f"to {format_speed(m8.volume_post_step_bps)}) after transferring "
                    f"{format_size(m8.volume_step_down_at_bytes)} in one session."
                ),
                evidence=[
                    f"Pre-threshold throughput: {format_speed(m8.volume_pre_step_bps)}",
                    f"Post-threshold throughput after {format_size(m8.volume_step_down_at_bytes)}: {format_speed(m8.volume_post_step_bps)}",
                ],
            )
        )

    # 10. Check M10 Local vs International vs ISP-Cached
    m10 = dataset.m10_local_vs_intl
    dom_1 = m10.domestic_median_bps
    intl_1 = m10.international_median_bps
    cache_1 = m10.isp_cached_median_bps
    dom_16 = m10.domestic_16_bps or dom_1
    intl_16 = m10.international_16_bps or intl_1

    # Check "cache effect" note first
    if cache_1 is not None and intl_1 is not None and intl_1 > 0 and (cache_1 / intl_1) >= 2.0:
        c_ratio = cache_1 / intl_1
        notes.append(
            VerdictNote(
                code="cache_effect",
                title="cache effect",
                text=(
                    f"ISP-cached services ({format_speed(cache_1)}) transferred {c_ratio:.1f}x faster than "
                    f"comparable uncached international hosts ({format_speed(intl_1)}). This reflects local "
                    "edge cache peering (e.g., GGC/FNA) vs international transit capacity, not service-specific throttling."
                ),
                recommendations=[
                    "For uncached international files, prefer CDN mirrors with local or regional PoPs when available."
                ],
            )
        )

    # Distinguish:
    # (a) Per-flow cap affecting domestic & international alike -> MUST NOT be labeled international bandwidth limit
    # (b) International bandwidth limit -> domestic is much faster than international, all international targets share
    #     the slowdown, and flow scaling on international does NOT close the gap to domestic.
    # (c) Peering/transit congestion -> specific international ASNs/upstreams are slow while other international ASNs are fast.
    both_per_flow_capped = dom_flow_scaled and intl_flow_scaled
    intl_16_catches_dom = (
        dom_16 is not None and intl_16 is not None and dom_16 > 0 and (intl_16 / dom_16) >= 0.60
    )

    if (
        dom_1 is not None
        and intl_1 is not None
        and intl_1 > 0
        and (dom_1 / intl_1) >= 2.2
        and not both_per_flow_capped
        and not intl_16_catches_dom
    ):
        if m10.fast_international_asns and m10.slow_international_asns:
            # Only a subset of international ASNs are slow -> peering/transit congestion
            conf = max(0.45, (0.84 if m10.shared_slow_upstream_asn else 0.74) - penalty)
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.PEERING_TRANSIT_CONGESTION.value,
                    confidence=round(conf, 2),
                    summary=(
                        f"Specific international routes ({', '.join(m10.slow_international_asns)}) are slow "
                        f"({format_speed(intl_1)}) via upstream {m10.shared_slow_upstream_asn or 'transit'}, "
                        f"while other international routes ({', '.join(m10.fast_international_asns)}) and domestic "
                        f"({format_speed(dom_1)}) remain fast."
                    ),
                    evidence=[
                        f"Domestic median: {format_speed(dom_1)}",
                        f"Slow international ASNs: {', '.join(m10.slow_international_asns)}",
                        f"Fast international ASNs: {', '.join(m10.fast_international_asns)}",
                        f"Shared upstream transit ASN: {m10.shared_slow_upstream_asn or 'N/A'}",
                    ],
                )
            )
        else:
            # All uncached international targets share the slowdown and flow scaling does not fix it
            dom_intl_ratio = dom_1 / intl_1
            conf = min(0.95, 0.84 + 0.02 * min(dom_intl_ratio, 5.0)) - penalty
            ev_intl = [
                f"Domestic (BDIX/local) median throughput: {format_speed(dom_1)} (16-stream: {format_speed(dom_16 or dom_1)})",
                f"International uncached median throughput: {format_speed(intl_1)} (16-stream: {format_speed(intl_16 or intl_1)})",
                f"Domestic vs international ratio: {dom_intl_ratio:.1f}x; flow scaling on international stays capped at {format_speed(intl_16 or intl_1)}",
            ]
            if m10.shared_slow_upstream_asn:
                ev_intl.append(
                    f"International paths share upstream transit ASN {m10.shared_slow_upstream_asn}"
                )
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.INTERNATIONAL_BW_LIMIT.value,
                    confidence=round(max(0.45, conf), 2),
                    summary=(
                        f"Domestic transfers reach {format_speed(dom_16 or dom_1)}, while all uncached international "
                        f"targets share a {format_speed(intl_16 or intl_1)} ceiling ({dom_intl_ratio:.1f}x slower) "
                        "that multi-connection scaling does not bypass."
                    ),
                    evidence=ev_intl,
                )
            )
    elif m10.fast_international_asns and m10.slow_international_asns:
        conf = max(0.45, (0.83 if m10.shared_slow_upstream_asn else 0.72) - penalty)
        verdicts.append(
            Verdict(
                verdict_class=VerdictClass.PEERING_TRANSIT_CONGESTION.value,
                confidence=round(conf, 2),
                summary=(
                    f"Selective peering/transit congestion on international ASNs {', '.join(m10.slow_international_asns)} "
                    f"(sharing upstream {m10.shared_slow_upstream_asn or 'path'}), while {', '.join(m10.fast_international_asns)} remain fast."
                ),
                evidence=[
                    f"Slow international ASNs: {', '.join(m10.slow_international_asns)}",
                    f"Fast international ASNs: {', '.join(m10.fast_international_asns)}",
                    f"Shared upstream transit ASN: {m10.shared_slow_upstream_asn or 'N/A'}",
                ],
            )
        )

    # 11. Check Line-Capacity Limit vs No-Throttling when no policy/shaping verdict fired
    specific_classes = {v.verdict_class for v in verdicts}
    peak_observed_bps = max(
        [s.peak_bps for s in dataset.m2_single_flow]
        + [p.aggregate_bps for f in dataset.m3_flow_scaling for p in f.points]
        + [dom_16 or 0.0, intl_16 or 0.0],
        default=0.0,
    )
    plan_bps = (
        (dataset.plan_mbps * 1_000_000.0 / 8.0)
        if (dataset.plan_mbps and dataset.plan_mbps > 0)
        else None
    )

    if not specific_classes:
        if plan_bps is not None and peak_observed_bps >= plan_bps * 0.82 and not contaminated:
            conf = min(0.95, 0.85 + 0.10 * min(1.0, peak_observed_bps / plan_bps))
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.NO_THROTTLING.value,
                    confidence=round(conf, 2),
                    summary=(
                        f"Peak throughput ({format_speed(peak_observed_bps)}) reaches "
                        f"{(peak_observed_bps / plan_bps) * 100:.0f}% of the configured {dataset.plan_mbps:.0f} Mbps plan "
                        "with no burst collapse or flow/destination policing."
                    ),
                    evidence=[
                        f"Peak observed speed: {format_speed(peak_observed_bps)} vs plan {format_speed(plan_bps)}",
                        "No burst step-down, per-flow cap, or protocol differentiation detected",
                    ],
                )
            )
        elif (
            flat_m3
            and m4.independence_ratio < 1.35
            and peak_observed_bps > 0
            and (plan_bps is None or peak_observed_bps < plan_bps * 0.75)
            and not contaminated
        ):
            conf = max(0.45, 0.80 - penalty)
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.LINE_CAPACITY_LIMIT.value,
                    confidence=round(conf, 2),
                    summary=(
                        f"All single-host, multi-connection, and concurrent multi-host transfers share a uniform "
                        f"line ceiling around {format_speed(peak_observed_bps)}."
                    ),
                    evidence=[
                        f"1-stream and 16-stream transfers plateau at {format_speed(peak_observed_bps)}",
                        f"Concurrent two-host test achieved {format_speed(m4.concurrent_total_bps)} (ratio {m4.independence_ratio:.2f}x <= 1.35x)",
                    ],
                )
            )
        elif peak_observed_bps >= 10 * 1024 * 1024 and not contaminated:
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.NO_THROTTLING.value,
                    confidence=0.84,
                    summary=f"High sustained throughput ({format_speed(peak_observed_bps)}) with no throttling signatures.",
                    evidence=[f"Peak sustained speed: {format_speed(peak_observed_bps)}"],
                )
            )
        else:
            verdicts.append(
                Verdict(
                    verdict_class=VerdictClass.INCONCLUSIVE.value,
                    confidence=round(0.42 if contaminated else 0.50, 2),
                    summary="Measurements are noisy, sparse, or contaminated; no single throttling policy passed confidence thresholds.",
                    evidence=[
                        f"Contaminated environment: {contaminated}",
                        f"Peak observed throughput: {format_speed(peak_observed_bps)}",
                    ],
                )
            )

    # Sort verdicts by confidence descending
    verdicts.sort(key=lambda v: v.confidence, reverse=True)

    # Build RecommendedProfile based on active verdicts
    v_set = {v.verdict_class for v in verdicts}
    rec = RecommendedProfile(source="throttle-probe")
    if VerdictClass.NO_RANGE_SUPPORT.value in v_set:
        rec.default_connections = 1
        rec.international_connections = 1
        rec.recovery_action = "single_stream"
        rec.explanation.append("Target server does not support HTTP Range (206); use 1 stream.")
    elif VerdictClass.BURST_THEN_THROTTLE.value in v_set:
        rec.default_connections = 16
        rec.international_connections = 16
        rec.domestic_connections = 16
        rec.recycle_interval_sec = 10.0
        rec.recovery_action = "tier2_pause_unpause"
        rec.explanation.append(
            "Burst-then-throttle detected: use 16 streams with Watchdog v2 in-process socket cycling every ~10-15s."
        )
    elif VerdictClass.PER_FLOW_CAP.value in v_set:
        rec.default_connections = 16
        rec.international_connections = 16
        rec.domestic_connections = 16
        rec.recovery_action = "tier1_native"
        rec.explanation.append(
            "Per-flow cap detected: use 16 parallel connections (`-c 16`) to multiply throughput."
        )
    elif VerdictClass.PER_DESTINATION_LIMIT.value in v_set:
        rec.default_connections = 4
        rec.international_connections = 4
        rec.use_mirrors = True
        rec.recovery_action = "tier1_native"
        rec.explanation.append(
            "Per-destination cap detected: keep per-host connections modest (4) and add alternate mirrors (`--mirror`) so aria2 pulls from multiple hosts concurrently."
        )
    elif VerdictClass.INTERNATIONAL_BW_LIMIT.value in v_set:
        rec.domestic_connections = 16
        rec.international_connections = 8
        rec.default_connections = 8
        rec.use_mirrors = True
        rec.explanation.append(
            "International subscriber cap detected: use 16 connections for domestic/BDIX servers, and use mirrors/regional CDN edges for international downloads."
        )
    else:
        rec.default_connections = 16
        rec.domestic_connections = 16
        rec.international_connections = 8
        rec.explanation.append("Use standard 8-16 parallel streams with Watchdog v2 enabled.")

    if VerdictClass.PROTOCOL_PORT_DIFF.value in v_set:
        rec.try_http3 = True
        rec.explanation.append(
            "Protocol/port differentiation observed: test HTTP/3 or alternate ports when supported."
        )
    if VerdictClass.SNI_SERVICE_DIFF.value in v_set:
        rec.try_tunnel = True
        rec.explanation.append(
            "SNI/DPI or destination policy observed: routing large transfers via a proxy/tunnel bypasses the cap."
        )

    # Build Neutral ISP Evidence Summary & Questions
    dates = sorted(
        {s.timestamp[:10] for s in dataset.m8_time_volume_upload.samples if len(s.timestamp) >= 10}
    ) or ["2026-11-07"]
    tested_hosts = sorted({s.host for s in dataset.m2_single_flow if s.host})
    measured_lines: list[str] = []
    if dom_1 is not None:
        measured_lines.append(
            f"Domestic / BDIX targets: median 1-stream {format_speed(dom_1)}, 16-stream {format_speed(dom_16 or dom_1)}"
        )
    if intl_1 is not None:
        measured_lines.append(
            f"Uncached international targets: median 1-stream {format_speed(intl_1)}, 16-stream {format_speed(intl_16 or intl_1)}"
        )
    if cache_1 is not None:
        measured_lines.append(f"ISP-cached targets: median 1-stream {format_speed(cache_1)}")
    measured_lines.extend(
        f"Host {s2.host} ({s2.locality}): avg {format_speed(s2.avg_bps)}, peak {format_speed(s2.peak_bps)}, sustained {format_speed(s2.sustained_bps)}"
        for s2 in dataset.m2_single_flow[:3]
    )
    measured_lines.extend(
        f"Path latency to {p6.host}: idle RTT {p6.idle_rtt_ms:.1f} ms, loaded RTT {p6.loaded_rtt_ms:.1f} ms, TCP retransmit delta {p6.retransmit_rate_pct:.2f}%"
        for p6 in dataset.m6_policing_shaping[:2]
    )

    questions = [
        f"What is the provisioned international transit bandwidth vs domestic/BDIX bandwidth per subscriber on my {dataset.isp} package?",
        "Which upstream international transit providers (ASNs) and Singapore/European peering links carry traffic for this subnet?",
        "Are there any per-TCP-flow rate shapers, token-bucket burst limits, or session/daily Fair Usage Policy (FUP) thresholds configured on my subscriber profile?",
    ]

    isp_summary = ISPEvidenceSummary(
        isp=dataset.isp,
        dates=dates,
        targets_tested=tested_hosts,
        methodology=[
            "Controlled HTTP/HTTPS range transfers using 1, 2, 4, 8, and 16 parallel TCP streams (250 ms sampling).",
            "Paired same-moment comparisons across domestic (BDIX), ISP-cached, and uncached international endpoints.",
            "Idle vs under-load ICMP/TCP RTT and kernel TCP retransmission counter deltas (`netstat -s -p tcp`).",
        ],
        measured_numbers=measured_lines,
        neutral_questions=questions,
    )

    primary_mode = (
        "paired_same_moment" if tod_status == "insufficient time data" else "paired_and_time_of_day"
    )

    return ClassificationResult(
        verdicts=verdicts,
        notes=notes,
        recommended_profile=rec,
        isp_evidence=isp_summary,
        time_of_day_status=tod_status,
        next_best_hours=next_hours,
        primary_evidence_mode=primary_mode,
    )
