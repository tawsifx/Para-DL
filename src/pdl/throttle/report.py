"""Terminal, Markdown, JSON, and self-contained HTML/SVG report generators."""

from __future__ import annotations

import html
import json
from pathlib import Path
from typing import Any

from rich.console import Console
from rich.panel import Panel
from rich.table import Table

from pdl.throttle.models import (
    BaselinePathResult,
    BudgetSummary,
    BurstBucketResult,
    ClassificationResult,
    CompetingProcess,
    DestinationIndependenceResult,
    FlowScalePoint,
    FlowScalingResult,
    GroundTruthResult,
    ISPEvidenceSummary,
    LocalVsInternationalResult,
    MeasurementDataset,
    PathBaselineTarget,
    PolicingShapingResult,
    ProtocolPortResult,
    RecommendedProfile,
    SampleRecord,
    SanityResult,
    SingleFlowSeries,
    TargetPathTrace,
    ThrottleReport,
    TimeVolumeUploadResult,
    TracerouteHop,
    ValidatedTarget,
    Verdict,
    VerdictNote,
)


def _mbps_str(bps: float | None) -> str:
    if bps is None or bps <= 0:
        return "n/a"
    return f"{(bps * 8.0) / 1_000_000.0:.2f} Mbps"


def _mb_str(num_bytes: int) -> str:
    return f"{num_bytes / (1024 * 1024):.1f} MB"


def _local_vs_intl_reading(report: ThrottleReport) -> str:
    m10 = report.dataset.m10_local_vs_intl
    dom = (m10.domestic_median_bps or 0.0) * 8.0 / 1_000_000.0
    intl = (m10.international_median_bps or 0.0) * 8.0 / 1_000_000.0
    cached = (m10.isp_cached_median_bps or 0.0) * 8.0 / 1_000_000.0

    if dom > 0 and intl > 0 and dom >= intl * 2.5:
        if cached >= intl * 2.5:
            return (
                f"Domestic ({dom:.1f} Mbps) and ISP-cached ({cached:.1f} Mbps) routes are "
                f"substantially faster than uncached international routes ({intl:.1f} Mbps). "
                "This indicates international bandwidth shaping or upstream transit bottlenecks "
                "while local BDIX/GGC peering runs near line rate."
            )
        return (
            f"Domestic peers ({dom:.1f} Mbps) are {dom / max(intl, 0.1):.1f}x faster than "
            f"international paths ({intl:.1f} Mbps), pointing to international transit or "
            "upstream capacity constraints rather than a local last-mile limit."
        )
    if cached > 0 and intl > 0 and cached >= intl * 2.5:
        return (
            f"ISP-cached CDN edges ({cached:.1f} Mbps) are {cached / max(intl, 0.1):.1f}x faster "
            f"than uncached international hosts ({intl:.1f} Mbps). Fast speed tests or video "
            "streams reflect local cache hits rather than uncached international capacity."
        )
    if intl > 0:
        return (
            f"Uncached international throughput averaged {intl:.1f} Mbps "
            f"(domestic: {dom:.1f} Mbps, ISP-cached: {cached:.1f} Mbps)."
        )
    return "Insufficient multi-locality targets to separate domestic from international paths."


def render_terminal_report(report: ThrottleReport, console: Console | None = None) -> None:
    """Render a Rich summary of the diagnostic report to the terminal."""
    con = console or Console()
    plan_str = f"{report.plan_mbps:.0f} Mbps" if report.plan_mbps else "Unspecified"
    header = (
        f"[bold cyan]ISP Throttling Diagnostic Report[/bold cyan]\n"
        f"ISP: [bold]{report.isp}[/bold] | Mode: [bold]{report.mode}[/bold] | "
        f"Plan: {plan_str} | Run ID: {report.run_id}\n"
        f"Data Budget: {_mb_str(report.budget.actual_used_bytes)} used / "
        f"{_mb_str(report.budget.limit_bytes)} limit "
        f"(planned worst-case: {_mb_str(report.budget.planned_worst_case_bytes)})"
    )
    con.print(Panel(header, expand=False))

    # Top Section: Local vs International
    m10 = report.dataset.m10_local_vs_intl
    loc_table = Table(title="Local vs International Throughput (Paired Comparison)")
    loc_table.add_column("Locality Group", style="bold")
    loc_table.add_column("1-Stream Median", justify="right")
    loc_table.add_column("16-Stream Aggregate", justify="right")
    loc_table.add_row(
        "Domestic (In-Country / IXP)",
        _mbps_str(m10.domestic_median_bps),
        _mbps_str(m10.domestic_16_bps),
    )
    loc_table.add_row(
        "ISP-Cached CDN Edge",
        _mbps_str(m10.isp_cached_median_bps),
        _mbps_str(m10.isp_cached_16_bps),
    )
    loc_table.add_row(
        "Uncached International",
        _mbps_str(m10.international_median_bps),
        _mbps_str(m10.international_16_bps),
    )
    con.print(loc_table)
    con.print(f"[italic]{_local_vs_intl_reading(report)}[/italic]\n")

    # Verdicts Table
    v_table = Table(title="Classifier Verdicts")
    v_table.add_column("Verdict Class", style="bold yellow")
    v_table.add_column("Confidence", justify="right")
    v_table.add_column("Summary & Evidence")
    for v in report.classification.verdicts:
        ev_str = "; ".join(v.evidence)
        v_table.add_row(
            v.verdict_class,
            f"{v.confidence * 100:.0f}%",
            f"{v.summary}\n[dim]{ev_str}[/dim]",
        )
    con.print(v_table)

    # Notes
    for note in report.classification.notes:
        con.print(
            Panel(
                f"{note.text}\n" + "\n".join(f"- {r}" for r in note.recommendations),
                title=f"Note: {note.title}",
                border_style="blue",
            )
        )

    # Recommended Profile
    prof = report.classification.recommended_profile
    prof_text = (
        f"Default Connections: [bold]{prof.default_connections}[/bold] "
        f"(Domestic: {prof.domestic_connections}, International: {prof.international_connections})\n"
        f"Stream Recycle Interval: {prof.recycle_interval_sec:.0f}s | "
        f"Multi-Host Mirrors: {prof.use_mirrors} | Recovery: {prof.recovery_action}\n"
        + "\n".join(f"• {line}" for line in prof.explanation)
    )
    con.print(Panel(prof_text, title="Recommended Download Profile (Saved to PDL)", border_style="green"))


