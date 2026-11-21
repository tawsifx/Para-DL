//! Report generators for the throttling diagnostic suite.
//!
//! [`generate_report_json`] is byte-exact against the `report.json` schema
//! `1.0` golden (parity row L12): field order mirrors Python's
//! `ThrottleReport.to_dict()`, and `serde_json`'s two-space pretty printer
//! matches `json.dumps(..., indent=2)`.
//!
//! The Markdown and HTML generators are functional ports of the Python
//! terminal report writers. No JSONL is ever written to stdout: this module
//! only builds strings and writes files.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Result;
use crate::models::{
    BaselinePathResult, BudgetSummary, BurstBucketResult, DestinationIndependenceResult,
    FlowScalingResult, GroundTruthResult, ISPEvidenceSummary, LocalVsInternationalResult,
    PolicingShapingResult, ProtocolPortResult, RecommendedProfile, SanityResult, SingleFlowSeries,
    ThrottleReport, TimeVolumeUploadResult, ValidatedTarget, Verdict, VerdictNote,
};

#[derive(Serialize)]
struct ModulesDto<'a> {
    m0_sanity: &'a SanityResult,
    m1_baseline: &'a BaselinePathResult,
    m2_single_flow: &'a [SingleFlowSeries],
    m3_flow_scaling: &'a [FlowScalingResult],
    m4_destination_independence: &'a DestinationIndependenceResult,
    m5_burst_bucket: &'a [BurstBucketResult],
    m6_policing_shaping: &'a [PolicingShapingResult],
    m7_protocol_port: &'a [ProtocolPortResult],
    m8_time_volume_upload: &'a TimeVolumeUploadResult,
    m9_ground_truth: &'a GroundTruthResult,
    m10_local_vs_intl: &'a LocalVsInternationalResult,
}

#[derive(Serialize)]
struct ReportDto<'a> {
    schema_version: &'a str,
    run_id: &'a str,
    mode: &'a str,
    isp: &'a str,
    plan_mbps: Option<f64>,
    timestamp: &'a str,
    budget: &'a BudgetSummary,
    environment: &'a SanityResult,
    targets: &'a [ValidatedTarget],
    modules: ModulesDto<'a>,
    verdicts: &'a [Verdict],
    notes: &'a [VerdictNote],
    time_of_day_status: &'a str,
    next_best_hours: &'a [String],
    primary_evidence_mode: &'a str,
    recommended_profile: &'a RecommendedProfile,
    isp_evidence_summary: &'a ISPEvidenceSummary,
}

/// Generate the `report.json` payload for a report (no trailing newline).
///
/// The returned string matches Python's `json.dumps(report.to_dict(), indent=2)`
/// byte-for-byte. Write a trailing newline yourself when producing a file whose
/// golden includes one.
///
/// ```
/// use paradl_throttle::models::ThrottleReport;
/// use paradl_throttle::report::generate_report_json;
/// let json = generate_report_json(&ThrottleReport::default());
/// assert!(json.contains("\"schema_version\": \"1.0\""));
/// assert!(json.contains("\"m10_local_vs_intl\""));
/// ```
pub fn generate_report_json(report: &ThrottleReport) -> String {
    let dto = ReportDto {
        schema_version: &report.schema_version,
        run_id: &report.run_id,
        mode: &report.mode,
        isp: &report.isp,
        plan_mbps: report.plan_mbps,
        timestamp: &report.timestamp,
        budget: &report.budget,
        environment: &report.dataset.m0_sanity,
        targets: &report.targets,
        modules: ModulesDto {
            m0_sanity: &report.dataset.m0_sanity,
            m1_baseline: &report.dataset.m1_baseline,
            m2_single_flow: &report.dataset.m2_single_flow,
            m3_flow_scaling: &report.dataset.m3_flow_scaling,
            m4_destination_independence: &report.dataset.m4_destination_independence,
            m5_burst_bucket: &report.dataset.m5_burst_bucket,
            m6_policing_shaping: &report.dataset.m6_policing_shaping,
            m7_protocol_port: &report.dataset.m7_protocol_port,
            m8_time_volume_upload: &report.dataset.m8_time_volume_upload,
            m9_ground_truth: &report.dataset.m9_ground_truth,
            m10_local_vs_intl: &report.dataset.m10_local_vs_intl,
        },
        verdicts: &report.classification.verdicts,
        notes: &report.classification.notes,
        time_of_day_status: &report.classification.time_of_day_status,
        next_best_hours: &report.classification.next_best_hours,
        primary_evidence_mode: &report.classification.primary_evidence_mode,
        recommended_profile: &report.classification.recommended_profile,
        isp_evidence_summary: &report.classification.isp_evidence,
    };
    serde_json::to_string_pretty(&dto).expect("report DTO is always serializable")
}

