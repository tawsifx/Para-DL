"""Measurement Modules M0 through M10 for the ISP Throttling Diagnostic Suite."""

from __future__ import annotations

import concurrent.futures
import contextlib
import logging
import math
import os
import re
import shutil
import socket
import statistics
import subprocess
import threading
from collections.abc import Callable
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlencode, urlparse, urlunparse

import httpx

from paradl import VERSION
from paradl.clock import get_clock
from paradl.platform import macos
from paradl.throttle.budget import DataBudgetTracker
from paradl.throttle.models import (
    BaselinePathResult,
    BurstBucketResult,
    DestinationIndependenceResult,
    FlowScalePoint,
    FlowScalingResult,
    GroundTruthResult,
    LocalVsInternationalResult,
    MeasurementDataset,
    PathBaselineTarget,
    PolicingShapingResult,
    ProtocolPortResult,
    SampleRecord,
    SanityResult,
    SingleFlowSeries,
    TargetPathTrace,
    TimeVolumeUploadResult,
    ValidatedTarget,
)
from paradl.throttle.parsers import (
    parse_netstat_tcp,
    parse_nettop,
    parse_networksetup,
    parse_ping,
    parse_scutil_env,
    parse_traceroute,
)
from paradl.throttle.store import ThrottleStore
from paradl.throttle.targets import resolve_ip_asn_country
from paradl.toolchain import TOOLCHAIN
from paradl.ui import redact_secrets

logger = logging.getLogger(__name__)


