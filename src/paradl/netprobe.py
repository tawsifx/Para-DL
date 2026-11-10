"""Network & ISP throttling diagnostic ('pdl netprobe <url>') with SQLite profile persistence."""

from __future__ import annotations

import concurrent.futures
import logging
import socket
import subprocess
from dataclasses import dataclass, field
from urllib.parse import urlparse

from paradl import VERSION
from paradl.clock import get_clock
from paradl.history import HistoryManager, HostStrategy
from paradl.models import ExitCode, format_speed
from paradl.net import probe_range_support
from paradl.toolchain import TOOLCHAIN
from paradl.ui import UI, print_plain

logger = logging.getLogger(__name__)


@dataclass
class ProbePassMetrics:
    label: str
    connections: int
    ip_family: str
    duration: float
    total_bytes: int
    avg_speed_bps: float
    early_speed_bps: float
    late_speed_bps: float
    avg_rtt_ms: float = 0.0
    packet_loss_pct: float = 0.0
    time_series_bps: list[float] = field(default_factory=list)


@dataclass
class NetprobeReport:
    url: str
    host: str
    supports_range: bool
    content_length: int | None
    passes: list[ProbePassMetrics]
    throttle_class: str
    recommended_connections: int
    recommended_action: str
    recommended_ip_family: str
    verdict: str
    http3_supported: bool | None = None
    http3_detail: str | None = None


def probe_http3_support(url: str, timeout: float = 5.0) -> tuple[bool, str]:
    """Phase 3 #9: Optional probe checking whether curl with HTTP/3 works for url.
    Off by default; never changes download behavior automatically.
    """
    curl_bin = TOOLCHAIN.curl
    if not curl_bin:
        return False, "curl binary not found"
    try:
        proc = subprocess.run(
            [curl_bin, "--http3", "-I", "-s", "-S", "-m", str(int(max(timeout, 2))), url],
            capture_output=True,
            text=True,
            timeout=timeout + 2.0,
            check=False,
        )
        out = (proc.stdout or "") + "\n" + (proc.stderr or "")
        if proc.returncode == 0 and ("HTTP/3" in out or "HTTP/2" in out or "200" in out):
            first_line = (proc.stdout.strip().splitlines() or ["HTTP/3 OK"])[0]
            return True, first_line
        err_line = (proc.stderr.strip().splitlines() or [f"exit code {proc.returncode}"])[0]
        return False, err_line
    except (OSError, subprocess.SubprocessError) as exc:
        return False, str(exc)


