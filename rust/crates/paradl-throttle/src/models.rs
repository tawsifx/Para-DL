//! Domain models for the ISP throttling diagnostic suite.
//!
//! Field names and declaration order mirror `src/paradl/throttle/models.py`
//! exactly so that `serde_json` reproduces Python's `dataclasses.asdict()` key
//! order in `report.json` (parity row L12).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The report schema version, byte-identical to `REPORT_SCHEMA_VERSION`.
pub const REPORT_SCHEMA_VERSION: &str = "1.0";

/// Canonical verdict classes emitted by the rule-based classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerdictClass {
    /// No throttling signature detected.
    NoThrottling,
    /// Uniform line-capacity ceiling.
    LineCapacityLimit,
    /// Per-TCP-flow rate cap.
    PerFlowCap,
    /// Burst followed by a sustained throttle.
    BurstThenThrottle,
    /// Per-destination (shared host) cap.
    PerDestinationLimit,
    /// Hard packet-drop / reset policing.
    LossBasedPolicing,
    /// Queue-based (bufferbloat) shaping.
    QueueBasedShaping,
    /// Protocol or port differentiation.
    ProtocolPortDiff,
    /// TLS SNI or service-based differentiation.
    SniServiceDiff,
    /// Time-of-day congestion.
    TimeOfDayCongestion,
    /// Volume-based fair-usage throttling.
    VolumeFupThrottling,
    /// Peering / transit congestion on selected ASNs.
    PeeringTransitCongestion,
    /// International bandwidth limit.
    InternationalBwLimit,
    /// HTTP 200-only server (no byte ranges).
    NoRangeSupport,
    /// No single signature passed the confidence threshold.
    Inconclusive,
}

impl VerdictClass {
    /// Return the exact string value used by the Python `StrEnum`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoThrottling => "no-throttling",
            Self::LineCapacityLimit => "line-capacity limit",
            Self::PerFlowCap => "per-flow cap",
            Self::BurstThenThrottle => "burst-then-throttle",
            Self::PerDestinationLimit => "per-destination limit",
            Self::LossBasedPolicing => "loss-based policing",
            Self::QueueBasedShaping => "queue-based shaping",
            Self::ProtocolPortDiff => "protocol/port differentiation",
            Self::SniServiceDiff => "SNI or service-based differentiation",
            Self::TimeOfDayCongestion => "time-of-day congestion",
            Self::VolumeFupThrottling => "volume-based (FUP) throttling",
            Self::PeeringTransitCongestion => "peering/transit congestion",
            Self::InternationalBwLimit => "international bandwidth limit",
            Self::NoRangeSupport => "200-only (no Range)",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// A candidate endpoint loaded from `targets.toml` or CLI flags (M0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetCandidate {
    /// Stable identifier.
    pub id: String,
    /// Absolute URL to probe.
    pub url: String,
    /// Group name (`global-cdn`, `domestic`, `isp-cached`, …).
    pub group: String,
    /// Human-readable label.
    pub label: String,
    /// Alternate port for port-differentiation probes.
    pub alt_port: Option<i64>,
    /// Plain-HTTP companion URL.
    pub http_url: Option<String>,
    /// Whether the endpoint accepts uploads.
    pub supports_upload: bool,
    /// Whether best-effort SNI probing is allowed.
    pub sni_probe_allowed: bool,
    /// Alternate SNI hostname.
    pub sni_alt_host: Option<String>,
}

