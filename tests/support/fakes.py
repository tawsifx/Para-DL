"""Shared test fakes and domain factories for pdl tests.

Provides:
- FakeAria2Server: In-process fake JSON-RPC daemon
- FakeKeychain: In-memory macOS Keychain token store
- FakeClock: Re-exported deterministic clock
- Factories: target_factory, job_factory, result_factory, archive_factory
"""

from __future__ import annotations

import io
import tarfile
import zipfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

from pdl.clock import FakeClock
from pdl.models import (
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
)
from pdl.queue import QueueJob
from pdl.throttle.models import (
    BaselinePathResult,
    BurstBucketResult,
    DestinationIndependenceResult,
    FlowScalePoint,
    FlowScalingResult,
    GroundTruthResult,
    LocalVsInternationalResult,
    MeasurementDataset,
    PolicingShapingResult,
    ProtocolPortResult,
    SampleRecord,
    SanityResult,
    SingleFlowSeries,
    TimeVolumeUploadResult,
    VerdictClass,
)

__all__ = [
    "CLASSIFIER_SCENARIO_TABLE",
    "FakeASNResolver",
    "FakeAria2Server",
    "FakeClock",
    "FakeKeychain",
    "archive_factory",
    "gappy_time_samples_factory",
    "job_factory",
    "measurement_dataset_factory",
    "metalink_factory",
    "result_factory",
    "target_factory",
]


class FakeKeychain:
    """In-memory replacement for macOS Login Keychain storage."""

    def __init__(self) -> None:
        self._store: dict[tuple[str, str], str] = {}

    def set(self, service: str, account: str, secret: str) -> None:
        self._store[(service, account)] = secret

    def get(self, service: str, account: str) -> str | None:
        return self._store.get((service, account))

    def delete(self, service: str, account: str) -> bool:
        return self._store.pop((service, account), None) is not None

    def clear(self) -> None:
        self._store.clear()


class FakeAria2Server:
    """In-memory handler simulating aria2 JSON-RPC responses."""

    def __init__(self, secret: str = "fake_rpc_secret") -> None:
        self.secret = secret
        self.downloads: dict[str, dict[str, Any]] = {}
        self.options: dict[str, dict[str, str]] = {}
        self.call_history: list[tuple[str, tuple[Any, ...]]] = []
        self._next_gid: int = 1000

    def call(self, method: str, *params: Any) -> Any:
        self.call_history.append((method, params))
        # Validate RPC token
        token_param = str(params[0]) if params else ""
        if token_param != f"token:{self.secret}":
            raise PermissionError("Unauthorized aria2 RPC secret")

        actual_params = params[1:] if len(params) > 1 else ()

        if method == "aria2.getVersion":
            return {"version": "1.37.0", "enabledFeatures": ["BitTorrent", "GZip", "HTTPS", "MessageDigest"]}

        if method == "aria2.addUri":
            _uris = actual_params[0] if actual_params else []
            opts = actual_params[1] if len(actual_params) > 1 else {}
            gid = f"{self._next_gid:016x}"
            self._next_gid += 1
            self.downloads[gid] = {
                "gid": gid,
                "status": "active",
                "totalLength": "10485760",
                "completedLength": "5242880",
                "downloadSpeed": "1048576",
                "files": [{"path": "/tmp/test.bin", "length": "10485760", "completedLength": "5242880"}],
            }
            self.options[gid] = dict(opts)
            return gid

        if method == "aria2.addMetalink":
            gid = f"{self._next_gid:016x}"
            self._next_gid += 1
            opts = actual_params[1] if len(actual_params) > 1 else {}
            self.downloads[gid] = {
                "gid": gid,
                "status": "active",
                "totalLength": "5242880",
                "completedLength": "5242880",
                "downloadSpeed": "524288",
                "files": [{"path": "/tmp/metalink.bin", "length": "5242880", "completedLength": "5242880"}],
            }
            self.options[gid] = dict(opts)
            return [gid]

        if method == "aria2.tellStatus":
            gid = str(actual_params[0])
            if gid in self.downloads:
                return dict(self.downloads[gid])
            return {"gid": gid, "status": "complete", "totalLength": "0", "completedLength": "0", "downloadSpeed": "0"}

        if method == "aria2.getGlobalStat":
            active_count = sum(1 for d in self.downloads.values() if d.get("status") == "active")
            return {
                "downloadSpeed": "2097152" if active_count > 0 else "0",
                "uploadSpeed": "0",
                "numActive": str(active_count),
                "numWaiting": "0",
                "numStopped": "0",
            }

        if method == "aria2.forcePause":
            gid = str(actual_params[0])
            if gid in self.downloads:
                self.downloads[gid]["status"] = "paused"
            return gid

        if method == "aria2.unpause":
            gid = str(actual_params[0])
            if gid in self.downloads:
                self.downloads[gid]["status"] = "active"
            return gid

        if method == "aria2.changeOption":
            gid = str(actual_params[0])
            new_opts = actual_params[1]
            if gid in self.options:
                self.options[gid].update(new_opts)
            return "OK"

        if method == "aria2.shutdown":
            return "OK"

        return {}