def generate_markdown_report(report: ThrottleReport) -> str:
    """Generate the comprehensive Markdown diagnostic report (`report.md`)."""
    plan_str = f"{report.plan_mbps:.1f} Mbps" if report.plan_mbps else "Unspecified"
    m0 = report.dataset.m0_sanity
    m10 = report.dataset.m10_local_vs_intl
    prof = report.classification.recommended_profile
    isp_ev = report.classification.isp_evidence

    lines: list[str] = [
        f"# ISP Throttling Diagnostic Report — {report.isp}",
        "",
        f"- **ISP**: {report.isp}",
        f"- **Run ID**: `{report.run_id}`",
        f"- **Mode**: `{report.mode}`",
        f"- **Timestamp**: `{report.timestamp}`",
        f"- **Subscribed Plan**: {plan_str}",
        f"- **Data Budget**: {_mb_str(report.budget.actual_used_bytes)} used of {_mb_str(report.budget.limit_bytes)} "
        f"(planned worst-case: {_mb_str(report.budget.planned_worst_case_bytes)})",
        f"- **Primary Evidence Strategy**: `{report.classification.primary_evidence_mode}`",
        "",
        "## 1. Local vs International Summary",
        "",
        "| Locality Tier | 1-Connection Median | 16-Connection Aggregate |",
        "| --- | ---: | ---: |",
        f"| **Domestic (In-Country / IXP)** | {_mbps_str(m10.domestic_median_bps)} | {_mbps_str(m10.domestic_16_bps)} |",
        f"| **ISP-Cached CDN Edge** | {_mbps_str(m10.isp_cached_median_bps)} | {_mbps_str(m10.isp_cached_16_bps)} |",
        f"| **Uncached International** | {_mbps_str(m10.international_median_bps)} | {_mbps_str(m10.international_16_bps)} |",
        "",
        f"> **Plain-Language Reading**: {_local_vs_intl_reading(report)}",
        "",
    ]

    if m10.shared_slow_upstream_asn:
        lines.append(f"- **Shared Upstream Transit ASN on Slow International Paths**: `{m10.shared_slow_upstream_asn}`")
        lines.append("")

    lines.extend(
        [
            "## 2. Diagnostic Verdicts",
            "",
            "| Verdict Class | Confidence | Summary | Supporting Evidence |",
            "| --- | ---: | --- | --- |",
        ]
    )
    for v in report.classification.verdicts:
        ev_joined = "; ".join(v.evidence)
        lines.append(f"| **{v.verdict_class}** | {v.confidence * 100:.0f}% | {v.summary} | {ev_joined} |")
    lines.append("")

    if report.classification.notes:
        lines.append("### Diagnostic Notes")
        lines.append("")
        for note in report.classification.notes:
            lines.append(f"#### {note.title} (`{note.code}`)")
            lines.append(note.text)
            for rec in note.recommendations:
                lines.append(f"- {rec}")
            lines.append("")

    lines.extend(
        [
            "## 3. What This Means for Downloads",
            "",
            "### Domestic Sources (Local Mirrors, BDIX, In-Country Peers)",
            f"- Use **{prof.domestic_connections} connections**. Domestic routes typically bypass international transit shapers.",
            "",
            "### Uncached International Sources (GitHub Releases, HuggingFace, PyPI, Overseas Servers)",
            f"- Use **{prof.international_connections} parallel connections** with stream recycling every **{prof.recycle_interval_sec:.0f}s**.",
            "- When single-host caps apply, split across mirror URLs (`--mirrors`) so multiple destinations run concurrently.",
            "",
            "### YouTube & Cache-Fronted Sources (GGC / Local Edge Caches)",
            "- High speeds on YouTube or ISP-cached CDNs reflect local edge caching (`cache effect`), not uncached international capacity.",
            "- Do not use cached CDN speeds as the baseline expected rate for uncached overseas file transfers.",
            "",
            "### Recommended Download Profile (`source: throttle-probe`)",
            "",
            f"- **Default Connections**: `{prof.default_connections}`",
            f"- **Domestic Connections**: `{prof.domestic_connections}`",
            f"- **International Connections**: `{prof.international_connections}`",
            f"- **Recycle Interval**: `{prof.recycle_interval_sec:.1f}s`",
            f"- **Use Multi-Host Mirrors**: `{prof.use_mirrors}`",
            f"- **Try HTTP/3**: `{prof.try_http3}`",
            f"- **Try Proxy/Tunnel**: `{prof.try_tunnel}`",
            f"- **Preferred IP Family**: `{prof.ip_family}`",
            f"- **Watchdog Recovery Action**: `{prof.recovery_action}`",
            "",
        ]
    )
    for exp in prof.explanation:
        lines.append(f"- {exp}")
    lines.append("")

    # Module details
    lines.extend(
        [
            "## 4. Module Results (M0–M10)",
            "",
            "### M0: Sanity & Local Environment",
            f"- **Active Interface**: `{m0.active_interface}` ({m0.interface_type}, SSID: `{m0.ssid or 'n/a'}`)",
            f"- **Loopback Simulator Throughput**: {_mbps_str(m0.loopback_bps)} (`ok={m0.loopback_ok}`)",
            f"- **VPN/Tunnel Active**: `{m0.vpn_or_tunnel_active}` ({', '.join(m0.tunnel_interfaces) or 'none'})",
            f"- **CPU 1m Load**: `{m0.cpu_load_1m:.2f}` | **Competing Traffic**: `{_mb_str(m0.competing_bytes_total)}`",
            f"- **Environment Contaminated**: `{m0.contaminated}`",
        ]
    )
    for w in m0.warnings:
        lines.append(f"  - Warning: {w}")
    lines.append("")

    if report.dataset.m3_flow_scaling:
        lines.append("### M3: Connection Flow Scaling")
        lines.append("")
        lines.append("| Target | Locality | 1 Stream | 4 Streams | 16 Streams | 16x/1x Scaling |")
        lines.append("| --- | --- | ---: | ---: | ---: | ---: |")
        for m3 in report.dataset.m3_flow_scaling:
            pts = {p.connections: p.aggregate_bps for p in m3.points}
            lines.append(
                f"| `{m3.host}` | {m3.locality} | {_mbps_str(pts.get(1))} | "
                f"{_mbps_str(pts.get(4))} | {_mbps_str(pts.get(16))} | {m3.scaling_ratio_16_to_1:.2f}x |"
            )
        lines.append("")

    if report.dataset.m5_burst_bucket:
        lines.append("### M5: Burst & Token Bucket Analysis")
        lines.append("")
        lines.append("| Target | Initial Burst | Sustained Rate | Burst Duration | Est. Bucket | Drop Shape |")
        lines.append("| --- | ---: | ---: | ---: | ---: | --- |")
        for m5 in report.dataset.m5_burst_bucket:
            lines.append(
                f"| `{m5.host}` | {_mbps_str(m5.initial_burst_bps)} | {_mbps_str(m5.sustained_bps)} | "
                f"{m5.burst_duration_sec:.1f}s | {_mb_str(m5.estimated_bucket_bytes)} | `{m5.drop_shape}` |"
            )
        lines.append("")

    if report.dataset.m6_policing_shaping:
        lines.append("### M6: Policing vs Shaping vs Congestion")
        lines.append("")
        lines.append("| Target | Idle RTT | Loaded RTT | RTT Inflation | Retransmit Rate | Mechanism Hint |")
        lines.append("| --- | ---: | ---: | ---: | ---: | --- |")
        for m6 in report.dataset.m6_policing_shaping:
            lines.append(
                f"| `{m6.host}` | {m6.idle_rtt_ms:.1f} ms | {m6.loaded_rtt_ms:.1f} ms | "
                f"+{m6.rtt_inflation_ms:.1f} ms ({m6.rtt_inflation_ratio:.2f}x) | "
                f"{m6.retransmit_rate_pct:.2f}% | `{m6.mechanism_hint}` |"
            )
        lines.append("")

    lines.extend(
        [
            "### M8: Time-of-Day Coverage & Sparse Sampling Status",
            f"- **Time-of-Day Verdict Status**: `{report.classification.time_of_day_status}`",
            f"- **Recorded Time-of-Day Samples**: `{len(report.dataset.m8_time_volume_upload.samples)}`",
            f"- **Next Most Informative Hours to Sample**: {', '.join(report.classification.next_best_hours) or 'None needed'}",
            "",
            "## 5. Neutral ISP Evidence Summary (For Support Escalation)",
            "",
            f"**ISP**: {isp_ev.isp}",
            "",
            "**Measurement Dates/Times**:",
        ]
    )
    for d in isp_ev.dates:
        lines.append(f"- `{d}`")
    lines.append("")
    lines.append("**Targets Tested**:")
    for t in isp_ev.targets_tested:
        lines.append(f"- {t}")
    lines.append("")
    lines.append("**Methodology**:")
    for m in isp_ev.methodology:
        lines.append(f"- {m}")
    lines.append("")
    lines.append("**Measured Findings**:")
    for num in isp_ev.measured_numbers:
        lines.append(f"- {num}")
    lines.append("")
    lines.append("**Neutral Technical Questions for ISP Engineering**:")
    for q in isp_ev.neutral_questions:
        lines.append(f"1. {q}")
    lines.append("")

    lines.extend(
        [
            "## 6. Reference Guides",
            "",
            "### How to Run a Clean Test",
            "1. Connect via **Ethernet** or stand close to your 5 GHz Wi-Fi router with no VPN or proxy active.",
            "2. Pause active downloads, video streams, cloud backups, and software updates before starting.",
            '3. Run `pdl throttle quick --isp "Duronto Internet"` for a 3-minute paired baseline, or `pdl throttle sample` at different hours of the day.',
            "4. Run `pdl throttle coverage` to see which time-of-day buckets still need samples.",
            "",
            "### How to Read the Verdict",
            "- **`per-flow cap`**: Individual TCP streams are rate-limited, while multiple parallel connections (`-c 16`) scale up aggregate throughput.",
            "- **`burst-then-throttle`**: Downloads start fast while a token bucket drains, then step down to a lower sustained rate.",
            "- **`loss-based policing` vs `queue-based shaping`**: Policers drop excess packets immediately (sawtooth speed + TCP retransmits + low RTT growth), whereas shapers buffer packets in a queue (smooth speed cap + large RTT inflation under load).",
            "- **`international bandwidth limit` / `peering/transit congestion`**: Domestic BDIX/peering and local CDN caches are fast, while uncached international routes are constrained on shared upstream transit ASNs.",
            "- **`cache effect` note**: Fast speeds to ISP-cached CDN edges (such as GGC/Cloudflare local nodes) do not carry over to uncached overseas file hosts.",
            "",
        ]
    )
    return "\n".join(lines)


