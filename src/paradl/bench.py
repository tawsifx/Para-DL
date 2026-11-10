"""Watchdog benchmark runner ('pdl bench <url> --watchdog v1,v2,off --duration 60')."""

from __future__ import annotations

import tempfile
from dataclasses import dataclass
from pathlib import Path

from paradl.engines.aria2 import Aria2Engine
from paradl.models import (
    ExitCode,
    TargetInfo,
    TargetType,
    WatchdogMode,
    format_size,
    format_speed,
)
from paradl.ui import UI, print_plain


@dataclass
class BenchRow:
    mode: str
    avg_speed_bps: float
    p10_speed_bps: float
    recycle_count: int
    bytes_downloaded: int
    total_time: float
    success: bool


def parse_watchdog_modes(modes_str: str) -> list[WatchdogMode]:
    """Parse comma-separated watchdog modes (e.g. 'v1,v2,off')."""
    modes: list[WatchdogMode] = []
    for part in modes_str.split(","):
        p = part.strip().lower()
        if not p:
            continue
        modes.append(WatchdogMode(p))
    return modes or [WatchdogMode.V1, WatchdogMode.V2, WatchdogMode.OFF]


def run_bench(
    url: str,
    *,
    modes_str: str = "v1,v2,off",
    duration_sec: float = 60.0,
    connections: int = 16,
    anti_throttle: str | None = "4MBps",
    grace_period: int = 5,
    poll_interval: float | None = None,
    min_cooldown_sec: float | None = None,
) -> tuple[int, list[BenchRow]]:
    """Run each specified watchdog mode back-to-back against `url` and print a comparison table."""
    try:
        modes = parse_watchdog_modes(modes_str)
    except ValueError as exc:
        print_plain(
            f"  [error]Invalid --watchdog mode list {modes_str!r}: {exc}[/error]", force=True
        )
        return int(ExitCode.USAGE), []

    effective_poll = (
        poll_interval if poll_interval is not None else (0.2 if duration_sec <= 15.0 else 0.5)
    )
    effective_cooldown = (
        min_cooldown_sec
        if min_cooldown_sec is not None
        else (1.5 if duration_sec <= 15.0 else 15.0)
    )

    print_plain(f"  [dim]•[/dim] [white]Benchmark Target:[/white] [cyan]{url}[/cyan]")
    print_plain(
        f"  [dim]•[/dim] [white]Modes:[/white]            [bold cyan]{', '.join(m.value for m in modes)}[/bold cyan] "
        f"[dim](max {duration_sec:.0f}s per mode)[/dim]"
    )

    rows: list[BenchRow] = []

    for mode in modes:
        print_plain(f"\n  [bold cyan]▶ Running benchmark mode: {mode.value}[/bold cyan]")
        with tempfile.TemporaryDirectory(prefix=f"pdl_bench_{mode.value}_") as tmp_dir:
            t_info = TargetInfo(
                raw=url,
                target_type=TargetType.DIRECT,
                clean_url=url,
                filename_hint=f"bench_{mode.value}.bin",
            )
            res = Aria2Engine.download(
                t_info,
                Path(tmp_dir),
                connections=connections,
                anti_throttle=anti_throttle if mode != WatchdogMode.OFF else None,
                grace_period=grace_period,
                watchdog_mode=mode,
                poll_interval=effective_poll,
                min_cooldown_sec=effective_cooldown,
                max_duration_sec=duration_sec,
            )
            rows.append(
                BenchRow(
                    mode=mode.value,
                    avg_speed_bps=res.avg_speed_bps,
                    p10_speed_bps=res.p10_speed_bps,
                    recycle_count=res.recycle_count,
                    bytes_downloaded=res.bytes_downloaded,
                    total_time=res.duration,
                    success=res.success,
                )
            )

    if UI.json_mode:
        UI.emit_event(
            "bench",
            url=url,
            duration_limit_sec=duration_sec,
            results=[
                {
                    "mode": r.mode,
                    "avg_speed_bps": round(r.avg_speed_bps, 2),
                    "p10_speed_bps": round(r.p10_speed_bps, 2),
                    "recycle_count": r.recycle_count,
                    "bytes_downloaded": r.bytes_downloaded,
                    "total_time": round(r.total_time, 3),
                    "success": r.success,
                }
                for r in rows
            ],
        )
    else:
        from rich import box
        from rich.table import Table

        table = Table(
            title=f"Watchdog Benchmark Comparison ({url})",
            box=box.HORIZONTALS,
            pad_edge=False,
        )
        table.add_column("Watchdog Mode", style="bold cyan")
        table.add_column("Average Speed", justify="right", style="bold green")
        table.add_column("p10 Speed", justify="right", style="yellow")
        table.add_column("Recycles", justify="right", style="cyan")
        table.add_column("Transferred", justify="right", style="dim white")
        table.add_column("Total Time", justify="right", style="white")

        for r in rows:
            table.add_row(
                r.mode,
                format_speed(r.avg_speed_bps),
                format_speed(r.p10_speed_bps),
                str(r.recycle_count),
                format_size(r.bytes_downloaded),
                f"{r.total_time:.2f}s",
            )

        UI.print()
        UI.print_renderable(table)
        UI.print()

    all_ok = all(r.success for r in rows)
    return (int(ExitCode.OK) if all_ok else int(ExitCode.FAILURE)), rows