/// A target that passed runtime reachability and TLS validation (M0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ValidatedTarget {
    /// Stable identifier.
    pub id: String,
    /// Absolute URL.
    pub url: String,
    /// Lower-cased hostname.
    pub host: String,
    /// Resolved IP address.
    pub ip: String,
    /// Source group.
    pub group: String,
    /// Human-readable label.
    pub label: String,
    /// HTTP status code observed during validation.
    pub status_code: i64,
    /// Whether byte-range requests are supported.
    pub supports_range: bool,
    /// Content length from `Content-Range` / `Content-Length`.
    pub content_length: Option<i64>,
    /// Whether the URL used HTTPS.
    pub tls_ok: bool,
    /// Country from ASN resolution.
    pub country: String,
    /// ASN string (e.g. `AS13335`).
    pub asn: String,
    /// ASN organisation.
    pub asn_org: String,
    /// `domestic`, `international`, `isp-cached`, or `loopback`.
    pub locality: String,
    /// Alternate port, when applicable.
    pub alt_port: Option<i64>,
    /// Plain-HTTP companion URL.
    pub http_url: Option<String>,
    /// Whether uploads are supported.
    pub supports_upload: bool,
    /// Whether SNI probing is allowed.
    pub sni_probe_allowed: bool,
    /// Alternate SNI hostname.
    pub sni_alt_host: Option<String>,
}

/// A competing network process observed in a `nettop` snapshot (M0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompetingProcess {
    /// Process name.
    pub process: String,
    /// Network interface.
    pub interface: String,
    /// Connection state.
    pub state: String,
    /// Bytes received.
    pub bytes_in: i64,
    /// Bytes transmitted.
    pub bytes_out: i64,
}

/// M0 sanity and local-environment factors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SanityResult {
    /// Measured loopback throughput in bytes/s.
    pub loopback_bps: f64,
    /// Whether the loopback probe succeeded.
    pub loopback_ok: bool,
    /// Active network interface.
    pub active_interface: String,
    /// `Wi-Fi`, `Ethernet`, `VPN/Tunnel`, or `Loopback`.
    pub interface_type: String,
    /// Wi-Fi SSID (redacted by the caller when needed).
    pub ssid: String,
    /// Negotiated Wi-Fi link rate in Mbps.
    pub wifi_link_mbps: Option<f64>,
    /// Whether a VPN/tunnel interface is present.
    pub vpn_or_tunnel_active: bool,
    /// Tunnel interface names.
    pub tunnel_interfaces: Vec<String>,
    /// 1-minute CPU load average.
    pub cpu_load_1m: f64,
    /// Total bytes attributed to competing processes.
    pub competing_bytes_total: i64,
    /// Competing process snapshots.
    pub competing_processes: Vec<CompetingProcess>,
    /// Whether the machine is on battery power.
    pub on_battery: bool,
    /// Whether local conditions contaminated the measurements.
    pub contaminated: bool,
    /// Human-readable contamination warnings.
    pub warnings: Vec<String>,
}

impl Default for SanityResult {
    fn default() -> Self {
        Self {
            loopback_bps: 0.0,
            loopback_ok: true,
            active_interface: "en0".to_owned(),
            interface_type: "Ethernet".to_owned(),
            ssid: String::new(),
            wifi_link_mbps: None,
            vpn_or_tunnel_active: false,
            tunnel_interfaces: Vec::new(),
            cpu_load_1m: 0.0,
            competing_bytes_total: 0,
            competing_processes: Vec::new(),
            on_battery: false,
            contaminated: false,
            warnings: Vec::new(),
        }
    }
}

/// M1 per-target baseline path quality.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathBaselineTarget {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Resolved IP.
    pub ip: String,
    /// DNS resolution time in ms.
    pub dns_ms: f64,
    /// TCP connect time in ms.
    pub tcp_connect_ms: f64,
    /// Minimum RTT in ms.
    pub rtt_min_ms: f64,
    /// Average RTT in ms.
    pub rtt_avg_ms: f64,
    /// Maximum RTT in ms.
    pub rtt_max_ms: f64,
    /// Jitter in ms.
    pub jitter_ms: f64,
    /// Packet-loss percentage.
    pub loss_pct: f64,
    /// Path MTU in bytes.
    pub path_mtu: i64,
}

/// M1 aggregate baseline path quality including multi-resolver DNS.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BaselinePathResult {
    /// Resolver name to DNS latency in ms.
    pub resolver_dns_ms: BTreeMap<String, f64>,
    /// Per-target baselines.
    pub targets: Vec<PathBaselineTarget>,
}