def _svg_local_vs_intl(report: ThrottleReport) -> str:
    m10 = report.dataset.m10_local_vs_intl
    bars = [
        ("Domestic (1x)", (m10.domestic_median_bps or 0.0) * 8.0 / 1_000_000.0, "#10b981"),
        ("Domestic (16x)", (m10.domestic_16_bps or 0.0) * 8.0 / 1_000_000.0, "#059669"),
        ("ISP-Cached (1x)", (m10.isp_cached_median_bps or 0.0) * 8.0 / 1_000_000.0, "#3b82f6"),
        ("International (1x)", (m10.international_median_bps or 0.0) * 8.0 / 1_000_000.0, "#f59e0b"),
        ("International (16x)", (m10.international_16_bps or 0.0) * 8.0 / 1_000_000.0, "#d97706"),
    ]
    max_val = max([b[1] for b in bars] + [report.plan_mbps or 10.0, 1.0])
    width, height = 680, 220
    parts = [
        f'<svg viewBox="0 0 {width} {height}" width="100%" height="{height}" '
        'role="img" aria-label="Local vs International Throughput">',
        '<rect width="100%" height="100%" fill="#0f172a" rx="8"/>',
        '<text x="20" y="28" fill="#f8fafc" font-size="14" font-weight="bold">'
        "Local vs International Throughput (Mbps)</text>",
    ]
    y = 50
    for label, val, color in bars:
        bar_w = int((val / max_val) * 400) if max_val > 0 else 0
        bar_w = max(bar_w, 2 if val > 0 else 0)
        parts.append(f'<text x="20" y="{y + 15}" fill="#cbd5e1" font-size="12">{html.escape(label)}</text>')
        parts.append(f'<rect x="180" y="{y}" width="{bar_w}" height="20" fill="{color}" rx="4"/>')
        val_label = f"{val:.2f} Mbps" if val > 0 else "no data"
        parts.append(f'<text x="{190 + bar_w}" y="{y + 15}" fill="#e2e8f0" font-size="12">{val_label}</text>')
        y += 32
    parts.append("</svg>")
    return "\n".join(parts)