# ---------------------------------------------------------------------------
# Domain Model Factories
# ---------------------------------------------------------------------------


def target_factory(
    raw: str = "https://example.com/data.zip",
    target_type: TargetType = TargetType.DIRECT,
    clean_url: str | None = None,
    filename_hint: str = "data.zip",
    content_length: int = 1048576,
    accept_ranges: bool = True,
    expected_digest: str | None = None,
    **kwargs: Any,
) -> TargetInfo:
    """Create a TargetInfo domain model with sensible defaults."""
    return TargetInfo(
        raw=raw,
        target_type=target_type,
        clean_url=clean_url or raw,
        filename_hint=filename_hint,
        content_length=content_length,
        accept_ranges=accept_ranges,
        expected_digest=expected_digest,
        **kwargs,
    )


def job_factory(
    job_id: int = 1,
    target: str = "https://example.com/archive.tar.gz",
    target_type: str = "direct",
    out_dir: str = "/tmp/downloads",
    filename: str = "archive.tar.gz",
    priority: int = 50,
    connections: int = 8,
    status: str = "queued",
    require_ac: bool = False,
    pause_on_low_power: bool = False,
    when_fast_bps: float = 0.0,
    start_at: str | None = None,
    expected_length: int = 1048576,
    expected_digest: str | None = None,
    bytes_downloaded: int = 0,
    error: str | None = None,
    mirrors_json: str | None = None,
    options_json: str | None = None,
) -> QueueJob:
    """Create a QueueJob model instance for queue tests."""
    return QueueJob(
        id=job_id,
        target=target,
        target_type=target_type,
        out_dir=out_dir,
        filename=filename,
        priority=priority,
        connections=connections,
        status=status,
        require_ac=require_ac,
        pause_on_low_power=pause_on_low_power,
        when_fast_bps=when_fast_bps,
        start_at=start_at,
        expected_length=expected_length,
        expected_digest=expected_digest,
        bytes_downloaded=bytes_downloaded,
        error=error,
        mirrors_json=mirrors_json,
        options_json=options_json,
    )


def result_factory(
    target: str = "https://example.com/test.bin",
    success: bool = True,
    exit_code: ExitCode = ExitCode.OK,
    bytes_downloaded: int = 1048576,
    duration: float = 1.0,
    file_path: Path | None = None,
    error: str | None = None,
    **kwargs: Any,
) -> TransferResult:
    """Create a TransferResult model instance for engine/app tests."""
    return TransferResult(
        target=target,
        success=success,
        exit_code=exit_code,
        bytes_downloaded=bytes_downloaded,
        duration=duration,
        file_path=file_path or Path("/tmp/test.bin"),
        error=error,
        **kwargs,
    )