fn mbps_str(bps: Option<f64>) -> String {
    match bps {
        Some(bps) if bps > 0.0 => format!("{:.2} Mbps", (bps * 8.0) / 1_000_000.0),
        _ => "n/a".to_owned(),
    }
}

fn mb_str(bytes: i64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn local_vs_intl_reading(report: &ThrottleReport) -> String {
    let m10 = &report.dataset.m10_local_vs_intl;
    let dom = (m10.domestic_median_bps.unwrap_or(0.0)) * 8.0 / 1_000_000.0;
    let intl = (m10.international_median_bps.unwrap_or(0.0)) * 8.0 / 1_000_000.0;
    let cached = (m10.isp_cached_median_bps.unwrap_or(0.0)) * 8.0 / 1_000_000.0;
    if dom > 0.0 && intl > 0.0 && dom >= intl * 2.5 {
        if cached >= intl * 2.5 {
            return format!(
                "Domestic ({dom:.1} Mbps) and ISP-cached ({cached:.1} Mbps) routes are substantially faster than uncached international routes ({intl:.1} Mbps). This indicates international bandwidth shaping or upstream transit bottlenecks while local BDIX/GGC peering runs near line rate."
            );
        }
        return format!(
            "Domestic peers ({dom:.1} Mbps) are {:.1}x faster than international paths ({intl:.1} Mbps), pointing to international transit or upstream capacity constraints rather than a local last-mile limit.",
            dom / intl.max(0.1)
        );
    }
    if cached > 0.0 && intl > 0.0 && cached >= intl * 2.5 {
        return format!(
            "ISP-cached CDN edges ({cached:.1} Mbps) are {:.1}x faster than uncached international hosts ({intl:.1} Mbps). Fast speed tests or video streams reflect local cache hits rather than uncached international capacity.",
            cached / intl.max(0.1)
        );
    }
    if intl > 0.0 {
        return format!(
            "Uncached international throughput averaged {intl:.1} Mbps (domestic: {dom:.1} Mbps, ISP-cached: {cached:.1} Mbps)."
        );
    }
    "Insufficient multi-locality targets to separate domestic from international paths.".to_owned()
}

/// Generate the comprehensive Markdown report (`report.md`).
pub fn generate_markdown_report(report: &ThrottleReport) -> String {
    let plan_str = match report.plan_mbps {
        Some(p) => format!("{p:.1} Mbps"),
        None => "Unspecified".to_owned(),
    };
    let m0 = &report.dataset.m0_sanity;
    let m10 = &report.dataset.m10_local_vs_intl;
    let prof = &report.classification.recommended_profile;
    let isp_ev = &report.classification.isp_evidence;
    let mut lines: Vec<String> = vec![
        format!("# ISP Throttling Diagnostic Report — {}", report.isp),
        String::new(),
        format!("- **ISP**: {}", report.isp),
        format!("- **Run ID**: `{}`", report.run_id),
        format!("- **Mode**: `{}`", report.mode),
        format!("- **Timestamp**: `{}`", report.timestamp),
        format!("- **Subscribed Plan**: {plan_str}"),
        format!(
            "- **Data Budget**: {} used of {} (planned worst-case: {})",
            mb_str(report.budget.actual_used_bytes),
            mb_str(report.budget.limit_bytes),
            mb_str(report.budget.planned_worst_case_bytes)
        ),
        format!(
            "- **Primary Evidence Strategy**: `{}`",
            report.classification.primary_evidence_mode
        ),
        String::new(),
        "## 1. Local vs International Summary".to_owned(),
        String::new(),
        "| Locality Tier | 1-Connection Median | 16-Connection Aggregate |".to_owned(),
        "| --- | ---: | ---: |".to_owned(),
        format!(
            "| **Domestic (In-Country / IXP)** | {} | {} |",
            mbps_str(m10.domestic_median_bps),
            mbps_str(m10.domestic_16_bps)
        ),
        format!(
            "| **ISP-Cached CDN Edge** | {} | {} |",
            mbps_str(m10.isp_cached_median_bps),
            mbps_str(m10.isp_cached_16_bps)
        ),
        format!(
            "| **Uncached International** | {} | {} |",
            mbps_str(m10.international_median_bps),
            mbps_str(m10.international_16_bps)
        ),
        String::new(),
        format!(
            "> **Plain-Language Reading**: {}",
            local_vs_intl_reading(report)
        ),
        String::new(),
    ];
    if let Some(asn) = &m10.shared_slow_upstream_asn {
        lines.push(format!(
            "- **Shared Upstream Transit ASN on Slow International Paths**: `{asn}`"
        ));
        lines.push(String::new());
    }
    lines.extend([
        "## 2. Diagnostic Verdicts".to_owned(),
        String::new(),
        "| Verdict Class | Confidence | Summary | Supporting Evidence |".to_owned(),
        "| --- | ---: | --- | --- |".to_owned(),
    ]);
    for v in &report.classification.verdicts {
        lines.push(format!(
            "| **{}** | {:.0}% | {} | {} |",
            v.verdict_class,
            v.confidence * 100.0,
            v.summary,
            v.evidence.join("; ")
        ));
    }
    lines.push(String::new());
    if !report.classification.notes.is_empty() {
        lines.push("### Diagnostic Notes".to_owned());
        lines.push(String::new());
        for note in &report.classification.notes {
            lines.push(format!("#### {} (`{}`)", note.title, note.code));
            lines.push(note.text.clone());
            for rec in &note.recommendations {
                lines.push(format!("- {rec}"));
            }
            lines.push(String::new());
        }
    }
    lines.extend([
        "## 3. What This Means for Downloads".to_owned(),
        String::new(),
        "### Domestic Sources (Local Mirrors, BDIX, In-Country Peers)".to_owned(),
        format!(
            "- Use **{} connections**. Domestic routes typically bypass international transit shapers.",
            prof.domestic_connections
        ),
        String::new(),
        "### Uncached International Sources (GitHub Releases, HuggingFace, PyPI, Overseas Servers)".to_owned(),
        format!(
            "- Use **{} parallel connections** with stream recycling every **{:.0}s**.",
            prof.international_connections, prof.recycle_interval_sec
        ),
        "- When single-host caps apply, split across mirror URLs (`--mirrors`) so multiple destinations run concurrently.".to_owned(),
        String::new(),
        "### YouTube & Cache-Fronted Sources (GGC / Local Edge Caches)".to_owned(),
        "- High speeds on YouTube or ISP-cached CDNs reflect local edge caching (`cache effect`), not uncached international capacity.".to_owned(),
        "- Do not use cached CDN speeds as the baseline expected rate for uncached overseas file transfers.".to_owned(),
        String::new(),
        "### Recommended Download Profile (`source: throttle-probe`)".to_owned(),
        String::new(),
        format!("- **Default Connections**: `{}`", prof.default_connections),
        format!("- **Domestic Connections**: `{}`", prof.domestic_connections),
        format!("- **International Connections**: `{}`", prof.international_connections),
        format!("- **Recycle Interval**: `{:.1}s`", prof.recycle_interval_sec),
        format!("- **Use Multi-Host Mirrors**: `{}`", prof.use_mirrors),
        format!("- **Try HTTP/3**: `{}`", prof.try_http3),
        format!("- **Try Proxy/Tunnel**: `{}`", prof.try_tunnel),
        format!("- **Preferred IP Family**: `{}`", prof.ip_family),
        format!("- **Watchdog Recovery Action**: `{}`", prof.recovery_action),
        String::new(),
    ]);
    for exp in &prof.explanation {
        lines.push(format!("- {exp}"));
    }
    lines.push(String::new());
    lines.extend([
        "## 4. Module Results (M0-M10)".to_owned(),
        String::new(),
        "### M0: Sanity & Local Environment".to_owned(),
        format!(
            "- **Active Interface**: `{}` ({}, SSID: `{}`)",
            m0.active_interface,
            m0.interface_type,
            if m0.ssid.is_empty() { "n/a" } else { &m0.ssid }
        ),
        format!(
            "- **Loopback Simulator Throughput**: {} (`ok={}`)",
            mbps_str(Some(m0.loopback_bps)),
            m0.loopback_ok
        ),
        format!(
            "- **VPN/Tunnel Active**: `{}` ({})",
            m0.vpn_or_tunnel_active,
            if m0.tunnel_interfaces.is_empty() {
                "none".to_owned()
            } else {
                m0.tunnel_interfaces.join(", ")
            }
        ),
        format!(
            "- **CPU 1m Load**: `{:.2}` | **Competing Traffic**: `{}`",
            m0.cpu_load_1m,
            mb_str(m0.competing_bytes_total)
        ),
        format!("- **Environment Contaminated**: `{}`", m0.contaminated),
    ]);
    for warning in &m0.warnings {
        lines.push(format!("  - Warning: {warning}"));
    }
    lines.push(String::new());
    if !report.dataset.m3_flow_scaling.is_empty() {
        lines.push("### M3: Connection Flow Scaling".to_owned());
        lines.push(String::new());
        lines.push(
            "| Target | Locality | 1 Stream | 4 Streams | 16 Streams | 16x/1x Scaling |".to_owned(),
        );
        lines.push("| --- | --- | ---: | ---: | ---: | ---: |".to_owned());
        for m3 in &report.dataset.m3_flow_scaling {
            let get = |n: i64| -> Option<f64> {
                m3.points
                    .iter()
                    .find(|p| p.connections == n)
                    .map(|p| p.aggregate_bps)
            };
            lines.push(format!(
                "| `{}` | {} | {} | {} | {} | {:.2}x |",
                m3.host,
                m3.locality,
                mbps_str(get(1)),
                mbps_str(get(4)),
                mbps_str(get(16)),
                m3.scaling_ratio_16_to_1
            ));
        }
        lines.push(String::new());
    }
    if !report.dataset.m5_burst_bucket.is_empty() {
        lines.push("### M5: Burst & Token Bucket Analysis".to_owned());
        lines.push(String::new());
        lines.push("| Target | Initial Burst | Sustained Rate | Burst Duration | Est. Bucket | Drop Shape |".to_owned());
        lines.push("| --- | ---: | ---: | ---: | ---: | --- |".to_owned());
        for m5 in &report.dataset.m5_burst_bucket {
            lines.push(format!(
                "| `{}` | {} | {} | {:.1}s | {} | `{}` |",
                m5.host,
                mbps_str(Some(m5.initial_burst_bps)),
                mbps_str(Some(m5.sustained_bps)),
                m5.burst_duration_sec,
                mb_str(m5.estimated_bucket_bytes),
                m5.drop_shape
            ));
        }
        lines.push(String::new());
    }
    if !report.dataset.m6_policing_shaping.is_empty() {
        lines.push("### M6: Policing vs Shaping vs Congestion".to_owned());
        lines.push(String::new());
        lines.push(
            "| Target | Idle RTT | Loaded RTT | RTT Inflation | Retransmit Rate | Mechanism Hint |"
                .to_owned(),
        );
        lines.push("| --- | ---: | ---: | ---: | ---: | --- |".to_owned());
        for m6 in &report.dataset.m6_policing_shaping {
            lines.push(format!(
                "| `{}` | {:.1} ms | {:.1} ms | +{:.1} ms ({:.2}x) | {:.2}% | `{}` |",
                m6.host,
                m6.idle_rtt_ms,
                m6.loaded_rtt_ms,
                m6.rtt_inflation_ms,
                m6.rtt_inflation_ratio,
                m6.retransmit_rate_pct,
                m6.mechanism_hint
            ));
        }
        lines.push(String::new());
    }
    lines.extend([
        "### M8: Time-of-Day Coverage & Sparse Sampling Status".to_owned(),
        format!(
            "- **Time-of-Day Verdict Status**: `{}`",
            report.classification.time_of_day_status
        ),
        format!(
            "- **Recorded Time-of-Day Samples**: `{}`",
            report.dataset.m8_time_volume_upload.samples.len()
        ),
        format!(
            "- **Next Most Informative Hours to Sample**: {}",
            if report.classification.next_best_hours.is_empty() {
                "None needed".to_owned()
            } else {
                report.classification.next_best_hours.join(", ")
            }
        ),
        String::new(),
        "## 5. Neutral ISP Evidence Summary (For Support Escalation)".to_owned(),
        String::new(),
        format!("**ISP**: {}", isp_ev.isp),
        String::new(),
        "**Measurement Dates/Times**:".to_owned(),
    ]);
    for d in &isp_ev.dates {
        lines.push(format!("- `{d}`"));
    }
    lines.extend([String::new(), "**Targets Tested**:".to_owned()]);
    for t in &isp_ev.targets_tested {
        lines.push(format!("- {t}"));
    }
    lines.extend([String::new(), "**Methodology**:".to_owned()]);
    for m in &isp_ev.methodology {
        lines.push(format!("- {m}"));
    }
    lines.extend([String::new(), "**Measured Findings**:".to_owned()]);
    for num in &isp_ev.measured_numbers {
        lines.push(format!("- {num}"));
    }
    lines.extend([
        String::new(),
        "**Neutral Technical Questions for ISP Engineering**:".to_owned(),
    ]);
    for q in &isp_ev.neutral_questions {
        lines.push(format!("1. {q}"));
    }
    lines.extend([
        String::new(),
        "## 6. Reference Guides".to_owned(),
        String::new(),
        "### How to Run a Clean Test".to_owned(),
        "1. Connect via **Ethernet** or stand close to your 5 GHz Wi-Fi router with no VPN or proxy active.".to_owned(),
        "2. Pause active downloads, video streams, cloud backups, and software updates before starting.".to_owned(),
        "3. Run `pdl throttle quick` for a 3-minute paired baseline, or `pdl throttle sample` at different hours of the day.".to_owned(),
        "4. Run `pdl throttle coverage` to see which time-of-day buckets still need samples.".to_owned(),
        String::new(),
        "### How to Read the Verdict".to_owned(),
        "- **`per-flow cap`**: Individual TCP streams are rate-limited, while multiple parallel connections (`-c 16`) scale up aggregate throughput.".to_owned(),
        "- **`burst-then-throttle`**: Downloads start fast while a token bucket drains, then step down to a lower sustained rate.".to_owned(),
        "- **`loss-based policing` vs `queue-based shaping`**: Policers drop excess packets immediately (sawtooth speed + TCP retransmits + low RTT growth), whereas shapers buffer packets in a queue (smooth speed cap + large RTT inflation under load).".to_owned(),
        "- **`international bandwidth limit` / `peering/transit congestion`**: Domestic BDIX/peering and local CDN caches are fast, while uncached international routes are constrained on shared upstream transit ASNs.".to_owned(),
        "- **`cache effect` note**: Fast speeds to ISP-cached CDN edges (such as GGC/Cloudflare local nodes) do not carry over to uncached overseas file hosts.".to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Generate a self-contained HTML report (`report.html`).
pub fn generate_html_report(report: &ThrottleReport) -> String {
    let verdict_rows = report
        .classification
        .verdicts
        .iter()
        .map(|v| {
            format!(
                "<tr><td>{}</td><td>{:.0}%</td><td>{}</td></tr>",
                escape_html(&v.verdict_class),
                v.confidence * 100.0,
                escape_html(&v.summary)
            )
        })
        .collect::<String>();
    let note_rows = report
        .classification
        .notes
        .iter()
        .map(|n| {
            format!(
                "<section><h3>{}</h3><p>{}</p></section>",
                escape_html(&n.title),
                escape_html(&n.text)
            )
        })
        .collect::<String>();
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>ISP Throttling Diagnostic Report — {}</title></head>\n<body>\n<h1>ISP Throttling Diagnostic Report</h1>\n<p>ISP: {} | Mode: {} | Run ID: {}</p>\n<h2>Classifier Verdicts</h2>\n<table><thead><tr><th>Class</th><th>Confidence</th><th>Summary</th></tr></thead><tbody>{verdict_rows}</tbody></table>\n<h2>Notes</h2>\n{note_rows}\n<h2>Next Best Hours</h2>\n<ul>{}</ul>\n</body>\n</html>\n",
        escape_html(&report.isp),
        escape_html(&report.isp),
        escape_html(&report.mode),
        escape_html(&report.run_id),
        report
            .classification
            .next_best_hours
            .iter()
            .map(|h| format!("<li>{}</li>", escape_html(h)))
            .collect::<String>()
    )
}

/// Write `report.json`, `report.md`, and optionally `report.html` into `out_dir`.
///
/// The JSON file is the byte-exact payload from [`generate_report_json`], with
/// a trailing newline to match the oracle fixture's file representation.
pub fn write_report_files(
    report: &ThrottleReport,
    out_dir: &Path,
    include_html: bool,
) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(out_dir)?;
    let mut paths = Vec::new();

    let json_path = out_dir.join("report.json");
    fs::write(&json_path, format!("{}\n", generate_report_json(report)))?;
    paths.push(json_path);

    let md_path = out_dir.join("report.md");
    fs::write(&md_path, generate_markdown_report(report))?;
    paths.push(md_path);

    if include_html {
        let html_path = out_dir.join("report.html");
        fs::write(&html_path, generate_html_report(report))?;
        paths.push(html_path);
    }
    Ok(paths)
}