def _svg_m2_timeseries(report: ThrottleReport) -> str:
    width, height = 680, 220
    parts = [
        f'<svg viewBox="0 0 {width} {height}" width="100%" height="{height}" '
        'role="img" aria-label="Single-Flow Time Series">',
        '<rect width="100%" height="100%" fill="#0f172a" rx="8"/>',
        '<text x="20" y="28" fill="#f8fafc" font-size="14" font-weight="bold">'
        "M2: Single-Flow Throughput Time Series (250 ms samples)</text>",
    ]
    series_list = report.dataset.m2_single_flow
    if not series_list or not any(s.samples_bps for s in series_list):
        parts.append(
            '<text x="20" y="110" fill="#94a3b8" font-size="13">No single-flow time series data recorded.</text>'
        )
        parts.append("</svg>")
        return "\n".join(parts)

    colors = ["#38bdf8", "#f43f5e", "#a855f7", "#10b981"]
    all_vals = [(v * 8.0 / 1_000_000.0) for s in series_list for v in s.samples_bps]
    max_mbps = max(all_vals + [1.0])
    plot_x0, plot_y0, plot_w, plot_h = 60, 45, 590, 140
    parts.append(
        f'<line x1="{plot_x0}" y1="{plot_y0 + plot_h}" x2="{plot_x0 + plot_w}" '
        f'y2="{plot_y0 + plot_h}" stroke="#334155" stroke-width="1"/>'
    )
    parts.append(f'<text x="10" y="{plot_y0 + 10}" fill="#94a3b8" font-size="10">{max_mbps:.1f}M</text>')
    parts.append(f'<text x="10" y="{plot_y0 + plot_h}" fill="#94a3b8" font-size="10">0.0M</text>')

    for idx, series in enumerate(series_list[:4]):
        if not series.samples_bps:
            continue
        color = colors[idx % len(colors)]
        n = len(series.samples_bps)
        coords: list[str] = []
        for i, bps in enumerate(series.samples_bps):
            mbps = (bps * 8.0) / 1_000_000.0
            px = plot_x0 + (i / max(n - 1, 1)) * plot_w
            py = plot_y0 + plot_h - (mbps / max_mbps) * plot_h
            coords.append(f"{px:.1f},{py:.1f}")
        parts.append(f'<polyline fill="none" stroke="{color}" stroke-width="2" points="{" ".join(coords)}"/>')
        parts.append(
            f'<text x="{plot_x0 + idx * 150}" y="{height - 10}" fill="{color}" font-size="11">'
            f"{html.escape(series.host)} ({series.locality})</text>"
        )

    parts.append("</svg>")
    return "\n".join(parts)