def archive_factory(
    dest_path: Path,
    members: Mapping[str, bytes | str],
    kind: str = "zip",
) -> Path:
    """Synthesize a safe or intentionally unsafe zip/tar archive on disk."""
    dest_path.parent.mkdir(parents=True, exist_ok=True)
    if kind == "zip":
        with zipfile.ZipFile(dest_path, "w") as zf:
            for member_name, content in members.items():
                data = content.encode() if isinstance(content, str) else content
                zf.writestr(member_name, data)
    elif kind in ("tar", "tar.gz", "tar.bz2"):
        mode = "w:gz" if kind == "tar.gz" else ("w:bz2" if kind == "tar.bz2" else "w")
        with tarfile.open(dest_path, mode) as tf:
            for member_name, content in members.items():
                data = content.encode() if isinstance(content, str) else content
                ti = tarfile.TarInfo(name=member_name)
                ti.size = len(data)
                tf.addfile(ti, io.BytesIO(data))
    return dest_path


def metalink_factory(
    filename: str,
    file_data: bytes,
    urls: list[str],
    piece_length: int = 65536,
) -> bytes:
    """Build a valid RFC 5854 .meta4 XML document with full-file and piece SHA-256 hashes."""
    import hashlib

    full_sha256 = hashlib.sha256(file_data).hexdigest()
    pieces_xml: list[str] = []
    for offset in range(0, len(file_data), piece_length):
        chunk = file_data[offset : offset + piece_length]
        piece_hash = hashlib.sha256(chunk).hexdigest()
        pieces_xml.append(f"      <hash>{piece_hash}</hash>")
    pieces_block = "\n".join(pieces_xml)
    urls_block = "\n".join(f'    <url priority="{idx + 1}">{u}</url>' for idx, u in enumerate(urls))
    xml_text = f"""<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="{filename}">
    <size>{len(file_data)}</size>
    <hash type="sha-256">{full_sha256}</hash>
    <pieces length="{piece_length}" type="sha-256">
{pieces_block}
    </pieces>
{urls_block}
  </file>
</metalink>
"""
    return xml_text.encode("utf-8")


# ---------------------------------------------------------------------------
# ISP Throttling Diagnostic Suite Shared Fakes & Factories
# ---------------------------------------------------------------------------


class FakeASNResolver:
    """Deterministic in-memory IP -> (ASN, country, org) resolver for tests."""

    def __init__(self, mapping: dict[str, tuple[str, str, str]] | None = None) -> None:
        self.calls: list[tuple[str, str]] = []
        self.mapping: dict[str, tuple[str, str, str]] = mapping or {
            "103.10.20.1": ("AS58717", "BD", "Duronto-BDIX-Peer"),
            "103.10.20.2": ("AS15169", "BD", "Duronto-GGC-Edge"),
            "1.1.1.1": ("AS13335", "SG", "Cloudflare-SG"),
            "185.199.108.153": ("AS54113", "US", "Fastly-GitHub"),
            "64.233.160.1": ("AS9498", "IN", "Bharti-Airtel-Transit"),
        }

    def __call__(self, ip: str, host_hint: str = "") -> tuple[str, str, str]:
        self.calls.append((ip, host_hint))
        if ip in self.mapping:
            return self.mapping[ip]
        hint = host_hint.lower()
        if "domestic" in hint or "bdix" in hint or ".bd" in hint:
            return ("AS58717", "BD", "Duronto-BDIX-Peer")
        if "cached" in hint or "ggc" in hint or "googlevideo" in hint:
            return ("AS15169", "BD", "Duronto-GGC-Edge")
        return ("AS13335", "US", "International-CDN")