def _measure_parallel_stream(
    url: str,
    *,
    connections: int,
    duration_sec: float,
    total_size: int | None,
    supports_range: bool,
) -> ProbePassMetrics:
    """Download from url using `connections` concurrent HTTP streams for `duration_sec` seconds
    and record a throughput time series (early burst vs late sustained).
    """
    import httpx

    conns = max(1, min(connections, 16))
    if not supports_range:
        conns = 1

    bucket_interval = 0.25 if duration_sec <= 6.0 else 0.5
    num_buckets = max(2, int(duration_sec / bucket_interval))
    bucket_bytes = [0] * (num_buckets + 4)
    start_ts = get_clock().monotonic()
    stop_at = start_ts + duration_sec

    def _worker(worker_idx: int) -> tuple[int, float]:
        worker_total = 0
        worker_rtt_ms = 0.0
        headers = {"User-Agent": f"pdl-netprobe/{VERSION}"}
        if supports_range and total_size and total_size > conns * 4096:
            chunk_span = total_size // conns
            start_byte = worker_idx * chunk_span
            end_byte = (
                (total_size - 1) if worker_idx == conns - 1 else ((worker_idx + 1) * chunk_span - 1)
            )
            headers["Range"] = f"bytes={start_byte}-{end_byte}"
        elif supports_range and worker_idx > 0:
            headers["Range"] = f"bytes={worker_idx * 65536}-"

        try:
            with httpx.Client(
                follow_redirects=True,
                timeout=httpx.Timeout(min(duration_sec + 3.0, 10.0), connect=3.0),
            ) as client:
                while get_clock().monotonic() < stop_at:
                    ttfb_start = get_clock().monotonic()
                    with client.stream("GET", url, headers=headers) as resp:
                        if resp.status_code not in (200, 206):
                            break
                        # Measure RTT as time-to-first-byte on the first request of each connection
                        first_chunk = True
                        for chunk in resp.iter_bytes(chunk_size=16384):
                            now = get_clock().monotonic()
                            if first_chunk:
                                worker_rtt_ms = (now - ttfb_start) * 1000.0
                                first_chunk = False
                            if now >= stop_at:
                                break
                            n = len(chunk)
                            worker_total += n
                            b_idx = int((now - start_ts) / bucket_interval)
                            if 0 <= b_idx < len(bucket_bytes):
                                bucket_bytes[b_idx] += n
                    # If the file was small and finished before stop_at, loop to keep measuring link
                    if get_clock().monotonic() >= stop_at:
                        break
        except Exception as exc:
            logger.debug("netprobe worker %d stopped: %s", worker_idx, exc)
        return worker_total, worker_rtt_ms

    with concurrent.futures.ThreadPoolExecutor(max_workers=conns) as pool:
        futs = [pool.submit(_worker, i) for i in range(conns)]
        results = [f.result() for f in futs]
        total_bytes = sum(r[0] for r in results)
        rtt_samples = [r[1] for r in results if r[1] > 0]

    actual_dur = max(get_clock().monotonic() - start_ts, 0.05)
    valid_buckets = max(1, min(int(actual_dur / bucket_interval), num_buckets))
    series = [bucket_bytes[i] / bucket_interval for i in range(valid_buckets)]
    if not series:
        series = [total_bytes / actual_dur]

    mid = max(1, len(series) // 2)
    early_slice = series[:mid]
    late_slice = series[mid:] if len(series) > 1 else series

    early_avg = sum(early_slice) / len(early_slice)
    late_avg = sum(late_slice) / len(late_slice)
    overall_avg = total_bytes / actual_dur
    avg_rtt = sum(rtt_samples) / len(rtt_samples) if rtt_samples else 0.0

    return ProbePassMetrics(
        label=f"{conns} conn(s)",
        connections=conns,
        ip_family="ipv4",
        duration=actual_dur,
        total_bytes=total_bytes,
        avg_speed_bps=overall_avg,
        early_speed_bps=early_avg,
        late_speed_bps=late_avg,
        avg_rtt_ms=avg_rtt,
        packet_loss_pct=0.0,
        time_series_bps=series,
    )


def classify_throttling(
    pass_1: ProbePassMetrics,
    pass_4: ProbePassMetrics,
    pass_16: ProbePassMetrics,
    *,
    supports_range: bool,
) -> tuple[str, int, str, str]:
    """Classify throttling profile from 1, 4, and 16 connection passes.

    Returns (throttle_class, recommended_connections, recommended_action, verdict).
    Classes: 'burst-then-throttle', 'per-flow cap', 'per-host cap', 'none'.
    """
    if not supports_range:
        return (
            "none",
            1,
            "single_stream",
            "Server does not honor HTTP Range (206); connection recycling disabled (1 stream).",
        )

    # 1. Check for burst-then-throttle: early burst speed significantly higher than late window speed
    for p in (pass_16, pass_4, pass_1):
        if p.early_speed_bps > 0 and p.late_speed_bps > 0:
            drop_ratio = p.late_speed_bps / p.early_speed_bps
            if drop_ratio <= 0.58:
                return (
                    "burst-then-throttle",
                    16,
                    "tier2_pause_unpause",
                    f"Detected burst-then-throttle ({format_speed(p.early_speed_bps)} burst -> "
                    f"{format_speed(p.late_speed_bps)} throttled). Watchdog v2 socket recycling recommended.",
                )

    s1 = max(pass_1.avg_speed_bps, 1.0)
    s4 = max(pass_4.avg_speed_bps, 1.0)
    s16 = max(pass_16.avg_speed_bps, 1.0)

    # 2. Check for per-flow cap: multi-connection scales strongly over 1 connection
    if (s16 / s1) >= 2.0 or (s4 / s1) >= 1.75:
        return (
            "per-flow cap",
            16,
            "tier1_native",
            f"Detected per-flow cap (1 conn: {format_speed(s1)} -> 16 conns: {format_speed(s16)}). "
            "16 parallel connections recommended.",
        )

    # 3. Check for per-host cap vs unthrottled ('none'):
    # When 1, 4, and 16 connections all hit roughly the same aggregate ceiling (< 1.35x scaling)
    # and the ceiling is modest (< 10 MB/s), classify as per-host cap.
    if (s16 / s1) < 1.35 and (s4 / s1) < 1.30 and s16 < 10 * 1024 * 1024:
        return (
            "per-host cap",
            4,
            "tier1_native",
            f"Detected per-host aggregate cap (~{format_speed(s16)} regardless of stream count). "
            "4 connections recommended to avoid socket contention.",
        )

    return (
        "none",
        16,
        "tier1_native",
        f"No ISP/server throttling detected (peak {format_speed(max(s1, s4, s16))}).",
    )


def _check_ipv6_reachable(host: str) -> bool:
    if not host:
        return False
    try:
        infos = socket.getaddrinfo(host, None, socket.AF_INET6)
        return len(infos) > 0
    except OSError:
        return False


def run_netprobe(
    url: str,
    *,
    duration_sec: float = 60.0,
    http3_probe: bool = False,
) -> tuple[int, NetprobeReport]:
    """Run the 1 / 4 / 16 connection netprobe diagnostic, persist strategy to SQLite, and display verdict."""
    parsed = urlparse(url)
    host = (parsed.hostname or "unknown").lower()

    print_plain(f"  [dim]•[/dim] [white]Target Host:[/white] [cyan]{host}[/cyan] ({url})")
    print_plain("  [dim]•[/dim] [white]Probing HTTP Range & IPv4/IPv6 support...[/white]")

    r_probe = probe_range_support(url, timeout=5.0)
    supports_range = r_probe.accept_ranges if r_probe.ok else False
    content_length = r_probe.content_length if r_probe.ok else None
    has_ipv6 = _check_ipv6_reachable(host)

    # Split duration across the 3 connection passes (1, 4, 16)
    per_pass_dur = max(0.6, float(duration_sec) / 3.0)
    passes: list[ProbePassMetrics] = []

    for c_cnt in (1, 4, 16):
        print_plain(
            f"  [dim]• Running {c_cnt}-connection throughput probe ({per_pass_dur:.1f}s)...[/dim]"
        )
        m = _measure_parallel_stream(
            r_probe.final_url or url,
            connections=c_cnt,
            duration_sec=per_pass_dur,
            total_size=content_length,
            supports_range=supports_range,
        )
        passes.append(m)

    t_class, rec_conns, rec_action, verdict = classify_throttling(
        passes[0],
        passes[1],
        passes[2],
        supports_range=supports_range,
    )
    rec_ip_family = "auto" if has_ipv6 else "ipv4"

    h3_ok: bool | None = None
    h3_detail: str | None = None
    if http3_probe:
        print_plain("  [dim]• Probing optional HTTP/3 support via curl...[/dim]")
        h3_ok, h3_detail = probe_http3_support(url)

    # Persist recommended profile into SQLite host_strategies
    strategy = HostStrategy(
        host=host,
        best_action=rec_action,
        connections=rec_conns,
        ip_family=rec_ip_family,
        speed_before=passes[0].avg_speed_bps,
        speed_after=passes[-1].avg_speed_bps,
        supports_range=supports_range,
        throttle_class=t_class,
        avg_rtt_ms=passes[0].avg_rtt_ms,
        packet_loss_pct=passes[0].packet_loss_pct,
    )
    HistoryManager.save_host_strategy(strategy)

    report = NetprobeReport(
        url=url,
        host=host,
        supports_range=supports_range,
        content_length=content_length,
        passes=passes,
        throttle_class=t_class,
        recommended_connections=rec_conns,
        recommended_action=rec_action,
        recommended_ip_family=rec_ip_family,
        verdict=verdict,
        http3_supported=h3_ok,
        http3_detail=h3_detail,
    )

    if UI.json_mode:
        UI.emit_event(
            "netprobe",
            url=url,
            host=host,
            supports_range=supports_range,
            ipv6_DNS=has_ipv6,
            throttle_class=t_class,
            recommended_connections=rec_conns,
            recommended_action=rec_action,
            verdict=verdict,
            http3_supported=h3_ok,
            http3_detail=h3_detail,
            passes=[
                {
                    "connections": p.connections,
                    "avg_speed_bps": round(p.avg_speed_bps, 2),
                    "early_speed_bps": round(p.early_speed_bps, 2),
                    "late_speed_bps": round(p.late_speed_bps, 2),
                    "total_bytes": p.total_bytes,
                    "duration": round(p.duration, 2),
                }
                for p in passes
            ],
        )
    else:
        from rich import box
        from rich.table import Table

        table = Table(
            title=f"Netprobe Diagnostic: {host}",
            box=box.HORIZONTALS,
            pad_edge=False,
        )
        table.add_column("Streams", justify="right", style="cyan")
        table.add_column("Early Burst", justify="right", style="green")
        table.add_column("Late Sustained", justify="right", style="yellow")
        table.add_column("Average Speed", justify="right", style="bold white")

        for p in passes:
            table.add_row(
                f"{p.connections}x",
                format_speed(p.early_speed_bps),
                format_speed(p.late_speed_bps),
                format_speed(p.avg_speed_bps),
            )

        UI.print()
        UI.print_renderable(table)
        UI.print()
        UI.print(f"  [bold white]Classification:[/bold white] [cyan]{t_class}[/cyan]")
        UI.print(
            f"  [bold white]Range Support:[/bold white]  {'[green]206 Partial Content[/green]' if supports_range else '[yellow]200 Only (No Range)[/yellow]'}"
        )
        if http3_probe:
            h3_str = (
                f"[green]Supported ({h3_detail})[/green]"
                if h3_ok
                else f"[dim]Unavailable ({h3_detail})[/dim]"
            )
            UI.print(f"  [bold white]HTTP/3 Probe:[/bold white]   {h3_str}")
        UI.print(f"  [bold white]Verdict:[/bold white]        {verdict}")
        UI.print(
            f"  [dim]Saved host profile for '{host}' (conns={rec_conns}, action={rec_action}) to SQLite.[/dim]\n"
        )

    return int(ExitCode.OK), report