def _svg_m3_flow_scaling(report: ThrottleReport) -> str:
    width, height = 680, 220
    parts = [
        f'<svg viewBox="0 0 {width} {height}" width="100%" height="{height}" '
        'role="img" aria-label="Flow-Scaling Curve">',
        '<rect width="100%" height="100%" fill="#0f172a" rx="8"/>',
        '<text x="20" y="28" fill="#f8fafc" font-size="14" font-weight="bold">'
        "M3: Multi-Connection Flow Scaling Curve (1..16 Streams)</text>",
    ]
    scaling_list = report.dataset.m3_flow_scaling
    if not scaling_list or not any(s.points for s in scaling_list):
        parts.append('<text x="20" y="110" fill="#94a3b8" font-size="13">No flow-scaling curve data recorded.</text>')
        parts.append("</svg>")
        return "\n".join(parts)

    colors = ["#22c55e", "#f97316", "#38bdf8"]
    all_mbps = [(p.aggregate_bps * 8.0 / 1_000_000.0) for s in scaling_list for p in s.points]
    max_mbps = max(all_mbps + [1.0])
    plot_x0, plot_y0, plot_w, plot_h = 60, 45, 590, 135

    for idx, sc in enumerate(scaling_list[:3]):
        if not sc.points:
            continue
        color = colors[idx % len(colors)]
        coords: list[str] = []
        max_conn = max(p.connections for p in sc.points) or 16
        for pt in sc.points:
            mbps = (pt.aggregate_bps * 8.0) / 1_000_000.0
            px = plot_x0 + ((pt.connections - 1) / max(max_conn - 1, 1)) * plot_w
            py = plot_y0 + plot_h - (mbps / max_mbps) * plot_h
            coords.append(f"{px:.1f},{py:.1f}")
            parts.append(f'<circle cx="{px:.1f}" cy="{py:.1f}" r="3.5" fill="{color}"/>')
        parts.append(f'<polyline fill="none" stroke="{color}" stroke-width="2" points="{" ".join(coords)}"/>')
        parts.append(
            f'<text x="{plot_x0 + idx * 200}" y="{height - 10}" fill="{color}" font-size="11">'
            f"{html.escape(sc.host)} ({sc.scaling_ratio_16_to_1:.1f}x)</text>"
        )

    parts.append("</svg>")
    return "\n".join(parts)


def _svg_m8_coverage_heatmap(report: ThrottleReport) -> str:
    width, height = 680, 175
    parts = [
        f'<svg viewBox="0 0 {width} {height}" width="100%" height="{height}" '
        'role="img" aria-label="Time-of-Day Coverage Matrix">',
        '<rect width="100%" height="100%" fill="#0f172a" rx="8"/>',
        '<text x="20" y="26" fill="#f8fafc" font-size="14" font-weight="bold">'
        f"M8: Time-of-Day Coverage Matrix (Status: {html.escape(report.classification.time_of_day_status)})</text>",
    ]
    samples = report.dataset.m8_time_volume_upload.samples
    cell_map: dict[tuple[int, bool], list[float]] = {}
    for s in samples:
        if s.download_bps > 0:
            cell_map.setdefault((int(s.hour) % 24, bool(s.is_weekend)), []).append((s.download_bps * 8.0) / 1_000_000.0)

    cell_w = 23
    x0 = 90
    for row_idx, (label, is_wknd) in enumerate([("Weekday", False), ("Weekend", True)]):
        ry = 52 + row_idx * 42
        parts.append(f'<text x="20" y="{ry + 18}" fill="#cbd5e1" font-size="12">{label}</text>')
        for hour in range(24):
            cx = x0 + hour * (cell_w + 1)
            vals = cell_map.get((hour, is_wknd), [])
            if not vals:
                fill = "#1e293b"
                title = f"{label} {hour:02d}:00 — no data"
            else:
                avg_v = sum(vals) / len(vals)
                fill = "#0ea5e9" if avg_v >= 10.0 else "#f59e0b"
                title = f"{label} {hour:02d}:00 — {avg_v:.1f} Mbps ({len(vals)} samples)"
            parts.append(
                f'<rect x="{cx}" y="{ry}" width="{cell_w}" height="26" fill="{fill}" rx="3">'
                f"<title>{html.escape(title)}</title></rect>"
            )
            if row_idx == 1 and hour % 3 == 0:
                parts.append(f'<text x="{cx + 2}" y="{ry + 40}" fill="#64748b" font-size="9">{hour:02d}h</text>')

    parts.append(
        '<text x="20" y="162" fill="#94a3b8" font-size="11">'
        'Dark cells indicate "no data" (unmeasured hours). Blue/amber cells contain recorded probes.</text>'
    )
    parts.append("</svg>")
    return "\n".join(parts)