def gappy_time_samples_factory(kind: str) -> list[SampleRecord]:
    """Build synthetic gappy or multi-window time-of-day sample lists for M8 classifier tests."""
    if kind == "evening_only":
        return [
            SampleRecord(
                timestamp=f"2026-11-0{d}T21:00:00+0600",
                epoch=1794000000.0 + d * 86400,
                hour=21,
                is_weekend=False,
                interface="en0",
                ssid="Duronto-5G",
                network_id="en0:Duronto-5G",
                source="active",
                target_group="international",
                host="speed.cloudflare.com",
                download_bps=400_000.0,
                rtt_ms=65.0,
            )
            for d in range(1, 5)
        ]
    if kind == "weekend_only":
        return [
            SampleRecord(
                timestamp=f"2026-11-07T{h:02d}:00:00+0600",
                epoch=1794000000.0 + i * 3600,
                hour=h,
                is_weekend=True,
                interface="en0",
                ssid="Duronto-5G",
                network_id="en0:Duronto-5G",
                source="active",
                target_group="international",
                host="speed.cloudflare.com",
                download_bps=500_000.0 if h >= 19 else 2_500_000.0,
                rtt_ms=55.0,
            )
            for i, h in enumerate((9, 10, 11, 20, 21, 22))
        ]
    if kind == "single_bucket":
        return [
            SampleRecord(
                timestamp="2026-11-05T14:00:00+0600",
                epoch=1794000000.0 + i * 3600,
                hour=14,
                is_weekend=False,
                interface="en0",
                ssid="Duronto-5G",
                network_id="en0:Duronto-5G",
                source="active",
                target_group="international",
                host="speed.cloudflare.com",
                download_bps=1_200_000.0,
                rtt_ms=42.0,
            )
            for i in range(4)
        ]
    if kind == "two_buckets_evening_slowdown":
        samples: list[SampleRecord] = []
        # Morning off-peak bucket (hours 06..11, 5 active samples around 5.0 MB/s)
        for i, h in enumerate((8, 9, 9, 10, 11)):
            samples.append(
                SampleRecord(
                    timestamp=f"2026-11-0{i + 1}T{h:02d}:00:00+0600",
                    epoch=1794000000.0 + i * 86400,
                    hour=h,
                    is_weekend=False,
                    interface="en0",
                    ssid="Duronto-5G",
                    network_id="en0:Duronto-5G",
                    source="active",
                    target_group="international",
                    host="speed.cloudflare.com",
                    download_bps=5_000_000.0 + i * 50_000.0,
                    rtt_ms=35.0,
                )
            )
        # Evening peak bucket (hours 18..23, 5 active samples around 1.1 MB/s)
        for i, h in enumerate((19, 20, 21, 21, 22)):
            samples.append(
                SampleRecord(
                    timestamp=f"2026-11-0{i + 1}T{h:02d}:00:00+0600",
                    epoch=1794043200.0 + i * 86400,
                    hour=h,
                    is_weekend=False,
                    interface="en0",
                    ssid="Duronto-5G",
                    network_id="en0:Duronto-5G",
                    source="active",
                    target_group="international",
                    host="speed.cloudflare.com",
                    download_bps=1_100_000.0 + i * 25_000.0,
                    rtt_ms=95.0,
                )
            )
        return samples
    return []