/// M2 single-connection throughput time series.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SingleFlowSeries {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Locality class.
    pub locality: String,
    /// Probe duration in seconds.
    pub duration_sec: f64,
    /// Sampling interval in seconds.
    pub sample_interval_sec: f64,
    /// Total bytes transferred.
    pub total_bytes: i64,
    /// Average rate in bytes/s.
    pub avg_bps: f64,
    /// Peak sampled rate in bytes/s.
    pub peak_bps: f64,
    /// Sustained (second-half mean) rate in bytes/s.
    pub sustained_bps: f64,
    /// Whether the target supports byte ranges.
    pub supports_range: bool,
    /// Per-sample rates in bytes/s.
    pub samples_bps: Vec<f64>,
}

/// One point on the M3 flow-scaling curve.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowScalePoint {
    /// Concurrent connection count.
    pub connections: i64,
    /// Aggregate throughput in bytes/s.
    pub aggregate_bps: f64,
    /// Per-flow throughput in bytes/s.
    pub per_flow_bps: f64,
    /// Step duration in seconds.
    pub duration_sec: f64,
    /// Bytes transferred during the step.
    pub bytes_transferred: i64,
}

/// M3 flow-scaling curve for one target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowScalingResult {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Locality class.
    pub locality: String,
    /// Whether byte ranges were supported.
    pub supports_range: bool,
    /// Ordered curve points.
    pub points: Vec<FlowScalePoint>,
    /// 16-stream to 1-stream scaling ratio.
    pub scaling_ratio_16_to_1: f64,
    /// 4-stream to 1-stream scaling ratio.
    pub scaling_ratio_4_to_1: f64,
}

/// M4 concurrent two-host download comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DestinationIndependenceResult {
    /// First host.
    pub host_a: String,
    /// Second host.
    pub host_b: String,
    /// Solo throughput to host A.
    pub solo_a_bps: f64,
    /// Solo throughput to host B.
    pub solo_b_bps: f64,
    /// Concurrent throughput to host A.
    pub concurrent_a_bps: f64,
    /// Concurrent throughput to host B.
    pub concurrent_b_bps: f64,
    /// Concurrent aggregate throughput.
    pub concurrent_total_bps: f64,
    /// Aggregate-to-solo ratio; defaults to 1.0.
    pub independence_ratio: f64,
}

impl Default for DestinationIndependenceResult {
    fn default() -> Self {
        Self {
            host_a: String::new(),
            host_b: String::new(),
            solo_a_bps: 0.0,
            solo_b_bps: 0.0,
            concurrent_a_bps: 0.0,
            concurrent_b_bps: 0.0,
            concurrent_total_bps: 0.0,
            independence_ratio: 1.0,
        }
    }
}

/// M5 burst rate, token-bucket estimate, drop shape, and idle refill.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BurstBucketResult {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Initial burst rate in bytes/s.
    pub initial_burst_bps: f64,
    /// Burst duration in seconds.
    pub burst_duration_sec: f64,
    /// Sustained rate in bytes/s.
    pub sustained_bps: f64,
    /// Burst-to-sustained ratio.
    pub burst_to_sustained_ratio: f64,
    /// Estimated token-bucket size in bytes.
    pub estimated_bucket_bytes: i64,
    /// `step`, `decay`, `periodic`, or `flat`.
    pub drop_shape: String,
    /// Burst rate after a 30-second idle period.
    pub refill_30s_burst_bps: Option<f64>,
    /// Burst rate after a 120-second idle period.
    pub refill_120s_burst_bps: Option<f64>,
    /// 30-second refill ratio.
    pub refill_30s_ratio: Option<f64>,
    /// 120-second refill ratio.
    pub refill_120s_ratio: Option<f64>,
}