def generate_html_report(report: ThrottleReport) -> str:
    """Generate a self-contained HTML5 report (`report.html`) with inline SVG charts and zero external assets."""
    plan_str = f"{report.plan_mbps:.1f} Mbps" if report.plan_mbps else "Unspecified"
    prof = report.classification.recommended_profile
    isp_ev = report.classification.isp_evidence

    verdict_rows = []
    for v in report.classification.verdicts:
        ev_items = "".join(f"<li>{html.escape(e)}</li>" for e in v.evidence)
        verdict_rows.append(
            f"<tr><td><strong>{html.escape(v.verdict_class)}</strong></td>"
            f"<td>{v.confidence * 100:.0f}%</td>"
            f"<td>{html.escape(v.summary)}</td>"
            f"<td><ul>{ev_items}</ul></td></tr>"
        )

    note_blocks = []
    for n in report.classification.notes:
        recs = "".join(f"<li>{html.escape(r)}</li>" for r in n.recommendations)
        note_blocks.append(
            f'<div class="note-box"><h4>{html.escape(n.title)} (<code>{html.escape(n.code)}</code>)</h4>'
            f"<p>{html.escape(n.text)}</p><ul>{recs}</ul></div>"
        )

    questions_html = "".join(f"<li>{html.escape(q)}</li>" for q in isp_ev.neutral_questions)
    numbers_html = "".join(f"<li>{html.escape(num)}</li>" for num in isp_ev.measured_numbers)

    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>ISP Throttling Report — {html.escape(report.isp)}</title>