def measurement_dataset_factory(scenario: str) -> MeasurementDataset:
    """Create a synthetic `MeasurementDataset` for a named classifier scenario."""
    ds = MeasurementDataset(
        isp="Duronto Internet",
        mode="quick",
        plan_mbps=40.0,  # 5,000,000 B/s
        m0_sanity=SanityResult(loopback_bps=150_000_000.0, loopback_ok=True, contaminated=False),
        m1_baseline=BaselinePathResult(),
    )

    if scenario == "per_flow_cap":
        ds.m2_single_flow = [
            SingleFlowSeries(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                duration_sec=10.0,
                avg_bps=350_000.0,
                peak_bps=380_000.0,
                sustained_bps=350_000.0,
            )
        ]
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 350_000.0, 350_000.0),
                    FlowScalePoint(4, 1_350_000.0, 337_500.0),
                    FlowScalePoint(16, 4_500_000.0, 281_250.0),
                ],
                scaling_ratio_4_to_1=3.86,
                scaling_ratio_16_to_1=12.86,
            )
        ]
        return ds

    if scenario == "shared_host_cap":
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="gh-1",
                host="objects.githubusercontent.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 1_000_000.0, 1_000_000.0),
                    FlowScalePoint(4, 1_050_000.0, 262_500.0),
                    FlowScalePoint(16, 1_080_000.0, 67_500.0),
                ],
                scaling_ratio_4_to_1=1.05,
                scaling_ratio_16_to_1=1.08,
            )
        ]
        ds.m4_destination_independence = DestinationIndependenceResult(
            host_a="objects.githubusercontent.com",
            host_b="cdn-lfs.hf.co",
            solo_a_bps=1_050_000.0,
            solo_b_bps=1_050_000.0,
            concurrent_a_bps=1_000_000.0,
            concurrent_b_bps=1_000_000.0,
            concurrent_total_bps=2_000_000.0,
            independence_ratio=1.90,
        )
        return ds

    if scenario == "burst_then_throttle":
        ds.m5_burst_bucket = [
            BurstBucketResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                initial_burst_bps=4_800_000.0,
                burst_duration_sec=5.0,
                sustained_bps=800_000.0,
                burst_to_sustained_ratio=6.0,
                estimated_bucket_bytes=20_000_000,
                drop_shape="step",
                refill_30s_ratio=0.45,
                refill_120s_ratio=0.98,
            )
        ]
        return ds

    if scenario == "uniformly_slow":
        ds.m2_single_flow = [
            SingleFlowSeries(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                duration_sec=10.0,
                avg_bps=1_200_000.0,
                peak_bps=1_250_000.0,
                sustained_bps=1_200_000.0,
            )
        ]
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 1_200_000.0, 1_200_000.0),
                    FlowScalePoint(4, 1_220_000.0, 305_000.0),
                    FlowScalePoint(16, 1_240_000.0, 77_500.0),
                ],
                scaling_ratio_4_to_1=1.02,
                scaling_ratio_16_to_1=1.03,
            )
        ]
        ds.m4_destination_independence = DestinationIndependenceResult(
            host_a="speed.cloudflare.com",
            host_b="proof.ovh.net",
            solo_a_bps=1_200_000.0,
            solo_b_bps=1_200_000.0,
            concurrent_a_bps=610_000.0,
            concurrent_b_bps=610_000.0,
            concurrent_total_bps=1_220_000.0,
            independence_ratio=1.02,
        )
        return ds

    if scenario == "loss_based_policing":
        ds.m6_policing_shaping = [
            PolicingShapingResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                idle_rtt_ms=38.0,
                loaded_rtt_ms=44.0,
                rtt_inflation_ms=6.0,
                rtt_inflation_ratio=1.16,
                tcp_packets_sent_delta=5000,
                tcp_retransmits_delta=180,
                retransmit_rate_pct=3.6,
                connection_resets=2,
                throughput_cov=0.38,
                mechanism_hint="policing",
            )
        ]
        return ds

    if scenario == "queue_based_shaping":
        ds.m6_policing_shaping = [
            PolicingShapingResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                idle_rtt_ms=35.0,
                loaded_rtt_ms=115.0,
                rtt_inflation_ms=80.0,
                rtt_inflation_ratio=3.29,
                tcp_packets_sent_delta=5000,
                tcp_retransmits_delta=5,
                retransmit_rate_pct=0.1,
                connection_resets=0,
                throughput_cov=0.06,
                mechanism_hint="shaping",
            )
        ]
        return ds

    if scenario == "time_of_day_change":
        ds.m8_time_volume_upload = TimeVolumeUploadResult(
            samples=gappy_time_samples_factory("two_buckets_evening_slowdown")
        )
        return ds

    if scenario == "volume_step_down":
        ds.m8_time_volume_upload = TimeVolumeUploadResult(
            volume_tested_bytes=200 * 1024 * 1024,
            volume_pre_step_bps=4_500_000.0,
            volume_post_step_bps=900_000.0,
            volume_step_down_at_bytes=100 * 1024 * 1024,
        )
        return ds

    if scenario == "200_only":
        ds.m2_single_flow = [
            SingleFlowSeries(
                target_id="legacy-1",
                host="legacy.example.org",
                locality="international",
                duration_sec=5.0,
                avg_bps=500_000.0,
                peak_bps=550_000.0,
                sustained_bps=500_000.0,
                supports_range=False,
            )
        ]
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="legacy-1",
                host="legacy.example.org",
                locality="international",
                supports_range=False,
                points=[FlowScalePoint(1, 500_000.0, 500_000.0)],
            )
        ]
        return ds

    if scenario == "protocol_specific":
        ds.m7_protocol_port = [
            ProtocolPortResult(
                host="speed.cloudflare.com",
                http1_1_bps=900_000.0,
                http2_bps=950_000.0,
                http3_bps=3_600_000.0,
                http3_status="measured",
                port_443_bps=900_000.0,
                alt_port=8443,
                alt_port_bps=3_200_000.0,
            )
        ]
        return ds

    if scenario == "sni_or_proxy_diff":
        ds.m9_ground_truth = GroundTruthResult(
            proxy_url_redacted="socks5://127.0.0.1:1080",
            direct_1_bps=750_000.0,
            proxy_1_bps=3_800_000.0,
            direct_16_bps=1_200_000.0,
            proxy_16_bps=4_500_000.0,
            proxy_speedup_ratio=3.75,
        )
        return ds

    if scenario == "domestic_fast_intl_slow_shared_upstream":
        ds.m10_local_vs_intl = LocalVsInternationalResult(
            domestic_median_bps=9_500_000.0,
            domestic_16_bps=11_500_000.0,
            international_median_bps=1_100_000.0,
            international_16_bps=1_250_000.0,
            isp_cached_median_bps=10_000_000.0,
            isp_cached_16_bps=11_800_000.0,
            shared_slow_upstream_asn="AS9498",
            fast_international_asns=[],
            slow_international_asns=["AS13335", "AS54113"],
        )
        return ds

    if scenario == "cached_fast_uncached_slow":
        ds.m10_local_vs_intl = LocalVsInternationalResult(
            domestic_median_bps=None,
            international_median_bps=1_000_000.0,
            international_16_bps=4_200_000.0,
            isp_cached_median_bps=8_500_000.0,
            isp_cached_16_bps=10_500_000.0,
        )
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="intl-1",
                host="objects.githubusercontent.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 1_000_000.0, 1_000_000.0),
                    FlowScalePoint(4, 2_800_000.0, 700_000.0),
                    FlowScalePoint(16, 4_200_000.0, 262_500.0),
                ],
                scaling_ratio_4_to_1=2.8,
                scaling_ratio_16_to_1=4.2,
            )
        ]
        return ds

    if scenario == "per_flow_cap_domestic_and_intl_alike":
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="dom-1",
                host="bdix.duronto.net.bd",
                locality="domestic",
                points=[
                    FlowScalePoint(1, 500_000.0, 500_000.0),
                    FlowScalePoint(4, 1_900_000.0, 475_000.0),
                    FlowScalePoint(16, 6_000_000.0, 375_000.0),
                ],
                scaling_ratio_4_to_1=3.8,
                scaling_ratio_16_to_1=12.0,
            ),
            FlowScalingResult(
                target_id="intl-1",
                host="speed.cloudflare.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 450_000.0, 450_000.0),
                    FlowScalePoint(4, 1_750_000.0, 437_500.0),
                    FlowScalePoint(16, 5_400_000.0, 337_500.0),
                ],
                scaling_ratio_4_to_1=3.89,
                scaling_ratio_16_to_1=12.0,
            ),
        ]
        ds.m10_local_vs_intl = LocalVsInternationalResult(
            domestic_median_bps=500_000.0,
            domestic_16_bps=6_000_000.0,
            international_median_bps=450_000.0,
            international_16_bps=5_400_000.0,
        )
        return ds

    if scenario == "peering_transit_congestion":
        ds.m10_local_vs_intl = LocalVsInternationalResult(
            domestic_median_bps=9_000_000.0,
            domestic_16_bps=10_000_000.0,
            international_median_bps=950_000.0,
            international_16_bps=1_200_000.0,
            shared_slow_upstream_asn="AS9498",
            fast_international_asns=["AS13335"],
            slow_international_asns=["AS54113", "AS16509"],
        )
        return ds

    if scenario == "no_throttling":
        ds.m2_single_flow = [
            SingleFlowSeries(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                duration_sec=10.0,
                avg_bps=4_800_000.0,
                peak_bps=4_950_000.0,
                sustained_bps=4_800_000.0,
            )
        ]
        ds.m3_flow_scaling = [
            FlowScalingResult(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                points=[
                    FlowScalePoint(1, 4_800_000.0, 4_800_000.0),
                    FlowScalePoint(4, 4_950_000.0, 1_237_500.0),
                    FlowScalePoint(16, 5_000_000.0, 312_500.0),
                ],
                scaling_ratio_4_to_1=1.03,
                scaling_ratio_16_to_1=1.04,
            )
        ]
        return ds

    if scenario == "ambiguous_contaminated":
        ds.m0_sanity = SanityResult(
            loopback_bps=150_000_000.0,
            loopback_ok=True,
            vpn_or_tunnel_active=True,
            tunnel_interfaces=["utun3"],
            contaminated=True,
            warnings=["Active VPN/tunnel interface detected: utun3"],
        )
        ds.m2_single_flow = [
            SingleFlowSeries(
                target_id="cf-1",
                host="speed.cloudflare.com",
                locality="international",
                duration_sec=5.0,
                avg_bps=800_000.0,
                peak_bps=950_000.0,
                sustained_bps=800_000.0,
            )
        ]
        return ds

    return ds