/// M6 RTT-under-load and TCP retransmission/loss deltas.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PolicingShapingResult {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Idle RTT in ms.
    pub idle_rtt_ms: f64,
    /// Loaded RTT in ms.
    pub loaded_rtt_ms: f64,
    /// RTT inflation in ms.
    pub rtt_inflation_ms: f64,
    /// RTT inflation ratio.
    pub rtt_inflation_ratio: f64,
    /// TCP packets sent delta.
    pub tcp_packets_sent_delta: i64,
    /// TCP retransmits delta.
    pub tcp_retransmits_delta: i64,
    /// Retransmission rate percentage.
    pub retransmit_rate_pct: f64,
    /// Connection resets observed.
    pub connection_resets: i64,
    /// Throughput coefficient of variation.
    pub throughput_cov: f64,
    /// `policing`, `shaping`, `congestion`, or `none`.
    pub mechanism_hint: String,
}

/// M7 protocol, port, IP-family, and best-effort SNI differentiation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtocolPortResult {
    /// Hostname.
    pub host: String,
    /// HTTP/1.1 throughput.
    pub http1_1_bps: Option<f64>,
    /// HTTP/2 throughput.
    pub http2_bps: Option<f64>,
    /// HTTP/3 throughput.
    pub http3_bps: Option<f64>,
    /// HTTP/3 probe status.
    pub http3_status: String,
    /// HTTPS throughput.
    pub https_bps: Option<f64>,
    /// Plain-HTTP throughput.
    pub http_plain_bps: Option<f64>,
    /// Port-443 throughput.
    pub port_443_bps: Option<f64>,
    /// Alternate port number.
    pub alt_port: Option<i64>,
    /// Alternate-port throughput.
    pub alt_port_bps: Option<f64>,
    /// IPv4 throughput.
    pub ipv4_bps: Option<f64>,
    /// IPv6 throughput.
    pub ipv6_bps: Option<f64>,
    /// Default-SNI throughput.
    pub sni_default_bps: Option<f64>,
    /// Alternate-SNI throughput.
    pub sni_alt_bps: Option<f64>,
    /// Label for the alternate SNI probe.
    pub sni_label: String,
}

impl Default for ProtocolPortResult {
    fn default() -> Self {
        Self {
            host: String::new(),
            http1_1_bps: None,
            http2_bps: None,
            http3_bps: None,
            http3_status: "skipped".to_owned(),
            https_bps: None,
            http_plain_bps: None,
            port_443_bps: None,
            alt_port: None,
            alt_port_bps: None,
            ipv4_bps: None,
            ipv6_bps: None,
            sni_default_bps: None,
            sni_alt_bps: None,
            sni_label: "best effort".to_owned(),
        }
    }
}

/// M8 stored active or passive time-of-day throughput sample.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleRecord {
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Unix epoch seconds.
    pub epoch: f64,
    /// Hour of day (may exceed 23 before normalisation).
    pub hour: i32,
    /// Whether the sample was taken on a weekend.
    pub is_weekend: bool,
    /// Network interface.
    pub interface: String,
    /// Wi-Fi SSID.
    pub ssid: String,
    /// Stable network identifier.
    pub network_id: String,
    /// `active` (weight 1.0) or another source (weight 0.35).
    pub source: String,
    /// Target group.
    pub target_group: String,
    /// Hostname.
    pub host: String,
    /// Download throughput in bytes/s.
    pub download_bps: f64,
    /// Upload throughput in bytes/s.
    pub upload_bps: f64,
    /// Round-trip time in ms.
    pub rtt_ms: f64,
    /// Bytes consumed by the sample.
    pub bytes_used: i64,
    /// Raw per-second rate series.
    pub series_bps: Vec<f64>,
}

impl SampleRecord {
    /// Effective statistical weight derived from the source label.
    pub fn weight(&self) -> f64 {
        if self.source == "active" { 1.0 } else { 0.35 }
    }
}