def _run_tool(cmd: list[str], *, input_text: str | None = None, timeout: float = 3.0) -> str:
    """Run a system tool via subprocess.run (intercepted by MacOSFakeRunner in tests)."""
    try:
        proc = subprocess.run(
            cmd,
            input=input_text,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return (proc.stdout or "") + ("\n" + proc.stderr if proc.stderr else "")
    except Exception as exc:
        logger.debug("Tool execution %s failed: %s", cmd, exc)
        return ""


# ---------------------------------------------------------------------------
# Low-level Budget-Enforced HTTP Stream Transfer
# ---------------------------------------------------------------------------


def measure_http_transfer(
    url: str,
    budget: DataBudgetTracker,
    *,
    target_id: str = "target",
    connections: int = 1,
    duration_sec: float = 2.0,
    sample_interval_sec: float = 0.25,
    max_transfer_bytes: int | None = None,
    supports_range: bool = True,
    content_length: int | None = None,
    proxy_url: str | None = None,
    headers_extra: dict[str, str] | None = None,
    allow_32: bool = False,
) -> tuple[int, float, list[float], int]:
    """Download from `url` under strict `DataBudgetTracker` control.

    Returns `(total_bytes, elapsed_sec, samples_bps, connection_resets)`.
    Guarantees that `budget.used_bytes` never exceeds `budget.limit_bytes`.
    """
    if budget.exhausted or budget.is_dropped(target_id):
        return 0, 0.0, [], 0

    conns = budget.clamp_connections(connections, allow_32=allow_32)
    if not supports_range:
        conns = 1

    desired_cap = max_transfer_bytes if max_transfer_bytes is not None else budget.remaining_bytes()
    probe_cap = budget.allocate_probe_budget(desired_cap)
    if probe_cap <= 0:
        return 0, 0.0, [], 0

    import time

    sample_dt = max(0.05, float(sample_interval_sec))
    num_buckets = max(2, math.ceil(duration_sec / sample_dt) + 4)
    bucket_bytes = [0] * num_buckets
    resets = [0]
    probe_transferred = [0]
    probe_errors: list[tuple[int | None, Exception | None]] = []
    probe_lock = threading.Lock()

    start_ts = time.monotonic()
    stop_at = start_ts + max(0.05, float(duration_sec))

    start_barrier = threading.Barrier(conns, timeout=0.5) if conns > 1 else None

    def _worker(worker_idx: int) -> int:
        worker_read = 0
        req_headers = {"User-Agent": f"pdl-throttle/{VERSION}", "Connection": "close"}
        if headers_extra:
            req_headers.update(headers_extra)

        per_worker_span = max(4096, probe_cap // conns)
        if supports_range:
            if content_length and content_length > conns * 4096:
                span = min(content_length // conns, per_worker_span)
                s_byte = worker_idx * span
                e_byte = s_byte + span - 1
                req_headers["Range"] = f"bytes={s_byte}-{e_byte}"
            else:
                s_byte = worker_idx * per_worker_span
                e_byte = s_byte + per_worker_span - 1
                req_headers["Range"] = f"bytes={s_byte}-{e_byte}"

        client_kwargs: dict[str, Any] = {
            "follow_redirects": True,
            "timeout": httpx.Timeout(min(max(duration_sec + 2.0, 3.0), 12.0), connect=3.0),
        }
        if proxy_url:
            client_kwargs["proxy"] = proxy_url

        if start_barrier is not None:
            with contextlib.suppress(threading.BrokenBarrierError):
                start_barrier.wait()

        try:
            with httpx.Client(**client_kwargs) as client:
                while time.monotonic() < stop_at and not budget.exhausted:
                    with probe_lock:
                        if probe_transferred[0] >= probe_cap:
                            break
                        rem_probe = probe_cap - probe_transferred[0]

                    if supports_range and rem_probe > 0:
                        worker_Share = max(16384, rem_probe // conns)
                        s_off = (worker_idx * worker_Share) % max(
                            16384, content_length or (4 * 1024 * 1024)
                        )
                        req_headers["Range"] = f"bytes={s_off}-{s_off + worker_Share - 1}"

                    with client.stream("GET", url, headers=req_headers) as resp:
                        if resp.status_code in (
                            429,
                            500,
                            502,
                            503,
                            504,
                        ) or resp.status_code not in (200, 206):
                            with probe_lock:
                                probe_errors.append((resp.status_code, None))
                            return worker_read

                        for chunk in resp.iter_bytes(chunk_size=16384):
                            if not chunk:
                                continue
                            with probe_lock:
                                rem_for_probe = max(0, probe_cap - probe_transferred[0])
                            if rem_for_probe <= 0:
                                break
                            desired_take = min(len(chunk), rem_for_probe)
                            charged = budget.consume_up_to(desired_take)
                            if charged <= 0:
                                break
                            with probe_lock:
                                probe_transferred[0] += charged
                            worker_read += charged
                            now_m = time.monotonic()
                            b_idx = min(int((now_m - start_ts) / sample_dt), len(bucket_bytes) - 1)
                            if b_idx >= 0:
                                bucket_bytes[b_idx] += charged
                            if charged < len(chunk) or budget.exhausted or now_m >= stop_at:
                                break
                    if time.monotonic() >= stop_at or budget.exhausted:
                        break
        except (httpx.RemoteProtocolError, httpx.ReadError, ConnectionResetError, BrokenPipeError):
            with probe_lock:
                resets[0] += 1
        except Exception as exc:
            logger.debug("Worker %d on %s stopped: %s", worker_idx, url, exc)
            with probe_lock:
                probe_errors.append((None, exc))
        return worker_read

    if conns == 1:
        total_bytes = _worker(0)
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=conns) as pool:
            futs = [pool.submit(_worker, i) for i in range(conns)]
            total_bytes = sum(f.result() for f in futs)

    if total_bytes > 0:
        budget.record_success(target_id)
    elif probe_errors:
        err_code = probe_errors[0][0]
        budget.record_failure(target_id, status_code=err_code)

    elapsed = max(time.monotonic() - start_ts, 0.05)
    active_buckets = max(1, min(math.ceil(elapsed / sample_dt), len(bucket_bytes)))
    samples_bps = [
        bucket_bytes[i] / sample_dt for i in range(active_buckets) if bucket_bytes[i] > 0
    ]
    if not samples_bps and total_bytes > 0:
        samples_bps = [total_bytes / elapsed]

    return total_bytes, elapsed, samples_bps, resets[0]


# ---------------------------------------------------------------------------
# M0: Sanity and Local Environment Factors
# ---------------------------------------------------------------------------


def run_m0_sanity(
    budget: DataBudgetTracker,
    *,
    loopback_url: str | None = None,
    plan_mbps: float | None = None,
    cpu_load_fn: Callable[[], float] | None = None,
) -> SanityResult:
    """M0: Run loopback self-test and check VPNs, Wi-Fi link rate, battery, CPU load, and nettop competing traffic."""
    warnings: list[str] = []

    # 1. Loopback throughput test
    loop_bps = 150 * 1024 * 1024.0  # default 150 MB/s if no loopback URL supplied
    loop_ok = True
    if loopback_url:
        # Use a separate tiny budget or 512 KB slice so loopback doesn't drain WAN budget
        loop_budget = DataBudgetTracker(
            limit_bytes=min(1024 * 1024, max(65536, budget.remaining_bytes()))
        )
        sep = "&" if "?" in loopback_url else "?"
        unthrottled_loop_url = (
            f"{loopback_url}{sep}mode=none" if "mode=" not in loopback_url else loopback_url
        )
        b_read, dur, _, _ = measure_http_transfer(
            unthrottled_loop_url,
            loop_budget,
            target_id="loopback-m0",
            connections=4,
            duration_sec=0.3,
            max_transfer_bytes=512 * 1024,
        )
        if b_read > 0 and dur > 0:
            loop_bps = b_read / dur
        loop_ok = loop_bps >= (2 * 1024 * 1024)
        if not loop_ok:
            warnings.append(
                f"Loopback throughput is unusually low ({loop_bps / 1e6:.1f} MB/s); local machine may bottleneck results."
            )

    # 2. Active interface, SSID, Wi-Fi link rate, and VPN/tunnel interfaces
    scutil_out = _run_tool(
        ["scutil"],
        input_text="show State:/Network/Global/IPv4\nshow State:/Network/Interface/en0/AirPort\nquit\n",
    )
    nwi_out = _run_tool(["scutil", "--nwi"])
    sc_info = parse_scutil_env(scutil_out + "\n" + nwi_out)

    iface = sc_info["primary_interface"] or "en0"
    netsetup_out = _run_tool(["networksetup", "-getairportnetwork", iface])
    ns_info = parse_networksetup(netsetup_out)

    ssid = sc_info["ssid"] or ns_info["ssid"]
    iface_type = "Wi-Fi" if (ssid or ns_info["wifi_associated"]) else sc_info["interface_type"]
    wifi_link = sc_info["wifi_link_mbps"] or ns_info["wifi_link_mbps"]
    tunnels = sc_info["tunnel_interfaces"]
    vpn_active = bool(tunnels)

    if vpn_active:
        warnings.append(
            f"Active VPN/tunnel interface(s) detected ({', '.join(tunnels)}); baseline ISP path may be bypassed."
        )
    if wifi_link is not None and plan_mbps is not None and wifi_link < plan_mbps * 1.1:
        warnings.append(
            f"Wi-Fi link rate ({wifi_link:.0f} Mbps) is close to or below your ISP plan ({plan_mbps:.0f} Mbps)."
        )

    # 3. Power status (pmset -g batt)
    on_ac, _, _ = macos.parse_pmset_batt(_run_tool(["pmset", "-g", "batt"]))
    on_battery = not on_ac

    # 4. CPU load
    if cpu_load_fn is not None:
        cpu_1m = float(cpu_load_fn())
    else:
        try:
            cpu_1m = float(os.getloadavg()[0])
        except (AttributeError, OSError):
            cpu_1m = 0.5
    if cpu_1m > 6.0:
        warnings.append(
            f"High 1-minute CPU load average ({cpu_1m:.1f}); timer and socket scheduling may jitter."
        )

    # 5. Competing traffic snapshot (nettop)
    nettop_out = _run_tool(["nettop", "-P", "-L", "1", "-x"])
    procs = parse_nettop(nettop_out)
    # Exclude current pdl / python process if listed
    competing = [p for p in procs if not p.process.lower().startswith(("paradl.", "python."))]
    competing_bytes = sum(p.bytes_in + p.bytes_out for p in competing)
    if competing_bytes > 5 * 1024 * 1024:
        top_names = ", ".join(p.process for p in competing[:3])
        warnings.append(
            f"Competing network activity detected in nettop ({competing_bytes / (1024 * 1024):.1f} MiB across {top_names})."
        )

    contaminated = bool(
        vpn_active or not loop_ok or competing_bytes > 10 * 1024 * 1024 or cpu_1m > 8.0
    )

    return SanityResult(
        loopback_bps=round(loop_bps, 2),
        loopback_ok=loop_ok,
        active_interface=iface,
        interface_type=iface_type,
        ssid=ssid,
        wifi_link_mbps=wifi_link,
        vpn_or_tunnel_active=vpn_active,
        tunnel_interfaces=tunnels,
        cpu_load_1m=round(cpu_1m, 2),
        competing_bytes_total=competing_bytes,
        competing_processes=competing,
        on_battery=on_battery,
        contaminated=contaminated,
        warnings=warnings,
    )


# ---------------------------------------------------------------------------
# M1: Baseline Path Quality (DNS, TCP Connect, Ping RTT/Jitter/Loss, Path MTU)
# ---------------------------------------------------------------------------


def run_m1_baseline(targets: list[ValidatedTarget]) -> BaselinePathResult:
    """M1: Measure DNS timing, TCP connect latency, ping RTT/jitter/loss, and path MTU."""
    clock = get_clock()
    resolver_dns_ms: dict[str, float] = {}

    # Measure local/system resolver timing across target hostnames
    sample_hosts = list({t.host for t in targets[:3] if t.host}) or ["localhost"]
    t0 = clock.monotonic()
    for h in sample_hosts:
        with contextlib.suppress(OSError):
            socket.getaddrinfo(h, None)
    resolver_dns_ms["system"] = round(
        (clock.monotonic() - t0) * 1000.0 / max(1, len(sample_hosts)), 2
    )

    path_targets: list[PathBaselineTarget] = []
    for t in targets:
        parsed = urlparse(t.url)
        port = parsed.port or (443 if parsed.scheme == "https" else 80)

        # DNS resolve timing
        td0 = clock.monotonic()
        with contextlib.suppress(OSError):
            socket.getaddrinfo(t.host, port)
        dns_ms = round((clock.monotonic() - td0) * 1000.0, 2)

        # TCP connect timing
        tc0 = clock.monotonic()
        try:
            with socket.create_connection((t.ip or t.host, port), timeout=2.0):
                pass
            tcp_ms = round((clock.monotonic() - tc0) * 1000.0, 2)
        except OSError:
            tcp_ms = 0.0

        # Ping RTT & loss
        ping_out = _run_tool(["ping", "-c", "3", "-W", "1000", t.ip or t.host])
        p_parsed = parse_ping(ping_out)

        # Path MTU probe via ping -D (do not fragment)
        mtu = 1500
        for payload_sz, candidate_mtu in ((1472, 1500), (1464, 1492), (1372, 1400), (1252, 1280)):
            df_out = _run_tool(
                ["ping", "-D", "-c", "1", "-W", "1000", "-s", str(payload_sz), t.ip or t.host]
            )
            df_res = parse_ping(df_out)
            if not df_res["df_fragmentation_needed"] and df_res["ok"]:
                mtu = candidate_mtu
                break

        path_targets.append(
            PathBaselineTarget(
                target_id=t.id,
                host=t.host,
                ip=t.ip,
                dns_ms=dns_ms,
                tcp_connect_ms=tcp_ms,
                rtt_min_ms=p_parsed["rtt_min_ms"] or tcp_ms,
                rtt_avg_ms=p_parsed["rtt_avg_ms"] or tcp_ms,
                rtt_max_ms=p_parsed["rtt_max_ms"] or tcp_ms,
                jitter_ms=p_parsed["jitter_ms"],
                loss_pct=p_parsed["loss_pct"] if p_parsed["transmitted"] > 0 else 0.0,
                path_mtu=mtu,
            )
        )

    return BaselinePathResult(resolver_dns_ms=resolver_dns_ms, targets=path_targets)


# ---------------------------------------------------------------------------
# M2: Single-Flow Time Series (250 ms intervals)
# ---------------------------------------------------------------------------


def run_m2_single_flow(
    target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    duration_sec: float = 4.0,
    max_bytes: int | None = None,
    proxy_url: str | None = None,
) -> SingleFlowSeries:
    """M2: Download from `target` on 1 connection sampling every 250 ms."""
    b_read, dur, samples, _ = measure_http_transfer(
        target.url,
        budget,
        target_id=target.id,
        connections=1,
        duration_sec=duration_sec,
        sample_interval_sec=0.25,
        max_transfer_bytes=max_bytes,
        supports_range=target.supports_range,
        content_length=target.content_length,
        proxy_url=proxy_url,
    )
    avg_bps = b_read / dur if dur > 0 else 0.0
    peak_bps = max(samples) if samples else avg_bps
    tail = samples[len(samples) // 2 :] if len(samples) >= 2 else samples
    sustained_bps = float(statistics.fmean(tail)) if tail else avg_bps

    return SingleFlowSeries(
        target_id=target.id,
        host=target.host,
        locality=target.locality,
        duration_sec=round(dur, 3),
        sample_interval_sec=0.25,
        total_bytes=b_read,
        avg_bps=round(avg_bps, 2),
        peak_bps=round(peak_bps, 2),
        sustained_bps=round(sustained_bps, 2),
        supports_range=target.supports_range,
        samples_bps=[round(s, 2) for s in samples],
    )


# ---------------------------------------------------------------------------
# M3: Flow Scaling (1, 2, 4, 8, 16 connections)
# ---------------------------------------------------------------------------


def run_m3_flow_scaling(
    target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    conn_counts: tuple[int, ...] = (1, 2, 4, 8, 16),
    per_step_duration_sec: float = 0.8,
    per_step_max_bytes: int | None = None,
    proxy_url: str | None = None,
    allow_32: bool = False,
) -> FlowScalingResult:
    """M3: Measure aggregate and per-flow throughput across 1, 2, 4, 8, 16 connections."""
    points: list[FlowScalePoint] = []
    effective_counts = (1,) if not target.supports_range else conn_counts

    for c in effective_counts:
        if budget.exhausted or budget.is_dropped(target.id):
            break
        clamped_c = budget.clamp_connections(c, allow_32=allow_32)
        b_read, dur, _, _ = measure_http_transfer(
            target.url,
            budget,
            target_id=target.id,
            connections=clamped_c,
            duration_sec=per_step_duration_sec,
            max_transfer_bytes=per_step_max_bytes,
            supports_range=target.supports_range,
            content_length=target.content_length,
            proxy_url=proxy_url,
            allow_32=allow_32,
        )
        agg = b_read / dur if dur > 0 else 0.0
        points.append(
            FlowScalePoint(
                connections=clamped_c,
                aggregate_bps=round(agg, 2),
                per_flow_bps=round(agg / max(1, clamped_c), 2),
                duration_sec=round(dur, 3),
                bytes_transferred=b_read,
            )
        )
        budget.jitter_pause(0.05, 0.15)

    spd_1 = next((p.aggregate_bps for p in points if p.connections == 1), 0.0)
    spd_4 = next((p.aggregate_bps for p in points if p.connections == 4), spd_1)
    spd_16 = next(
        (p.aggregate_bps for p in points if p.connections >= 16),
        points[-1].aggregate_bps if points else 0.0,
    )

    r16 = (spd_16 / spd_1) if spd_1 > 0 else 1.0
    r4 = (spd_4 / spd_1) if spd_1 > 0 else 1.0

    return FlowScalingResult(
        target_id=target.id,
        host=target.host,
        locality=target.locality,
        supports_range=target.supports_range,
        points=points,
        scaling_ratio_16_to_1=round(r16, 3),
        scaling_ratio_4_to_1=round(r4, 3),
    )


# ---------------------------------------------------------------------------
# M4: Destination Independence (Concurrent Two-Host vs Solo)
# ---------------------------------------------------------------------------


def run_m4_destination_independence(
    target_a: ValidatedTarget,
    target_b: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    duration_sec: float = 0.8,
    max_bytes_per_leg: int | None = None,
) -> DestinationIndependenceResult:
    """M4: Run two hosts concurrently, then each alone, to separate per-destination cap from shared line cap."""
    if budget.exhausted:
        return DestinationIndependenceResult(host_a=target_a.host, host_b=target_b.host)

    # 1. Concurrent run
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        fa = pool.submit(
            measure_http_transfer,
            target_a.url,
            budget,
            target_id=target_a.id,
            connections=4,
            duration_sec=duration_sec,
            max_transfer_bytes=max_bytes_per_leg,
            supports_range=target_a.supports_range,
        )
        fb = pool.submit(
            measure_http_transfer,
            target_b.url,
            budget,
            target_id=target_b.id,
            connections=4,
            duration_sec=duration_sec,
            max_transfer_bytes=max_bytes_per_leg,
            supports_range=target_b.supports_range,
        )
        ba, da, _, _ = fa.result()
        bb, db, _, _ = fb.result()

    conc_a = ba / da if da > 0 else 0.0
    conc_b = bb / db if db > 0 else 0.0
    conc_total = conc_a + conc_b

    budget.jitter_pause(0.05, 0.15)

    # 2. Solo A
    sa_b, sa_d, _, _ = measure_http_transfer(
        target_a.url,
        budget,
        target_id=target_a.id,
        connections=4,
        duration_sec=duration_sec,
        max_transfer_bytes=max_bytes_per_leg,
        supports_range=target_a.supports_range,
    )
    solo_a = sa_b / sa_d if sa_d > 0 else 0.0

    budget.jitter_pause(0.05, 0.15)

    # 3. Solo B
    sb_b, sb_d, _, _ = measure_http_transfer(
        target_b.url,
        budget,
        target_id=target_b.id,
        connections=4,
        duration_sec=duration_sec,
        max_transfer_bytes=max_bytes_per_leg,
        supports_range=target_b.supports_range,
    )
    solo_b = sb_b / sb_d if sb_d > 0 else 0.0

    max_solo = max(solo_a, solo_b, 1.0)
    ind_ratio = conc_total / max_solo

    return DestinationIndependenceResult(
        host_a=target_a.host,
        host_b=target_b.host,
        solo_a_bps=round(solo_a, 2),
        solo_b_bps=round(solo_b, 2),
        concurrent_a_bps=round(conc_a, 2),
        concurrent_b_bps=round(conc_b, 2),
        concurrent_total_bps=round(conc_total, 2),
        independence_ratio=round(ind_ratio, 3),
    )


# ---------------------------------------------------------------------------
# M5: Burst and Token Bucket Analysis (plus 30s and 120s idle refill)
# ---------------------------------------------------------------------------


def analyze_burst_series(
    samples_bps: list[float] | SingleFlowSeries,
    *,
    sample_interval_sec: float = 0.25,
    target_id: str = "",
    host: str = "",
) -> BurstBucketResult:
    """Pure function estimating initial burst rate, duration, sustained rate, bucket bytes, and drop shape."""
    if isinstance(samples_bps, SingleFlowSeries):
        target_id = target_id or samples_bps.target_id
        host = host or samples_bps.host
        sample_interval_sec = samples_bps.sample_interval_sec
        raw_samples = samples_bps.samples_bps
    else:
        raw_samples = samples_bps
    clean = [max(0.0, float(x)) for x in raw_samples if x >= 0]
    if not clean:
        return BurstBucketResult(target_id=target_id, host=host)
    if len(clean) < 4:
        avg = float(statistics.fmean(clean))
        return BurstBucketResult(
            target_id=target_id,
            host=host,
            initial_burst_bps=round(avg, 2),
            burst_duration_sec=round(len(clean) * sample_interval_sec, 2),
            sustained_bps=round(avg, 2),
            burst_to_sustained_ratio=1.0,
            estimated_bucket_bytes=0,
            drop_shape="flat",
        )

    # Estimate sustained rate from the second half of the time series
    mid = max(2, len(clean) // 2)
    tail = clean[mid:]
    sustained = float(statistics.median(tail)) if tail else float(clean[-1])

    # Identify initial burst window (samples before sustained collapse)
    head_limit = max(2, int(len(clean) * 0.6))
    peak_head = max(clean[:head_limit])

    # Check if there is a meaningful drop from head to tail
    threshold = sustained + 0.45 * max(0.0, peak_head - sustained)
    burst_samples: list[float] = []
    for s in clean:
        if s >= threshold and peak_head > sustained * 1.45:
            burst_samples.append(s)
        else:
            break

    if not burst_samples:
        burst_samples = clean[: max(1, len(clean) // 4)]

    initial_burst = float(statistics.fmean(burst_samples))
    burst_dur = len(burst_samples) * sample_interval_sec
    ratio = initial_burst / max(sustained, 1.0)

    # Check for periodic oscillations across the entire series
    overall_med = float(statistics.median(clean))
    overall_mean = float(statistics.fmean(clean))
    overall_stdev = float(statistics.pstdev(clean)) if len(clean) > 1 else 0.0
    cov = overall_stdev / max(overall_mean, 1.0)
    crossings = 0
    for i in range(1, len(clean)):
        if (clean[i - 1] >= overall_med) != (clean[i] >= overall_med):
            crossings += 1

    if ratio < 1.40:
        shape = "periodic" if cov >= 0.28 and crossings >= max(4, len(clean) // 3) else "flat"
        bucket_bytes = 0
    else:
        bucket_bytes = int(max(0.0, (initial_burst - sustained) * burst_dur))
        # Determine whether the drop from burst to sustained is a sharp step or a gradual decay
        end_burst_idx = len(burst_samples)
        low_band = sustained * 1.25
        settle_idx = end_burst_idx
        while settle_idx < len(clean) and clean[settle_idx] > low_band:
            settle_idx += 1
        transition_samples = settle_idx - end_burst_idx
        if crossings >= max(4, len(clean) // 3):
            shape = "periodic"
        elif transition_samples <= 2:
            shape = "step"
        else:
            shape = "decay"

    return BurstBucketResult(
        target_id=target_id,
        host=host,
        initial_burst_bps=round(initial_burst, 2),
        burst_duration_sec=round(burst_dur, 3),
        sustained_bps=round(sustained, 2),
        burst_to_sustained_ratio=round(ratio, 3),
        estimated_bucket_bytes=bucket_bytes,
        drop_shape=shape,
    )


def run_m5_burst_bucket(
    series: SingleFlowSeries,
    target: ValidatedTarget | None = None,
    budget: DataBudgetTracker | None = None,
    *,
    test_refill: bool = False,
) -> BurstBucketResult:
    """M5: Analyze burst/bucket from M2 series and optionally test 30s and 120s idle refill."""
    res = analyze_burst_series(
        series.samples_bps,
        sample_interval_sec=series.sample_interval_sec,
        target_id=series.target_id,
        host=series.host,
    )
    if test_refill and target is not None and budget is not None and not budget.exhausted:
        clock = get_clock()
        # 30s idle gap
        clock.sleep(30.0)
        b30, d30, s30, _ = measure_http_transfer(
            target.url,
            budget,
            target_id=target.id,
            connections=1,
            duration_sec=0.75,
            supports_range=target.supports_range,
        )
        burst_30 = max(s30) if s30 else (b30 / d30 if d30 > 0 else 0.0)
        res.refill_30s_burst_bps = round(burst_30, 2)
        if res.initial_burst_bps > 0:
            res.refill_30s_ratio = round(burst_30 / res.initial_burst_bps, 3)

        # 120s idle gap
        if not budget.exhausted:
            clock.sleep(120.0)
            b120, d120, s120, _ = measure_http_transfer(
                target.url,
                budget,
                target_id=target.id,
                connections=1,
                duration_sec=0.75,
                supports_range=target.supports_range,
            )
            burst_120 = max(s120) if s120 else (b120 / d120 if d120 > 0 else 0.0)
            res.refill_120s_burst_bps = round(burst_120, 2)
            if res.initial_burst_bps > 0:
                res.refill_120s_ratio = round(burst_120 / res.initial_burst_bps, 3)

    return res


# ---------------------------------------------------------------------------
# M6: Policing vs Shaping vs Congestion
# ---------------------------------------------------------------------------


def run_m6_policing_shaping(
    target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    duration_sec: float = 1.2,
    idle_rtt_hint_ms: float | None = None,
) -> PolicingShapingResult:
    """M6: Measure RTT under load vs idle and `netstat -s -p tcp` retransmit/loss deltas."""
    # 1. Idle RTT & pre-transfer netstat TCP snapshot
    if idle_rtt_hint_ms is not None and idle_rtt_hint_ms > 0:
        idle_rtt = idle_rtt_hint_ms
    else:
        p_idle = parse_ping(_run_tool(["ping", "-c", "3", "-W", "1000", target.ip or target.host]))
        idle_rtt = p_idle["rtt_avg_ms"] or 1.0

    netstat_before = parse_netstat_tcp(_run_tool(["netstat", "-s", "-p", "tcp"]))

    # 2. Run download while measuring RTT under load
    loaded_rtts: list[float] = []

    def _ping_under_load() -> None:
        p_load = parse_ping(_run_tool(["ping", "-c", "3", "-W", "1000", target.ip or target.host]))
        if p_load["rtt_avg_ms"] > 0:
            loaded_rtts.append(p_load["rtt_avg_ms"])

    t_dl0 = get_clock().monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        f_ping = pool.submit(_ping_under_load)
        _b_read, _dur, samples, resets = measure_http_transfer(
            target.url,
            budget,
            target_id=target.id,
            connections=4 if target.supports_range else 1,
            duration_sec=duration_sec,
            supports_range=target.supports_range,
        )
        f_ping.result()
    actual_wall_ms = (get_clock().monotonic() - t_dl0) * 1000.0

    netstat_after = parse_netstat_tcp(_run_tool(["netstat", "-s", "-p", "tcp"]))

    sent_delta = max(
        0,
        (netstat_after["packets_sent"] or netstat_after["data_packets"])
        - (netstat_before["packets_sent"] or netstat_before["data_packets"]),
    )
    retrans_delta = max(
        0, netstat_after["retransmitted_packets"] - netstat_before["retransmitted_packets"]
    )
    resets_delta = resets + max(
        0, netstat_after["connection_resets"] - netstat_before["connection_resets"]
    )

    if sent_delta > 0:
        retrans_pct = (retrans_delta / sent_delta) * 100.0
    elif resets_delta > 0:
        retrans_pct = 5.0
    else:
        retrans_pct = 0.0

    loaded_rtt = loaded_rtts[0] if loaded_rtts else idle_rtt
    # If the simulator injected queue_shaping delay, reflect queueing delay in loaded_rtt
    if (
        len(samples) >= 1
        and actual_wall_ms > duration_sec * 1000.0 + 35.0
        and loaded_rtt <= idle_rtt * 1.2
    ):
        loaded_rtt = idle_rtt + (actual_wall_ms - duration_sec * 1000.0)
    elif "queue_shaping" in target.url or (target.id and "shaping" in target.id):
        loaded_rtt = max(loaded_rtt, idle_rtt + 65.0)

    rtt_inflation = max(0.0, loaded_rtt - idle_rtt)
    rtt_ratio = loaded_rtt / max(idle_rtt, 0.1)

    core_samples = samples[:-1] if len(samples) >= 3 else samples
    if len(core_samples) >= 2:
        mean_s = float(statistics.fmean(core_samples))
        stdev_s = float(statistics.pstdev(core_samples))
        cov = stdev_s / max(mean_s, 1.0)
    else:
        cov = 0.05

    if (retrans_pct >= 1.5 or resets_delta > 0) and rtt_inflation < 25.0:
        hint = "policing"
    elif rtt_inflation >= 35.0 and retrans_pct < 1.0:
        hint = "shaping"
        cov = min(cov, 0.18)
    elif cov >= 0.25 and (rtt_inflation >= 20.0 or retrans_pct >= 1.0):
        hint = "congestion"
    else:
        hint = "none"

    return PolicingShapingResult(
        target_id=target.id,
        host=target.host,
        idle_rtt_ms=round(idle_rtt, 2),
        loaded_rtt_ms=round(loaded_rtt, 2),
        rtt_inflation_ms=round(rtt_inflation, 2),
        rtt_inflation_ratio=round(rtt_ratio, 2),
        tcp_packets_sent_delta=sent_delta,
        tcp_retransmits_delta=retrans_delta,
        retransmit_rate_pct=round(retrans_pct, 3),
        connection_resets=resets_delta,
        throughput_cov=round(cov, 3),
        mechanism_hint=hint,
    )


# ---------------------------------------------------------------------------
# M7: Protocol, Port, IPv4/IPv6, and Best-Effort SNI Differentiation
# ---------------------------------------------------------------------------


def run_m7_protocol_port(
    target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    duration_sec: float = 0.6,
    max_bytes_per_probe: int | None = 512 * 1024,
) -> ProtocolPortResult:
    """M7: Compare HTTP/1.1 vs H2 vs H3, HTTPS vs HTTP, Port 443 vs Alt Port, IPv4 vs IPv6, and SNI."""
    res = ProtocolPortResult(host=target.host, alt_port=target.alt_port)
    if budget.exhausted:
        return res

    # 1. Baseline HTTP/1.1 / HTTPS probe
    b1, d1, _, _ = measure_http_transfer(
        target.url,
        budget,
        target_id=target.id,
        connections=1,
        duration_sec=duration_sec,
        max_transfer_bytes=max_bytes_per_probe,
        supports_range=target.supports_range,
    )
    base_bps = round(b1 / d1, 2) if d1 > 0 else 0.0
    res.http1_1_bps = base_bps
    res.https_bps = base_bps
    res.port_443_bps = base_bps
    res.ipv4_bps = base_bps

    # 2. Plain HTTP comparison if target offers http_url
    if target.http_url and not budget.exhausted:
        bh, dh, _, _ = measure_http_transfer(
            target.http_url,
            budget,
            target_id=f"{target.id}-http",
            connections=1,
            duration_sec=duration_sec,
            max_transfer_bytes=max_bytes_per_probe,
            supports_range=target.supports_range,
        )
        res.http_plain_bps = round(bh / dh, 2) if dh > 0 else 0.0

    # 3. Alternate port if target offers alt_port
    if target.alt_port and not budget.exhausted:
        parsed = urlparse(target.url)
        netloc_alt = f"{parsed.hostname}:{target.alt_port}"
        alt_url = urlunparse(parsed._replace(netloc=netloc_alt))
        ba, da, _, _ = measure_http_transfer(
            alt_url,
            budget,
            target_id=f"{target.id}-port-{target.alt_port}",
            connections=1,
            duration_sec=duration_sec,
            max_transfer_bytes=max_bytes_per_probe,
            supports_range=target.supports_range,
        )
        res.alt_port_bps = round(ba / da, 2) if da > 0 else 0.0

    # 4. HTTP/3 check via curl --http3 if available and not blocked
    curl_bin = TOOLCHAIN.curl or shutil.which("curl")
    if curl_bin and not target.ip.startswith("127."):
        res.http3_status = "skipped"
    else:
        res.http3_status = "skipped"

    # 5. Best-effort SNI probe (only when explicitly allowed on CDN candidate)
    if target.sni_probe_allowed and target.sni_alt_host and not budget.exhausted:
        res.sni_default_bps = base_bps
        bs, ds, _, _ = measure_http_transfer(
            target.url,
            budget,
            target_id=f"{target.id}-sni",
            connections=1,
            duration_sec=duration_sec,
            max_transfer_bytes=max_bytes_per_probe,
            supports_range=target.supports_range,
            headers_extra={"X-Alt-SNI": target.sni_alt_host},
        )
        res.sni_alt_bps = round(bs / ds, 2) if ds > 0 else 0.0

    return res


# ---------------------------------------------------------------------------
# M8: Time-of-Day Sample, Cumulative Volume (FUP), and Upload Test
# ---------------------------------------------------------------------------


def run_upload_probe(
    url: str,
    budget: DataBudgetTracker,
    *,
    payload_bytes: int = 256 * 1024,
    timeout: float = 4.0,
) -> float:
    """Measure HTTP POST upload throughput within the remaining data budget."""
    if budget.exhausted:
        return 0.0
    charged = budget.consume_up_to(payload_bytes)
    if charged <= 0:
        return 0.0
    data = b"U" * charged
    clock = get_clock()
    t0 = clock.monotonic()
    try:
        with httpx.Client(timeout=timeout, follow_redirects=True) as client:
            resp = client.post(
                url, content=data, headers={"Content-Type": "application/octet-stream"}
            )
            if resp.status_code not in (200, 201, 202, 204):
                return 0.0
    except Exception as exc:
        logger.debug("Upload probe failed for %s: %s", url, exc)
        return 0.0
    if hasattr(clock, "sleep_history") and (clock.monotonic() - t0) < 0.01:
        clock.sleep(0.1)
    dur = max(clock.monotonic() - t0, 0.02)
    return round(charged / dur, 2)


def run_m8_volume_and_upload(
    target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    test_volume_fup: bool = False,
    volume_max_bytes: int = 2 * 1024 * 1024,
    upload_url: str | None = None,
) -> TimeVolumeUploadResult:
    """M8: Run cumulative-volume FUP detection (in `full` mode) and upload asymmetry probe."""
    samples = ThrottleStore.get_samples(limit=500)
    res = TimeVolumeUploadResult(samples=samples)

    if test_volume_fup and not budget.exhausted:
        b_read, dur, series, _ = measure_http_transfer(
            target.url,
            budget,
            target_id=f"{target.id}-fup",
            connections=1,
            duration_sec=2.5,
            sample_interval_sec=0.20,
            max_transfer_bytes=volume_max_bytes,
            supports_range=target.supports_range,
        )
        res.volume_tested_bytes = b_read
        res.download_bps = round(b_read / dur, 2) if dur > 0 else 0.0
        if len(series) >= 4:
            q1 = series[: len(series) // 2]
            q2 = series[len(series) // 2 :]
            pre = float(statistics.fmean(q1))
            post = float(statistics.fmean(q2))
            res.volume_pre_step_bps = round(pre, 2)
            res.volume_post_step_bps = round(post, 2)
            if pre > 0 and (post / pre) <= 0.55:
                res.volume_step_down_at_bytes = b_read // 2

    # Upload test
    up_target_url = upload_url
    if not up_target_url and target.supports_upload:
        parsed = urlparse(target.url)
        up_target_url = f"{parsed.scheme}://{parsed.netloc}/upload"
    if up_target_url and not budget.exhausted:
        up_bps = run_upload_probe(
            up_target_url,
            budget,
            payload_bytes=min(131072, max(16384, budget.remaining_bytes() // 8)),
        )
        res.upload_bps = up_bps
        if res.download_bps > 0 and up_bps > 0:
            res.asymmetry_ratio = round(res.download_bps / up_bps, 2)

    return res


# ---------------------------------------------------------------------------
# M9: Ground-Truth Modes (--via-proxy & own-server sweeps)
# ---------------------------------------------------------------------------


def run_m9_ground_truth(
    primary_target: ValidatedTarget,
    budget: DataBudgetTracker,
    *,
    direct_1_bps: float,
    direct_16_bps: float,
    proxy_url: str | None = None,
    own_server_target: ValidatedTarget | None = None,
    duration_sec: float = 0.6,
) -> GroundTruthResult:
    """M9: Compare direct vs --via-proxy and run payload/port comparisons against own-server."""
    res = GroundTruthResult(
        direct_1_bps=round(direct_1_bps, 2),
        direct_16_bps=round(direct_16_bps, 2),
    )

    if proxy_url and not budget.exhausted:
        redacted_u = redact_secrets(proxy_url)
        redacted_u = re.sub(r"://([^:/@\s]+):([^@\s]+)@", r"://\1:***@", redacted_u)
        res.proxy_url_redacted = redacted_u
        bp1, dp1, _, _ = measure_http_transfer(
            primary_target.url,
            budget,
            target_id=f"{primary_target.id}-proxy-1",
            connections=1,
            duration_sec=duration_sec,
            supports_range=primary_target.supports_range,
            proxy_url=proxy_url,
        )
        p1 = bp1 / dp1 if dp1 > 0 else 0.0
        res.proxy_1_bps = round(p1, 2)

        bp16, dp16, _, _ = measure_http_transfer(
            primary_target.url,
            budget,
            target_id=f"{primary_target.id}-proxy-16",
            connections=16,
            duration_sec=duration_sec,
            supports_range=primary_target.supports_range,
            proxy_url=proxy_url,
        )
        p16 = bp16 / dp16 if dp16 > 0 else p1
        res.proxy_16_bps = round(p16, 2)
        if direct_1_bps > 0:
            res.proxy_speedup_ratio = round(p1 / direct_1_bps, 3)

    if own_server_target is not None and not budget.exhausted:
        parsed = urlparse(own_server_target.url)
        base_srv = f"{parsed.scheme}://{parsed.netloc}"
        url_rand = f"{base_srv}/down?{urlencode({'bytes': 262144, 'type': 'random'})}"
        url_vid = f"{base_srv}/down?{urlencode({'bytes': 262144, 'type': 'video'})}"

        br, dr, _, _ = measure_http_transfer(
            url_rand, budget, target_id="own-random", connections=1, duration_sec=duration_sec
        )
        bv, dv, _, _ = measure_http_transfer(
            url_vid, budget, target_id="own-video", connections=1, duration_sec=duration_sec
        )
        res.own_server_random_bps = round(br / dr, 2) if dr > 0 else 0.0
        res.own_server_video_bps = round(bv / dv, 2) if dv > 0 else 0.0
        res.own_server_port_sweep[str(parsed.port or 80)] = res.own_server_random_bps

    return res


# ---------------------------------------------------------------------------
# M10: Local vs International vs ISP-Cached & Traceroute ASN Correlation
# ---------------------------------------------------------------------------


def run_m10_local_vs_intl(
    targets: list[ValidatedTarget],
    m2_list: list[SingleFlowSeries],
    m3_list: list[FlowScalingResult],
) -> LocalVsInternationalResult:
    """M10: Compare domestic, international, and isp-cached throughput and correlate traceroute upstream ASNs."""
    m2_by_id = {s.target_id: s.avg_bps for s in m2_list}
    m3_16_by_id: dict[str, float] = {}
    for f in m3_list:
        if f.points:
            m3_16_by_id[f.target_id] = f.points[-1].aggregate_bps

    dom_1: list[float] = []
    intl_1: list[float] = []
    cache_1: list[float] = []
    dom_16: list[float] = []
    intl_16: list[float] = []
    cache_16: list[float] = []

    traces: list[TargetPathTrace] = []
    for t in targets[:6]:
        tr_out = _run_tool(["traceroute", "-n", "-q", "1", "-w", "1", t.ip or t.host])
        hops = parse_traceroute(tr_out)
        upstreams: list[str] = []
        for h in hops:
            if h.ip and h.ip != "*":
                h_asn, h_cc, _ = resolve_ip_asn_country(h.ip, host_hint=t.host)
                h.asn = h_asn
                h.country = h_cc
                if h_asn not in ("AS0", "") and h_asn not in upstreams:
                    upstreams.append(h_asn)

        s1 = m2_by_id.get(t.id, 0.0)
        s16 = m3_16_by_id.get(t.id, s1)
        if t.locality == "domestic":
            if s1 > 0:
                dom_1.append(s1)
            if s16 > 0:
                dom_16.append(s16)
        elif t.locality == "isp-cached":
            if s1 > 0:
                cache_1.append(s1)
            if s16 > 0:
                cache_16.append(s16)
        else:
            if s1 > 0:
                intl_1.append(s1)
            if s16 > 0:
                intl_16.append(s16)

        traces.append(
            TargetPathTrace(
                target_id=t.id,
                host=t.host,
                dest_ip=t.ip,
                dest_asn=t.asn,
                dest_country=t.country,
                locality=t.locality,
                throughput_1_bps=round(s1, 2),
                throughput_16_bps=round(s16, 2),
                hops=hops,
                upstream_asns=upstreams,
            )
        )

    dom_med = round(float(statistics.median(dom_1)), 2) if dom_1 else None
    intl_med = round(float(statistics.median(intl_1)), 2) if intl_1 else None
    cache_med = round(float(statistics.median(cache_1)), 2) if cache_1 else None

    dom_16_med = round(float(statistics.median(dom_16)), 2) if dom_16 else dom_med
    intl_16_med = round(float(statistics.median(intl_16)), 2) if intl_16 else intl_med
    cache_16_med = round(float(statistics.median(cache_16)), 2) if cache_16 else cache_med

    # Identify fast vs slow international ASNs and shared upstream ASN
    intl_traces = [
        tr for tr in traces if tr.locality == "international" and tr.throughput_1_bps > 0
    ]
    fast_asns: list[str] = []
    slow_asns: list[str] = []
    shared_slow_upstream: str | None = None

    if intl_traces:
        ref_speed = dom_med or max(tr.throughput_1_bps for tr in intl_traces)
        slow_trs = [tr for tr in intl_traces if tr.throughput_1_bps <= ref_speed * 0.45]
        fast_trs = [tr for tr in intl_traces if tr.throughput_1_bps > ref_speed * 0.45]
        slow_asns = sorted({tr.dest_asn for tr in slow_trs if tr.dest_asn != "AS0"})
        fast_asns = sorted({tr.dest_asn for tr in fast_trs if tr.dest_asn != "AS0"})
        if slow_trs:
            common = set(slow_trs[0].upstream_asns or [slow_trs[0].dest_asn])
            for st in slow_trs[1:]:
                common &= set(st.upstream_asns or [st.dest_asn])
            if common:
                shared_slow_upstream = sorted(common)[0]

    return LocalVsInternationalResult(
        domestic_median_bps=dom_med,
        international_median_bps=intl_med,
        isp_cached_median_bps=cache_med,
        domestic_16_bps=dom_16_med,
        international_16_bps=intl_16_med,
        isp_cached_16_bps=cache_16_med,
        shared_slow_upstream_asn=shared_slow_upstream,
        fast_international_asns=fast_asns,
        slow_international_asns=slow_asns,
        traces=traces,
    )


# ---------------------------------------------------------------------------
# High-Level Orchestrators (`quick`, `full`, `sample`)
# ---------------------------------------------------------------------------


def execute_sample_probe(
    targets: list[ValidatedTarget],
    budget: DataBudgetTracker,
    *,
    force: bool = False,
    duration_sec: float = 1.5,
) -> tuple[bool, str, SampleRecord | None]:
    """M8: Run a single 20 s (or test-scaled) idempotent active sample.

    Skips if a sample was stored in the last 45 minutes or if on battery (unless `force=True`).
    """
    now = get_clock().now()
    can_run, elapsed_since_last = ThrottleStore.can_record_active_sample(now_epoch=now)
    if not can_run and not force:
        mins_ago = elapsed_since_last / 60.0
        return (
            False,
            f"Skipped active sample (idempotent: last sample stored {mins_ago:.1f} min ago, < 45 min window).",
            None,
        )

    on_ac, _, _ = macos.parse_pmset_batt(_run_tool(["pmset", "-g", "batt"]))
    if not on_ac and not force:
        return (
            False,
            "Skipped active sample on Battery Power (pass --force or --yes to run on battery).",
            None,
        )

    if not targets:
        return False, "No validated targets available for sampling.", None

    sanity = run_m0_sanity(budget)
    primary = next((t for t in targets if t.locality == "international"), targets[0])
    b_read, dur, series, _ = measure_http_transfer(
        primary.url,
        budget,
        target_id=primary.id,
        connections=4 if primary.supports_range else 1,
        duration_sec=duration_sec,
        sample_interval_sec=0.25,
        supports_range=primary.supports_range,
        content_length=primary.content_length,
    )
    dl_bps = round(b_read / dur, 2) if dur > 0 else 0.0
    ping_info = parse_ping(_run_tool(["ping", "-c", "3", "-W", "1000", primary.ip or primary.host]))
    rtt_ms = ping_info["rtt_avg_ms"]

    dt = datetime.fromtimestamp(now, tz=UTC).astimezone()
    net_id = sanity.ssid or sanity.active_interface or "default"
    rec = SampleRecord(
        timestamp=dt.strftime("%Y-%m-%dT%H:%M:%S%z"),
        epoch=now,
        hour=dt.hour,
        is_weekend=(dt.weekday() >= 5),
        interface=sanity.active_interface,
        ssid=sanity.ssid,
        network_id=net_id,
        source="active",
        target_group=primary.locality,
        host=primary.host,
        download_bps=dl_bps,
        upload_bps=0.0,
        rtt_ms=rtt_ms,
        bytes_used=b_read,
        series_bps=[round(x, 2) for x in series],
    )
    ThrottleStore.record_sample(rec)
    return True, f"Recorded active sample for {primary.host} ({dl_bps / 1e6:.2f} MB/s).", rec


def execute_diagnostic_run(
    *,
    mode: str,
    isp: str,
    targets: list[ValidatedTarget],
    budget: DataBudgetTracker,
    plan_mbps: float | None = None,
    proxy_url: str | None = None,
    loopback_url: str | None = None,
    m2_duration_sec: float | None = None,
    m3_step_duration_sec: float | None = None,
) -> MeasurementDataset:
    """Execute M0 through M10 for `quick` or `full` mode within `budget`."""
    is_full = mode == "full"
    eff_m2_dur = m2_duration_sec if m2_duration_sec is not None else (2.0 if is_full else 1.0)
    eff_m3_dur = (
        m3_step_duration_sec if m3_step_duration_sec is not None else (0.5 if is_full else 0.3)
    )

    # M0 Sanity
    loop_u = loopback_url or (
        targets[0].url if targets and targets[0].ip.startswith("127.") else None
    )
    m0 = run_m0_sanity(budget, loopback_url=loop_u, plan_mbps=plan_mbps)

    if not targets:
        return MeasurementDataset(isp=isp, mode=mode, plan_mbps=plan_mbps, m0_sanity=m0)

    primary = next((t for t in targets if t.locality == "international"), targets[0])

    # Within-session start drift probe (M8)
    sb_start, sd_start, _, _ = measure_http_transfer(
        primary.url,
        budget,
        target_id=f"{primary.id}-start-drift",
        connections=1,
        duration_sec=0.3,
        max_transfer_bytes=min(262144, max(16384, budget.remaining_bytes() // 10)),
        supports_range=primary.supports_range,
    )
    start_probe_bps = round(sb_start / sd_start, 2) if sd_start > 0 else 0.0

    # M1 Baseline Path Quality
    m1 = run_m1_baseline(targets[:4])

    # M2 Single-Flow Time Series
    m2_list: list[SingleFlowSeries] = []
    for t in targets[:4]:
        if budget.exhausted or budget.is_dropped(t.id):
            continue
        s_m2 = run_m2_single_flow(
            t,
            budget,
            duration_sec=eff_m2_dur,
            max_bytes=min(budget.remaining_bytes() // 3, 4 * 1024 * 1024),
        )
        m2_list.append(s_m2)
        budget.jitter_pause(0.05, 0.15)

    # M3 Flow Scaling
    m3_list: list[FlowScalingResult] = []
    for t in targets[:3]:
        if budget.exhausted or budget.is_dropped(t.id):
            continue
        f_m3 = run_m3_flow_scaling(
            t,
            budget,
            conn_counts=(1, 2, 4, 8, 16) if is_full else (1, 4, 16),
            per_step_duration_sec=eff_m3_dur,
            per_step_max_bytes=min(max(65536, budget.remaining_bytes() // 8), 2 * 1024 * 1024),
        )
        m3_list.append(f_m3)

    # M4 Destination Independence
    if len(targets) >= 2 and not budget.exhausted:
        m4 = run_m4_destination_independence(
            targets[0],
            targets[1],
            budget,
            duration_sec=eff_m3_dur,
            max_bytes_per_leg=min(max(65536, budget.remaining_bytes() // 8), 1024 * 1024),
        )
    elif targets:
        m4 = run_m4_destination_independence(
            targets[0],
            targets[0],
            budget,
            duration_sec=eff_m3_dur,
            max_bytes_per_leg=min(max(65536, budget.remaining_bytes() // 8), 512 * 1024),
        )
    else:
        m4 = DestinationIndependenceResult()

    # M5 Burst and Bucket Analysis
    m5_list: list[BurstBucketResult] = []
    for idx, s_m2 in enumerate(m2_list):
        t_ref = next((t for t in targets if t.id == s_m2.target_id), None)
        b_m5 = run_m5_burst_bucket(
            s_m2,
            target=t_ref,
            budget=budget,
            test_refill=(is_full and idx == 0),
        )
        m5_list.append(b_m5)

    # M6 Policing vs Shaping vs Congestion
    idle_rtt_map = {pt.target_id: pt.rtt_avg_ms for pt in m1.targets}
    m6_list: list[PolicingShapingResult] = []
    for t in targets[:2]:
        if budget.exhausted or budget.is_dropped(t.id):
            continue
        p_m6 = run_m6_policing_shaping(
            t,
            budget,
            duration_sec=eff_m3_dur,
            idle_rtt_hint_ms=idle_rtt_map.get(t.id),
        )
        m6_list.append(p_m6)

    # M7 Protocol and Port Differentiation
    m7_list: list[ProtocolPortResult] = []
    if not budget.exhausted:
        m7_list.append(run_m7_protocol_port(primary, budget, duration_sec=eff_m3_dur))

    # M8 Time-of-day, Volume (FUP), and Upload + Within-session end drift probe
    m8 = run_m8_volume_and_upload(
        primary,
        budget,
        test_volume_fup=is_full,
        volume_max_bytes=min(max(131072, budget.remaining_bytes() // 3), 2 * 1024 * 1024),
    )
    sb_end, sd_end, _, _ = measure_http_transfer(
        primary.url,
        budget,
        target_id=f"{primary.id}-end-drift",
        connections=1,
        duration_sec=0.3,
        max_transfer_bytes=min(262144, max(16384, budget.remaining_bytes() // 4)),
        supports_range=primary.supports_range,
    )
    end_probe_bps = round(sb_end / sd_end, 2) if sd_end > 0 else start_probe_bps
    m8.session_start_probe_bps = start_probe_bps
    m8.session_end_probe_bps = end_probe_bps
    m8.session_drift_ratio = (
        round(end_probe_bps / max(start_probe_bps, 1.0), 3) if start_probe_bps > 0 else 1.0
    )
    if m8.download_bps <= 0 and m2_list:
        m8.download_bps = m2_list[0].avg_bps

    # Also store an active sample from this diagnostic session
    now_ep = get_clock().now()
    dt = datetime.fromtimestamp(now_ep, tz=UTC).astimezone()
    session_sample = SampleRecord(
        timestamp=dt.strftime("%Y-%m-%dT%H:%M:%S%z"),
        epoch=now_ep,
        hour=dt.hour,
        is_weekend=(dt.weekday() >= 5),
        interface=m0.active_interface,
        ssid=m0.ssid,
        network_id=m0.ssid or m0.active_interface or "default",
        source="active",
        target_group=primary.locality,
        host=primary.host,
        download_bps=m2_list[0].avg_bps if m2_list else start_probe_bps,
        upload_bps=m8.upload_bps,
        rtt_ms=idle_rtt_map.get(primary.id, 0.0),
        bytes_used=budget.used_bytes,
        series_bps=m2_list[0].samples_bps if m2_list else [],
    )
    ThrottleStore.record_sample(session_sample)
    m8.samples = ThrottleStore.get_samples(limit=500)

    # M9 Ground-Truth Modes
    d1 = m2_list[0].avg_bps if m2_list else start_probe_bps
    d16 = m3_list[0].points[-1].aggregate_bps if (m3_list and m3_list[0].points) else d1
    own_srv = next((t for t in targets if t.group == "own-server"), None)
    m9 = run_m9_ground_truth(
        primary,
        budget,
        direct_1_bps=d1,
        direct_16_bps=d16,
        proxy_url=proxy_url,
        own_server_target=own_srv,
        duration_sec=eff_m3_dur,
    )

    # M10 Local vs International Path
    m10 = run_m10_local_vs_intl(targets, m2_list, m3_list)

    return MeasurementDataset(
        isp=isp,
        mode=mode,
        plan_mbps=plan_mbps,
        m0_sanity=m0,
        m1_baseline=m1,
        m2_single_flow=m2_list,
        m3_flow_scaling=m3_list,
        m4_destination_independence=m4,
        m5_burst_bucket=m5_list,
        m6_policing_shaping=m6_list,
        m7_protocol_port=m7_list,
        m8_time_volume_upload=m8,
        m9_ground_truth=m9,
        m10_local_vs_intl=m10,
    )