# Table of (scenario_name, expected_verdict_class, expected_note_code_or_none, exclude_from_non_tod_accuracy)
CLASSIFIER_SCENARIO_TABLE: list[tuple[str, str, str | None, bool]] = [
    ("per_flow_cap", VerdictClass.PER_FLOW_CAP.value, None, False),
    ("shared_host_cap", VerdictClass.PER_DESTINATION_LIMIT.value, None, False),
    ("burst_then_throttle", VerdictClass.BURST_THEN_THROTTLE.value, None, False),
    ("uniformly_slow", VerdictClass.LINE_CAPACITY_LIMIT.value, None, False),
    ("loss_based_policing", VerdictClass.LOSS_BASED_POLICING.value, None, False),
    ("queue_based_shaping", VerdictClass.QUEUE_BASED_SHAPING.value, None, False),
    ("time_of_day_change", VerdictClass.TIME_OF_DAY_CONGESTION.value, None, True),
    ("volume_step_down", VerdictClass.VOLUME_FUP_THROTTLING.value, None, False),
    ("200_only", VerdictClass.NO_RANGE_SUPPORT.value, None, False),
    ("protocol_specific", VerdictClass.PROTOCOL_PORT_DIFF.value, None, False),
    ("sni_or_proxy_diff", VerdictClass.SNI_SERVICE_DIFF.value, None, False),
    (
        "domestic_fast_intl_slow_shared_upstream",
        VerdictClass.INTERNATIONAL_BW_LIMIT.value,
        "cache_effect",
        False,
    ),
    ("cached_fast_uncached_slow", VerdictClass.PER_FLOW_CAP.value, "cache_effect", False),
    ("per_flow_cap_domestic_and_intl_alike", VerdictClass.PER_FLOW_CAP.value, None, False),
    ("peering_transit_congestion", VerdictClass.PEERING_TRANSIT_CONGESTION.value, None, False),
    ("no_throttling", VerdictClass.NO_THROTTLING.value, None, False),
    ("ambiguous_contaminated", VerdictClass.INCONCLUSIVE.value, "contaminated_environment", False),
]