/// M8 time-of-day, within-session drift, cumulative volume, and upload results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeVolumeUploadResult {
    /// Session-start probe throughput.
    pub session_start_probe_bps: f64,
    /// Session-end probe throughput.
    pub session_end_probe_bps: f64,
    /// End-to-start drift ratio.
    pub session_drift_ratio: f64,
    /// Bytes transferred for the volume test.
    pub volume_tested_bytes: i64,
    /// Throughput before the volume step-down.
    pub volume_pre_step_bps: f64,
    /// Throughput after the volume step-down.
    pub volume_post_step_bps: f64,
    /// Byte count at which the step-down occurred.
    pub volume_step_down_at_bytes: Option<i64>,
    /// Aggregate download throughput.
    pub download_bps: f64,
    /// Aggregate upload throughput.
    pub upload_bps: f64,
    /// Download-to-upload asymmetry ratio.
    pub asymmetry_ratio: f64,
    /// Time-of-day samples.
    pub samples: Vec<SampleRecord>,
}

impl Default for TimeVolumeUploadResult {
    fn default() -> Self {
        Self {
            session_start_probe_bps: 0.0,
            session_end_probe_bps: 0.0,
            session_drift_ratio: 1.0,
            volume_tested_bytes: 0,
            volume_pre_step_bps: 0.0,
            volume_post_step_bps: 0.0,
            volume_step_down_at_bytes: None,
            download_bps: 0.0,
            upload_bps: 0.0,
            asymmetry_ratio: 1.0,
            samples: Vec::new(),
        }
    }
}

/// M9 paired `--via-proxy` and own-server port/payload comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundTruthResult {
    /// Proxy URL with credentials removed.
    pub proxy_url_redacted: Option<String>,
    /// Direct 1-stream throughput.
    pub direct_1_bps: Option<f64>,
    /// Proxy 1-stream throughput.
    pub proxy_1_bps: Option<f64>,
    /// Direct 16-stream throughput.
    pub direct_16_bps: Option<f64>,
    /// Proxy 16-stream throughput.
    pub proxy_16_bps: Option<f64>,
    /// Measured proxy speed-up ratio.
    pub proxy_speedup_ratio: Option<f64>,
    /// Own-server random-payload throughput.
    pub own_server_random_bps: Option<f64>,
    /// Own-server video-payload throughput.
    pub own_server_video_bps: Option<f64>,
    /// Per-port throughput sweep.
    pub own_server_port_sweep: BTreeMap<String, f64>,
    /// UDP echo loss percentage.
    pub udp_echo_loss_pct: Option<f64>,
}

/// One hop from a `traceroute -n -q 1 -w 1` run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TracerouteHop {
    /// Hop index.
    pub hop: i64,
    /// Hop IP or `*`.
    pub ip: String,
    /// Round-trip time in ms.
    pub rtt_ms: Option<f64>,
    /// ASN string.
    pub asn: String,
    /// Country code.
    pub country: String,
}

/// M10 traceroute path and ASN sequence for one target.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetPathTrace {
    /// Target identifier.
    pub target_id: String,
    /// Hostname.
    pub host: String,
    /// Destination IP.
    pub dest_ip: String,
    /// Destination ASN.
    pub dest_asn: String,
    /// Destination country.
    pub dest_country: String,
    /// Locality class.
    pub locality: String,
    /// 1-stream throughput.
    pub throughput_1_bps: f64,
    /// 16-stream throughput.
    pub throughput_16_bps: f64,
    /// Round-trip time.
    pub rtt_ms: f64,
    /// Traceroute hops.
    pub hops: Vec<TracerouteHop>,
    /// Upstream ASN sequence.
    pub upstream_asns: Vec<String>,
}

/// M10 domestic vs international vs ISP-cached paired comparison.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalVsInternationalResult {
    /// Domestic median 1-stream throughput.
    pub domestic_median_bps: Option<f64>,
    /// International median 1-stream throughput.
    pub international_median_bps: Option<f64>,
    /// ISP-cached median 1-stream throughput.
    pub isp_cached_median_bps: Option<f64>,
    /// Domestic 16-stream throughput.
    pub domestic_16_bps: Option<f64>,
    /// International 16-stream throughput.
    pub international_16_bps: Option<f64>,
    /// ISP-cached 16-stream throughput.
    pub isp_cached_16_bps: Option<f64>,
    /// ASN shared by slow international paths.
    pub shared_slow_upstream_asn: Option<String>,
    /// Fast international ASNs.
    pub fast_international_asns: Vec<String>,
    /// Slow international ASNs.
    pub slow_international_asns: Vec<String>,
    /// Per-target path traces.
    pub traces: Vec<TargetPathTrace>,
}

