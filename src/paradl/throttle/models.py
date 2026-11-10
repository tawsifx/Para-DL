"""Domain models and JSON schema definitions for the ISP Throttling Diagnostic Suite."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from enum import StrEnum
from typing import Any

REPORT_SCHEMA_VERSION = "1.0"


class VerdictClass(StrEnum):
    """Canonical verdict classes emitted by the rule-based classifier."""

    NO_THROTTLING = "no-throttling"
    LINE_CAPACITY_LIMIT = "line-capacity limit"
    PER_FLOW_CAP = "per-flow cap"
    BURST_THEN_THROTTLE = "burst-then-throttle"
    PER_DESTINATION_LIMIT = "per-destination limit"
    LOSS_BASED_POLICING = "loss-based policing"
    QUEUE_BASED_SHAPING = "queue-based shaping"
    PROTOCOL_PORT_DIFF = "protocol/port differentiation"
    SNI_SERVICE_DIFF = "SNI or service-based differentiation"
    TIME_OF_DAY_CONGESTION = "time-of-day congestion"
    VOLUME_FUP_THROTTLING = "volume-based (FUP) throttling"
    PEERING_TRANSIT_CONGESTION = "peering/transit congestion"
    INTERNATIONAL_BW_LIMIT = "international bandwidth limit"
    NO_RANGE_SUPPORT = "200-only (no Range)"
    INCONCLUSIVE = "inconclusive"


@dataclass
class TargetCandidate:
    """Candidate endpoint loaded from targets.toml or CLI flags."""

    id: str
    url: str
    group: str  # global-cdn, domestic, isp-cached, problem-hosts, own-server
    label: str = ""
    alt_port: int | None = None
    http_url: str | None = None
    supports_upload: bool = False
    sni_probe_allowed: bool = False
    sni_alt_host: str | None = None


@dataclass
class ValidatedTarget:
    """Target that passed runtime reachability, status, and TLS validation."""

    id: str
    url: str
    host: str
    ip: str
    group: str
    label: str = ""
    status_code: int = 200
    supports_range: bool = True
    content_length: int | None = None
    tls_ok: bool = True
    country: str = "UNKNOWN"
    asn: str = "AS0"
    asn_org: str = "Unknown"
    locality: str = "international"  # "domestic", "international", "isp-cached", "loopback"
    alt_port: int | None = None
    http_url: str | None = None
    supports_upload: bool = False
    sni_probe_allowed: bool = False
    sni_alt_host: str | None = None


@dataclass
class CompetingProcess:
    """Competing network process observed in a nettop snapshot."""

    process: str
    interface: str
    state: str
    bytes_in: int
    bytes_out: int


@dataclass
class SanityResult:
    """M0: Sanity and local environment factors."""

    loopback_bps: float = 0.0
    loopback_ok: bool = True
    active_interface: str = "en0"
    interface_type: str = "Ethernet"  # "Wi-Fi", "Ethernet", "VPN/Tunnel", "Loopback"
    ssid: str = ""
    wifi_link_mbps: float | None = None
    vpn_or_tunnel_active: bool = False
    tunnel_interfaces: list[str] = field(default_factory=list)
    cpu_load_1m: float = 0.0
    competing_bytes_total: int = 0
    competing_processes: list[CompetingProcess] = field(default_factory=list)
    on_battery: bool = False
    contaminated: bool = False
    warnings: list[str] = field(default_factory=list)


@dataclass
class PathBaselineTarget:
    """M1: Per-target baseline path quality."""

    target_id: str
    host: str
    ip: str
    dns_ms: float = 0.0
    tcp_connect_ms: float = 0.0
    rtt_min_ms: float = 0.0
    rtt_avg_ms: float = 0.0
    rtt_max_ms: float = 0.0
    jitter_ms: float = 0.0
    loss_pct: float = 0.0
    path_mtu: int = 1500


@dataclass
class BaselinePathResult:
    """M1: Aggregate baseline path quality including multi-resolver DNS."""

    resolver_dns_ms: dict[str, float] = field(default_factory=dict)
    targets: list[PathBaselineTarget] = field(default_factory=list)


@dataclass
class SingleFlowSeries:
    """M2: Single-connection throughput time series sampled every 250 ms."""

    target_id: str
    host: str
    locality: str
    duration_sec: float
    sample_interval_sec: float = 0.25
    total_bytes: int = 0
    avg_bps: float = 0.0
    peak_bps: float = 0.0
    sustained_bps: float = 0.0
    supports_range: bool = True
    samples_bps: list[float] = field(default_factory=list)


@dataclass
class FlowScalePoint:
    """M3: Single point on the connection flow-scaling curve."""

    connections: int
    aggregate_bps: float
    per_flow_bps: float
    duration_sec: float = 1.0
    bytes_transferred: int = 0


@dataclass
class FlowScalingResult:
    """M3: Flow-scaling curve for a target across 1, 2, 4, 8, 16 (and optional 32) streams."""

    target_id: str
    host: str
    locality: str
    supports_range: bool = True
    points: list[FlowScalePoint] = field(default_factory=list)
    scaling_ratio_16_to_1: float = 1.0
    scaling_ratio_4_to_1: float = 1.0


@dataclass
class DestinationIndependenceResult:
    """M4: Concurrent two-host download vs solo host download comparison."""

    host_a: str = ""
    host_b: str = ""
    solo_a_bps: float = 0.0
    solo_b_bps: float = 0.0
    concurrent_a_bps: float = 0.0
    concurrent_b_bps: float = 0.0
    concurrent_total_bps: float = 0.0
    independence_ratio: float = 1.0  # concurrent_total / max(solo_a, solo_b)


@dataclass
class BurstBucketResult:
    """M5: Burst rate, token bucket estimation, drop shape, and idle refill analysis."""

    target_id: str = ""
    host: str = ""
    initial_burst_bps: float = 0.0
    burst_duration_sec: float = 0.0
    sustained_bps: float = 0.0
    burst_to_sustained_ratio: float = 1.0
    estimated_bucket_bytes: int = 0
    drop_shape: str = "flat"  # "step", "decay", "periodic", "flat"
    refill_30s_burst_bps: float | None = None
    refill_120s_burst_bps: float | None = None
    refill_30s_ratio: float | None = None  # refill_30s_burst / initial_burst
    refill_120s_ratio: float | None = None  # refill_120s_burst / initial_burst


@dataclass
class PolicingShapingResult:
    """M6: RTT under load vs idle and TCP retransmit/loss deltas."""

    target_id: str = ""
    host: str = ""
    idle_rtt_ms: float = 0.0
    loaded_rtt_ms: float = 0.0
    rtt_inflation_ms: float = 0.0
    rtt_inflation_ratio: float = 1.0
    tcp_packets_sent_delta: int = 0
    tcp_retransmits_delta: int = 0
    retransmit_rate_pct: float = 0.0
    connection_resets: int = 0
    throughput_cov: float = 0.0  # Coefficient of variation (sawtooth vs smooth)
    mechanism_hint: str = "none"  # "policing", "shaping", "congestion", "none"


@dataclass
class ProtocolPortResult:
    """M7: Protocol, port, IP family, and best-effort SNI differentiation."""

    host: str = ""
    http1_1_bps: float | None = None
    http2_bps: float | None = None
    http3_bps: float | None = None
    http3_status: str = "skipped"  # "measured", "skipped", "unsupported"
    https_bps: float | None = None
    http_plain_bps: float | None = None
    port_443_bps: float | None = None
    alt_port: int | None = None
    alt_port_bps: float | None = None
    ipv4_bps: float | None = None
    ipv6_bps: float | None = None
    sni_default_bps: float | None = None
    sni_alt_bps: float | None = None
    sni_label: str = "best effort"


@dataclass
class SampleRecord:
    """M8: Stored active or passive time-of-day throughput sample."""

    timestamp: str
    epoch: float
    hour: int
    is_weekend: bool
    interface: str
    ssid: str
    network_id: str
    source: str = "active"  # "active" (weight 1.0) or "passive" (weight 0.35)
    target_group: str = "international"
    host: str = ""
    download_bps: float = 0.0
    upload_bps: float = 0.0
    rtt_ms: float = 0.0
    bytes_used: int = 0
    series_bps: list[float] = field(default_factory=list)

    @property
    def weight(self) -> float:
        return 1.0 if self.source == "active" else 0.35


@dataclass
class CoverageCell:
    """M8: Single cell in the 24-hour x (weekday, weekend) coverage matrix."""

    hour: int
    is_weekend: bool
    active_count: int = 0
    passive_count: int = 0
    effective_weight: float = 0.0
    median_bps: float | None = None
    mean_bps: float | None = None

    @property
    def has_data(self) -> bool:
        return (self.active_count + self.passive_count) > 0


@dataclass
class TimeVolumeUploadResult:
    """M8: Time-of-day coverage, within-session drift, cumulative volume FUP, and upload test."""

    session_start_probe_bps: float = 0.0
    session_end_probe_bps: float = 0.0
    session_drift_ratio: float = 1.0
    volume_tested_bytes: int = 0
    volume_pre_step_bps: float = 0.0
    volume_post_step_bps: float = 0.0
    volume_step_down_at_bytes: int | None = None
    download_bps: float = 0.0
    upload_bps: float = 0.0
    asymmetry_ratio: float = 1.0  # download_bps / max(upload_bps, 1.0)
    samples: list[SampleRecord] = field(default_factory=list)


@dataclass
class GroundTruthResult:
    """M9: Paired --via-proxy and own-server port/payload comparison."""

    proxy_url_redacted: str | None = None
    direct_1_bps: float | None = None
    proxy_1_bps: float | None = None
    direct_16_bps: float | None = None
    proxy_16_bps: float | None = None
    proxy_speedup_ratio: float | None = None
    own_server_random_bps: float | None = None
    own_server_video_bps: float | None = None
    own_server_port_sweep: dict[str, float] = field(default_factory=dict)
    udp_echo_loss_pct: float | None = None


@dataclass
class TracerouteHop:
    """Single hop from `traceroute -n -q 1 -w 1`."""

    hop: int
    ip: str
    rtt_ms: float | None = None
    asn: str = "AS0"
    country: str = ""


@dataclass
class TargetPathTrace:
    """M10: Traceroute path and ASN sequence for a target."""

    target_id: str
    host: str
    dest_ip: str
    dest_asn: str
    dest_country: str
    locality: str
    throughput_1_bps: float = 0.0
    throughput_16_bps: float = 0.0
    rtt_ms: float = 0.0
    hops: list[TracerouteHop] = field(default_factory=list)
    upstream_asns: list[str] = field(default_factory=list)


@dataclass
class LocalVsInternationalResult:
    """M10: Domestic vs International vs ISP-Cached paired comparison & transit ASN correlation."""

    domestic_median_bps: float | None = None
    international_median_bps: float | None = None
    isp_cached_median_bps: float | None = None
    domestic_16_bps: float | None = None
    international_16_bps: float | None = None
    isp_cached_16_bps: float | None = None
    shared_slow_upstream_asn: str | None = None
    fast_international_asns: list[str] = field(default_factory=list)
    slow_international_asns: list[str] = field(default_factory=list)
    traces: list[TargetPathTrace] = field(default_factory=list)


@dataclass
class MeasurementDataset:
    """Complete input to the pure rule-based classifier."""

    isp: str = "Unknown ISP"
    mode: str = "quick"
    plan_mbps: float | None = None
    m0_sanity: SanityResult = field(default_factory=SanityResult)
    m1_baseline: BaselinePathResult = field(default_factory=BaselinePathResult)
    m2_single_flow: list[SingleFlowSeries] = field(default_factory=list)
    m3_flow_scaling: list[FlowScalingResult] = field(default_factory=list)
    m4_destination_independence: DestinationIndependenceResult = field(
        default_factory=DestinationIndependenceResult
    )
    m5_burst_bucket: list[BurstBucketResult] = field(default_factory=list)
    m6_policing_shaping: list[PolicingShapingResult] = field(default_factory=list)
    m7_protocol_port: list[ProtocolPortResult] = field(default_factory=list)
    m8_time_volume_upload: TimeVolumeUploadResult = field(default_factory=TimeVolumeUploadResult)
    m9_ground_truth: GroundTruthResult = field(default_factory=GroundTruthResult)
    m10_local_vs_intl: LocalVsInternationalResult = field(
        default_factory=LocalVsInternationalResult
    )

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


@dataclass
class Verdict:
    """Single classifier verdict with confidence score and supporting evidence."""

    verdict_class: str
    confidence: float
    summary: str
    evidence: list[str] = field(default_factory=list)


@dataclass
class VerdictNote:
    """Auxiliary diagnostic note (such as 'cache effect' or 'insufficient time data')."""

    code: str  # e.g. "cache_effect", "insufficient_time_data", "contaminated_environment"
    title: str
    text: str
    recommendations: list[str] = field(default_factory=list)


@dataclass
class RecommendedProfile:
    """Actionable download tuning profile derived from diagnostic verdicts."""

    source: str = "throttle-probe"
    domestic_connections: int = 16
    international_connections: int = 8
    default_connections: int = 16
    recycle_interval_sec: float = 15.0
    use_mirrors: bool = True
    try_http3: bool = False
    try_tunnel: bool = False
    ip_family: str = "auto"
    recovery_action: str = "tier2_pause_unpause"
    explanation: list[str] = field(default_factory=list)


@dataclass
class ISPEvidenceSummary:
    """Neutral, factual summary suitable for sharing with the user's ISP support team."""

    isp: str
    dates: list[str] = field(default_factory=list)
    targets_tested: list[str] = field(default_factory=list)
    methodology: list[str] = field(default_factory=list)
    measured_numbers: list[str] = field(default_factory=list)
    neutral_questions: list[str] = field(default_factory=list)