<style>
  :root {{
    color-scheme: dark;
    --bg: #020617;
    --card: #0f172a;
    --border: #1e293b;
    --text: #f8fafc;
    --muted: #94a3b8;
    --accent: #38bdf8;
  }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: var(--bg);
    color: var(--text);
    margin: 0;
    padding: 24px;
    line-height: 1.5;
  }}
  .container {{ max-width: 980px; margin: 0 auto; }}
  header {{
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 20px;
    margin-bottom: 20px;
  }}
  h1, h2, h3 {{ margin-top: 0; }}
  .meta {{ color: var(--muted); font-size: 0.92rem; }}
  .card {{
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 20px;
    margin-bottom: 20px;
  }}
  table {{
    width: 100%;
    border-collapse: collapse;
    margin-top: 12px;
  }}
  th, td {{
    border-bottom: 1px solid var(--border);
    padding: 10px 12px;
    text-align: left;
    vertical-align: top;
  }}
  th {{ color: var(--accent); }}
  .note-box {{
    border-left: 4px solid var(--accent);
    background: #1e293b;
    padding: 12px 16px;
    border-radius: 6px;
    margin-top: 12px;
  }}
  .chart-wrap {{ margin: 16px 0; }}
  code {{ background: #1e293b; padding: 2px 6px; border-radius: 4px; }}
</style>
</head>
<body>
<div class="container">
  <header>
    <h1>ISP Throttling Diagnostic Report — {html.escape(report.isp)}</h1>
    <div class="meta">
      ISP: <strong>{html.escape(report.isp)}</strong> |
      Mode: <code>{html.escape(report.mode)}</code> |
      Run ID: <code>{html.escape(report.run_id)}</code> |
      Subscribed Plan: {html.escape(plan_str)} |
      Budget Used: {_mb_str(report.budget.actual_used_bytes)} / {_mb_str(report.budget.limit_bytes)}
    </div>
  </header>

  <section class="card">
    <h2>1. Local vs International Throughput</h2>
    <p>{html.escape(_local_vs_intl_reading(report))}</p>
    <div class="chart-wrap">
      {_svg_local_vs_intl(report)}
    </div>
  </section>

  <section class="card">
    <h2>2. Classifier Verdicts &amp; Notes</h2>
    <table>
      <thead>
        <tr><th>Verdict Class</th><th>Confidence</th><th>Summary</th><th>Evidence</th></tr>
      </thead>
      <tbody>
        {"".join(verdict_rows)}
      </tbody>
    </table>
    {"".join(note_blocks)}
  </section>

  <section class="card">
    <h2>3. Throughput Time Series, Flow Scaling &amp; Time Coverage</h2>
    <div class="chart-wrap">{_svg_m2_timeseries(report)}</div>
    <div class="chart-wrap">{_svg_m3_flow_scaling(report)}</div>
    <div class="chart-wrap">{_svg_m8_coverage_heatmap(report)}</div>
  </section>

  <section class="card">
    <h2>4. What This Means for Downloads &amp; Recommended Profile</h2>
    <ul>
      <li><strong>Domestic Connections</strong>: <code>{prof.domestic_connections}</code></li>
      <li><strong>International Connections</strong>: <code>{prof.international_connections}</code></li>
      <li><strong>Stream Recycle Interval</strong>: <code>{prof.recycle_interval_sec:.0f}s</code></li>
      <li><strong>Multi-Host Mirrors</strong>: <code>{prof.use_mirrors}</code></li>
      <li><strong>Watchdog Recovery Action</strong>: <code>{html.escape(prof.recovery_action)}</code></li>
    </ul>
  </section>

  <section class="card">
    <h2>5. Neutral ISP Evidence Summary ({html.escape(isp_ev.isp)})</h2>
    <h3>Measured Findings</h3>
    <ul>{numbers_html}</ul>
    <h3>Neutral Questions for ISP Support</h3>
    <ol>{questions_html}</ol>
  </section>
</div>
</body>
</html>
"""


def write_report_files(
    report: ThrottleReport,
    out_dir: Path,
    *,
    include_html: bool = True,
) -> dict[str, Path]:
    """Write `report.json`, `report.md`, and optionally `report.html` to `out_dir`."""
    out_dir.mkdir(parents=True, exist_ok=True)
    paths: dict[str, Path] = {}

    json_path = out_dir / "report.json"
    json_path.write_text(json.dumps(report.to_dict(), indent=2), encoding="utf-8")
    paths["json"] = json_path

    md_path = out_dir / "report.md"
    md_path.write_text(generate_markdown_report(report), encoding="utf-8")
    paths["md"] = md_path

    if include_html:
        html_path = out_dir / "report.html"
        html_path.write_text(generate_html_report(report), encoding="utf-8")
        paths["html"] = html_path

    return paths


def report_from_dict(data: dict[str, Any]) -> ThrottleReport:
    """Rehydrate a `ThrottleReport` instance from a stored `report.json` dictionary."""
    mods = data.get("modules", {})
    m0_raw = mods.get("m0_sanity", data.get("environment", {}))
    comp_procs = [CompetingProcess(**cp) for cp in m0_raw.get("competing_processes", [])]
    m0 = SanityResult(
        loopback_bps=float(m0_raw.get("loopback_bps", 0.0)),
        loopback_ok=bool(m0_raw.get("loopback_ok", True)),
        active_interface=str(m0_raw.get("active_interface", "en0")),
        interface_type=str(m0_raw.get("interface_type", "Ethernet")),
        ssid=str(m0_raw.get("ssid", "")),
        wifi_link_mbps=m0_raw.get("wifi_link_mbps"),
        vpn_or_tunnel_active=bool(m0_raw.get("vpn_or_tunnel_active", False)),
        tunnel_interfaces=list(m0_raw.get("tunnel_interfaces", [])),
        cpu_load_1m=float(m0_raw.get("cpu_load_1m", 0.0)),
        competing_bytes_total=int(m0_raw.get("competing_bytes_total", 0)),
        competing_processes=comp_procs,
        on_battery=bool(m0_raw.get("on_battery", False)),
        contaminated=bool(m0_raw.get("contaminated", False)),
        warnings=list(m0_raw.get("warnings", [])),
    )

    m1_raw = mods.get("m1_baseline", {})
    m1 = BaselinePathResult(
        resolver_dns_ms=dict(m1_raw.get("resolver_dns_ms", {})),
        targets=[PathBaselineTarget(**t) for t in m1_raw.get("targets", [])],
    )
    m2 = [SingleFlowSeries(**s) for s in mods.get("m2_single_flow", [])]
    m3 = [
        FlowScalingResult(
            target_id=s["target_id"],
            host=s["host"],
            locality=s["locality"],
            supports_range=bool(s.get("supports_range", True)),
            points=[FlowScalePoint(**p) for p in s.get("points", [])],
            scaling_ratio_16_to_1=float(s.get("scaling_ratio_16_to_1", 1.0)),
            scaling_ratio_4_to_1=float(s.get("scaling_ratio_4_to_1", 1.0)),
        )
        for s in mods.get("m3_flow_scaling", [])
    ]
    m4 = DestinationIndependenceResult(**mods.get("m4_destination_independence", {}))
    m5 = [BurstBucketResult(**b) for b in mods.get("m5_burst_bucket", [])]
    m6 = [PolicingShapingResult(**p) for p in mods.get("m6_policing_shaping", [])]
    m7 = [ProtocolPortResult(**p) for p in mods.get("m7_protocol_port", [])]

    m8_raw = mods.get("m8_time_volume_upload", {})
    m8 = TimeVolumeUploadResult(
        session_start_probe_bps=float(m8_raw.get("session_start_probe_bps", 0.0)),
        session_end_probe_bps=float(m8_raw.get("session_end_probe_bps", 0.0)),
        session_drift_ratio=float(m8_raw.get("session_drift_ratio", 1.0)),
        volume_tested_bytes=int(m8_raw.get("volume_tested_bytes", 0)),
        volume_pre_step_bps=float(m8_raw.get("volume_pre_step_bps", 0.0)),
        volume_post_step_bps=float(m8_raw.get("volume_post_step_bps", 0.0)),
        volume_step_down_at_bytes=m8_raw.get("volume_step_down_at_bytes"),
        download_bps=float(m8_raw.get("download_bps", 0.0)),
        upload_bps=float(m8_raw.get("upload_bps", 0.0)),
        asymmetry_ratio=float(m8_raw.get("asymmetry_ratio", 1.0)),
        samples=[SampleRecord(**sr) for sr in m8_raw.get("samples", [])],
    )
    m9 = GroundTruthResult(**mods.get("m9_ground_truth", {}))

    m10_raw = mods.get("m10_local_vs_intl", {})
    traces = [
        TargetPathTrace(
            target_id=tr["target_id"],
            host=tr["host"],
            dest_ip=tr["dest_ip"],
            dest_asn=tr["dest_asn"],
            dest_country=tr["dest_country"],
            locality=tr["locality"],
            throughput_1_bps=float(tr.get("throughput_1_bps", 0.0)),
            throughput_16_bps=float(tr.get("throughput_16_bps", 0.0)),
            rtt_ms=float(tr.get("rtt_ms", 0.0)),
            hops=[TracerouteHop(**h) for h in tr.get("hops", [])],
            upstream_asns=list(tr.get("upstream_asns", [])),
        )
        for tr in m10_raw.get("traces", [])
    ]
    m10 = LocalVsInternationalResult(
        domestic_median_bps=m10_raw.get("domestic_median_bps"),
        international_median_bps=m10_raw.get("international_median_bps"),
        isp_cached_median_bps=m10_raw.get("isp_cached_median_bps"),
        domestic_16_bps=m10_raw.get("domestic_16_bps"),
        international_16_bps=m10_raw.get("international_16_bps"),
        isp_cached_16_bps=m10_raw.get("isp_cached_16_bps"),
        shared_slow_upstream_asn=m10_raw.get("shared_slow_upstream_asn"),
        fast_international_asns=list(m10_raw.get("fast_international_asns", [])),
        slow_international_asns=list(m10_raw.get("slow_international_asns", [])),
        traces=traces,
    )

    dataset = MeasurementDataset(
        isp=str(data.get("isp", "Duronto Internet")),
        mode=str(data.get("mode", "quick")),
        plan_mbps=data.get("plan_mbps"),
        m0_sanity=m0,
        m1_baseline=m1,
        m2_single_flow=m2,
        m3_flow_scaling=m3,
        m4_destination_independence=m4,
        m5_burst_bucket=m5,
        m6_policing_shaping=m6,
        m7_protocol_port=m7,
        m8_time_volume_upload=m8,
        m9_ground_truth=m9,
        m10_local_vs_intl=m10,
    )

    classification = ClassificationResult(
        verdicts=[Verdict(**v) for v in data.get("verdicts", [])],
        notes=[VerdictNote(**n) for n in data.get("notes", [])],
        recommended_profile=RecommendedProfile(**data.get("recommended_profile", {})),
        isp_evidence=ISPEvidenceSummary(**data.get("isp_evidence_summary", {"isp": dataset.isp})),
        time_of_day_status=str(data.get("time_of_day_status", "insufficient time data")),
        next_best_hours=list(data.get("next_best_hours", [])),
        primary_evidence_mode=str(data.get("primary_evidence_mode", "paired_same_moment")),
    )

    budget_raw = data.get("budget", {})
    budget = BudgetSummary(
        limit_bytes=int(budget_raw.get("limit_bytes", 300 * 1024 * 1024)),
        planned_worst_case_bytes=int(budget_raw.get("planned_worst_case_bytes", 0)),
        actual_used_bytes=int(budget_raw.get("actual_used_bytes", 0)),
        exceeded=bool(budget_raw.get("exceeded", False)),
    )

    return ThrottleReport(
        schema_version=str(data.get("schema_version", "1.0")),
        run_id=str(data.get("run_id", "rebuilt")),
        mode=str(data.get("mode", "quick")),
        isp=str(data.get("isp", "Duronto Internet")),
        plan_mbps=data.get("plan_mbps"),
        timestamp=str(data.get("timestamp", "")),
        budget=budget,
        targets=[ValidatedTarget(**t) for t in data.get("targets", [])],
        dataset=dataset,
        classification=classification,
    )


def rebuild_report_from_dict(
    data: dict[str, Any],
    out_dir: Path,
    *,
    include_html: bool = True,
) -> dict[str, Path]:
    """Rebuild `report.md`, `report.json`, and `report.html` from a stored run dictionary."""
    report = report_from_dict(data)
    return write_report_files(report, out_dir, include_html=include_html)