/// Complete input to the pure rule-based classifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeasurementDataset {
    /// ISP label.
    pub isp: String,
    /// Run mode (`quick` or `full`).
    pub mode: String,
    /// Subscribed plan in Mbps.
    pub plan_mbps: Option<f64>,
    /// M0 sanity result.
    pub m0_sanity: SanityResult,
    /// M1 baseline result.
    pub m1_baseline: BaselinePathResult,
    /// M2 single-flow series.
    pub m2_single_flow: Vec<SingleFlowSeries>,
    /// M3 flow-scaling results.
    pub m3_flow_scaling: Vec<FlowScalingResult>,
    /// M4 destination-independence result.
    pub m4_destination_independence: DestinationIndependenceResult,
    /// M5 burst-bucket results.
    pub m5_burst_bucket: Vec<BurstBucketResult>,
    /// M6 policing/shaping results.
    pub m6_policing_shaping: Vec<PolicingShapingResult>,
    /// M7 protocol/port results.
    pub m7_protocol_port: Vec<ProtocolPortResult>,
    /// M8 time/volume/upload result.
    pub m8_time_volume_upload: TimeVolumeUploadResult,
    /// M9 ground-truth result.
    pub m9_ground_truth: GroundTruthResult,
    /// M10 local vs international result.
    pub m10_local_vs_intl: LocalVsInternationalResult,
}

impl Default for MeasurementDataset {
    fn default() -> Self {
        Self {
            isp: "Unknown ISP".to_owned(),
            mode: "quick".to_owned(),
            plan_mbps: None,
            m0_sanity: SanityResult::default(),
            m1_baseline: BaselinePathResult::default(),
            m2_single_flow: Vec::new(),
            m3_flow_scaling: Vec::new(),
            m4_destination_independence: DestinationIndependenceResult::default(),
            m5_burst_bucket: Vec::new(),
            m6_policing_shaping: Vec::new(),
            m7_protocol_port: Vec::new(),
            m8_time_volume_upload: TimeVolumeUploadResult::default(),
            m9_ground_truth: GroundTruthResult::default(),
            m10_local_vs_intl: LocalVsInternationalResult::default(),
        }
    }
}

/// A single classifier verdict with a confidence score and supporting evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verdict {
    /// Exact verdict-class string.
    pub verdict_class: String,
    /// Confidence in the range 0.0–1.0.
    pub confidence: f64,
    /// Plain-language summary.
    pub summary: String,
    /// Supporting evidence lines.
    pub evidence: Vec<String>,
}

/// An auxiliary diagnostic note.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VerdictNote {
    /// Machine-readable note code.
    pub code: String,
    /// Human-readable title.
    pub title: String,
    /// Note body.
    pub text: String,
    /// Recommended follow-up actions.
    pub recommendations: Vec<String>,
}

/// Actionable download-tuning profile derived from active verdicts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecommendedProfile {
    /// Profile source label.
    pub source: String,
    /// Domestic connection count.
    pub domestic_connections: i64,
    /// International connection count.
    pub international_connections: i64,
    /// Default connection count.
    pub default_connections: i64,
    /// Socket recycle interval in seconds.
    pub recycle_interval_sec: f64,
    /// Whether to use multi-host mirrors.
    pub use_mirrors: bool,
    /// Whether to try HTTP/3.
    pub try_http3: bool,
    /// Whether to try a proxy/tunnel.
    pub try_tunnel: bool,
    /// Preferred IP family.
    pub ip_family: String,
    /// Watchdog recovery action string.
    pub recovery_action: String,
    /// Human-readable rationale lines.
    pub explanation: Vec<String>,
}

impl Default for RecommendedProfile {
    fn default() -> Self {
        Self {
            source: "throttle-probe".to_owned(),
            domestic_connections: 16,
            international_connections: 8,
            default_connections: 16,
            recycle_interval_sec: 15.0,
            use_mirrors: true,
            try_http3: false,
            try_tunnel: false,
            ip_family: "auto".to_owned(),
            recovery_action: "tier2_pause_unpause".to_owned(),
            explanation: Vec::new(),
        }
    }
}