@dataclass
class ClassificationResult:
    """Output of the pure rule-based classifier."""

    verdicts: list[Verdict] = field(default_factory=list)
    notes: list[VerdictNote] = field(default_factory=list)
    recommended_profile: RecommendedProfile = field(default_factory=RecommendedProfile)
    isp_evidence: ISPEvidenceSummary = field(
        default_factory=lambda: ISPEvidenceSummary(isp="Unknown ISP")
    )
    time_of_day_status: str = "insufficient time data"
    next_best_hours: list[str] = field(default_factory=list)
    primary_evidence_mode: str = "paired_same_moment"


@dataclass
class BudgetSummary:
    """Data budget accounting for a diagnostic run."""

    limit_bytes: int
    planned_worst_case_bytes: int
    actual_used_bytes: int
    exceeded: bool = False


@dataclass
class ThrottleReport:
    """Full versioned report model serialized to report.json, report.md, and report.html."""

    schema_version: str = REPORT_SCHEMA_VERSION
    run_id: str = ""
    mode: str = "quick"
    isp: str = "Unknown ISP"
    plan_mbps: float | None = None
    timestamp: str = ""
    budget: BudgetSummary = field(
        default_factory=lambda: BudgetSummary(
            limit_bytes=300 * 1024 * 1024,
            planned_worst_case_bytes=0,
            actual_used_bytes=0,
        )
    )
    targets: list[ValidatedTarget] = field(default_factory=list)
    dataset: MeasurementDataset = field(default_factory=MeasurementDataset)
    classification: ClassificationResult = field(default_factory=ClassificationResult)

    def to_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "run_id": self.run_id,
            "mode": self.mode,
            "isp": self.isp,
            "plan_mbps": self.plan_mbps,
            "timestamp": self.timestamp,
            "budget": asdict(self.budget),
            "environment": asdict(self.dataset.m0_sanity),
            "targets": [asdict(t) for t in self.targets],
            "modules": {
                "m0_sanity": asdict(self.dataset.m0_sanity),
                "m1_baseline": asdict(self.dataset.m1_baseline),
                "m2_single_flow": [asdict(x) for x in self.dataset.m2_single_flow],
                "m3_flow_scaling": [asdict(x) for x in self.dataset.m3_flow_scaling],
                "m4_destination_independence": asdict(self.dataset.m4_destination_independence),
                "m5_burst_bucket": [asdict(x) for x in self.dataset.m5_burst_bucket],
                "m6_policing_shaping": [asdict(x) for x in self.dataset.m6_policing_shaping],
                "m7_protocol_port": [asdict(x) for x in self.dataset.m7_protocol_port],
                "m8_time_volume_upload": asdict(self.dataset.m8_time_volume_upload),
                "m9_ground_truth": asdict(self.dataset.m9_ground_truth),
                "m10_local_vs_intl": asdict(self.dataset.m10_local_vs_intl),
            },
            "verdicts": [asdict(v) for v in self.classification.verdicts],
            "notes": [asdict(n) for n in self.classification.notes],
            "time_of_day_status": self.classification.time_of_day_status,
            "next_best_hours": list(self.classification.next_best_hours),
            "primary_evidence_mode": self.classification.primary_evidence_mode,
            "recommended_profile": asdict(self.classification.recommended_profile),
            "isp_evidence_summary": asdict(self.classification.isp_evidence),
        }