/// Neutral, factual summary suitable for sharing with an ISP support team.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ISPEvidenceSummary {
    /// ISP label.
    pub isp: String,
    /// Sorted observation dates.
    pub dates: Vec<String>,
    /// Tested hosts.
    pub targets_tested: Vec<String>,
    /// Methodology statements.
    pub methodology: Vec<String>,
    /// Measured numbers as printable lines.
    pub measured_numbers: Vec<String>,
    /// Neutral questions for the ISP.
    pub neutral_questions: Vec<String>,
}

impl Default for ISPEvidenceSummary {
    fn default() -> Self {
        Self {
            isp: "Unknown ISP".to_owned(),
            dates: Vec::new(),
            targets_tested: Vec::new(),
            methodology: Vec::new(),
            measured_numbers: Vec::new(),
            neutral_questions: Vec::new(),
        }
    }
}

/// Output of the pure rule-based classifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClassificationResult {
    /// Verdicts sorted by confidence (descending).
    pub verdicts: Vec<Verdict>,
    /// Auxiliary notes.
    pub notes: Vec<VerdictNote>,
    /// Recommended download profile.
    pub recommended_profile: RecommendedProfile,
    /// Neutral ISP evidence summary.
    pub isp_evidence: ISPEvidenceSummary,
    /// Time-of-day coverage status string.
    pub time_of_day_status: String,
    /// Next-best-hour suggestions (byte-exact goldens).
    pub next_best_hours: Vec<String>,
    /// `paired_same_moment` or `paired_and_time_of_day`.
    pub primary_evidence_mode: String,
}

impl Default for ClassificationResult {
    fn default() -> Self {
        Self {
            verdicts: Vec::new(),
            notes: Vec::new(),
            recommended_profile: RecommendedProfile::default(),
            isp_evidence: ISPEvidenceSummary::default(),
            time_of_day_status: "insufficient time data".to_owned(),
            next_best_hours: Vec::new(),
            primary_evidence_mode: "paired_same_moment".to_owned(),
        }
    }
}

/// Data-budget accounting for one diagnostic run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetSummary {
    /// Hard byte limit.
    pub limit_bytes: i64,
    /// Planned worst-case byte usage.
    pub planned_worst_case_bytes: i64,
    /// Actual bytes consumed.
    pub actual_used_bytes: i64,
    /// Whether the limit was exceeded.
    pub exceeded: bool,
}

/// Full versioned report serialized to `report.json`, `report.md`, and HTML.
#[derive(Debug, Clone, PartialEq)]
pub struct ThrottleReport {
    /// Schema version string (always `"1.0"`).
    pub schema_version: String,
    /// Run identifier.
    pub run_id: String,
    /// Run mode.
    pub mode: String,
    /// ISP label.
    pub isp: String,
    /// Subscribed plan in Mbps.
    pub plan_mbps: Option<f64>,
    /// ISO-8601 run timestamp.
    pub timestamp: String,
    /// Data-budget summary.
    pub budget: BudgetSummary,
    /// Validated targets.
    pub targets: Vec<ValidatedTarget>,
    /// Complete measurement dataset.
    pub dataset: MeasurementDataset,
    /// Classifier output.
    pub classification: ClassificationResult,
}

impl Default for ThrottleReport {
    fn default() -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION.to_owned(),
            run_id: String::new(),
            mode: "quick".to_owned(),
            isp: "Unknown ISP".to_owned(),
            plan_mbps: None,
            timestamp: String::new(),
            budget: BudgetSummary {
                limit_bytes: 300 * 1024 * 1024,
                planned_worst_case_bytes: 0,
                actual_used_bytes: 0,
                exceeded: false,
            },
            targets: Vec::new(),
            dataset: MeasurementDataset::default(),
            classification: ClassificationResult::default(),
        }
    }
}
