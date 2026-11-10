"""Persistent SQLite download queue, foreground priority runner, --when-fast / --at scheduling, and LaunchAgent plist generator."""

from __future__ import annotations

import contextlib
import json
import logging
import os
import plistlib
import re
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from paradl import platform
from paradl.clock import get_clock
from paradl.history import HistoryManager
from paradl.models import (
    ExitCode,
    TransferResult,
    WatchdogMode,
    format_size,
    format_speed,
)
from paradl.net import get_http_client, probe_target
from paradl.paths import get_logs_dir, get_paradl_home
from paradl.platform import macos
from paradl.ui import UI, print_plain

logger = logging.getLogger(__name__)

_AT_TIME_RE = re.compile(r"^\s*(\d{1,2}):(\d{2})\s*$")


def parse_start_at_time(at_str: str | None) -> tuple[int, int] | None:
    """Parse and validate a 24-hour 'HH:MM' time string into (hour, minute)."""
    if at_str is None:
        return None
    s = at_str.strip()
    if not s:
        return None
    m = _AT_TIME_RE.match(s)
    if not m:
        raise ValueError(
            f"Invalid --at time {at_str!r} (expected 24-hour HH:MM, e.g. '02:30' or '14:00')"
        )
    hh = int(m.group(1))
    mm = int(m.group(2))
    if not (0 <= hh <= 23 and 0 <= mm <= 59):
        raise ValueError(f"Invalid --at time {at_str!r} (hour must be 00..23 and minute 00..59)")
    return hh, mm


def format_start_at_time(at_str: str | None) -> str | None:
    """Normalize a valid 'HH:MM' string to zero-padded 'HH:MM'."""
    parsed = parse_start_at_time(at_str)
    if parsed is None:
        return None
    return f"{parsed[0]:02d}:{parsed[1]:02d}"


def is_schedule_due(
    start_at: str | None,
    *,
    now_dt: datetime | None = None,
) -> tuple[bool, float]:
    """Return (is_due, seconds_until_due) for an 'HH:MM' schedule."""
    parsed = parse_start_at_time(start_at)
    if parsed is None:
        return True, 0.0
    hh, mm = parsed
    now = now_dt or datetime.now()
    now_secs = now.hour * 3600 + now.minute * 60 + now.second
    target_secs = hh * 3600 + mm * 60
    if now_secs >= target_secs:
        return True, 0.0
    return False, float(target_secs - now_secs)


def run_short_speed_test(
    url: str,
    *,
    headers: dict[str, str] | None = None,
    probe_bytes: int = 131072,
    timeout: float = 3.0,
) -> float:
    """Run a short HTTP range speed probe (default 128 KiB) and return measured speed in B/s."""
    if not url.startswith(("http://", "https://")):
        return 0.0
    client = get_http_client()
    req_headers = {
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        "Range": f"bytes=0-{max(1024, int(probe_bytes)) - 1}",
    }
    if headers:
        req_headers.update(headers)

    t0 = get_clock().monotonic()
    total_read = 0
    try:
        with client.stream(
            "GET", url, headers=req_headers, timeout=timeout, follow_redirects=True
        ) as resp:
            if resp.status_code not in (200, 206):
                return 0.0
            for chunk in resp.iter_bytes(chunk_size=16384):
                total_read += len(chunk)
                if total_read >= probe_bytes or (get_clock().monotonic() - t0) >= timeout:
                    break
    except Exception as exc:
        logger.debug("Short speed test failed for %s: %s", url, exc)
        return 0.0

    elapsed = max(get_clock().monotonic() - t0, 0.001)
    if total_read <= 0:
        return 0.0
    return float(total_read) / elapsed


def check_when_fast_gate(
    url: str,
    threshold_bps: float,
    *,
    headers: dict[str, str] | None = None,
    probe_bytes: int = 131072,
    max_polls: int = 1,
    poll_interval_sec: float = 0.5,
    speed_prober: Callable[[str], float] | None = None,
) -> tuple[bool, float, str | None]:
    """Check whether short speed test exceeds `threshold_bps`, polling up to `max_polls` times.

    Returns (passed, measured_bps, failure_reason_or_None).
    """
    if threshold_bps <= 0:
        return True, 0.0, None

    polls = max(1, int(max_polls))
    measured = 0.0
    for attempt in range(polls):
        if speed_prober is not None:
            measured = float(speed_prober(url))
        else:
            measured = run_short_speed_test(url, headers=headers, probe_bytes=probe_bytes)
        UI.emit_event(
            "when_fast_probe",
            url=url,
            attempt=attempt + 1,
            measured_bps=round(measured, 2),
            threshold_bps=round(threshold_bps, 2),
            passed=measured >= threshold_bps,
        )
        if measured >= threshold_bps:
            return True, measured, None
        if attempt < polls - 1 and poll_interval_sec > 0:
            get_clock().sleep(poll_interval_sec)

    msg = (
        f"Deferred by --when-fast: short speed test measured {format_speed(measured)}, "
        f"below required threshold {format_speed(threshold_bps)}"
    )
    return False, measured, msg


@dataclass
class QueueJob:
    """Persistent download job stored in SQLite (`queue_jobs` table)."""

    id: int
    target: str
    target_type: str = "direct"
    out_dir: str = ""
    filename: str | None = None
    mirrors: list[str] = field(default_factory=list)
    priority: int = 0
    connections: int = 16
    status: str = "queued"  # queued, running, paused, completed, failed
    require_ac: bool = True
    pause_on_low_power: bool = False
    when_fast_bps: float = 0.0
    start_at: str | None = None
    expected_length: int | None = None
    expected_digest: str | None = None
    category: str = ""
    bytes_downloaded: int = 0
    error: str | None = None
    options: dict[str, Any] = field(default_factory=dict)
    created_at: str = ""
    updated_at: str = ""


class QueueManager:
    """SQLite-backed persistent download queue manager."""

    @staticmethod
    def _row_to_job(row: Any) -> QueueJob:
        try:
            mirrors = json.loads(row["mirrors_json"] or "[]")
            if not isinstance(mirrors, list):
                mirrors = []
        except json.JSONDecodeError:
            mirrors = []
        try:
            options = json.loads(row["options_json"] or "{}")
            if not isinstance(options, dict):
                options = {}
        except json.JSONDecodeError:
            options = {}

        return QueueJob(
            id=int(row["id"]),
            target=str(row["target"]),
            target_type=str(row["target_type"] or "direct"),
            out_dir=str(row["out_dir"]),
            filename=row["filename"],
            mirrors=[str(m) for m in mirrors],
            priority=int(row["priority"] or 0),
            connections=int(row["connections"] or 16),
            status=str(row["status"] or "queued"),
            require_ac=bool(row["require_ac"]),
            pause_on_low_power=bool(row["pause_on_low_power"]),
            when_fast_bps=float(row["when_fast_bps"] or 0.0),
            start_at=row["start_at"],
            expected_length=int(row["expected_length"])
            if row["expected_length"] is not None
            else None,
            expected_digest=row["expected_digest"],
            category=str(row["category"] or ""),
            bytes_downloaded=int(row["bytes_downloaded"] or 0),
            error=row["error"],
            options=options,
            created_at=str(row["created_at"] or ""),
            updated_at=str(row["updated_at"] or ""),
        )

    @classmethod
    def add_job(
        cls,
        target: str,
        out_dir: Path | str,
        *,
        target_type: str = "direct",
        filename: str | None = None,
        mirrors: list[str] | None = None,
        priority: int = 0,
        connections: int = 16,
        require_ac: bool = True,
        pause_on_low_power: bool = False,
        when_fast_bps: float = 0.0,
        start_at: str | None = None,
        expected_length: int | None = None,
        expected_digest: str | None = None,
        category: str = "",
        options: dict[str, Any] | None = None,
    ) -> QueueJob:
        """Add a job to the persistent SQLite queue.

        Note: Queued jobs default to `require_ac=True` per Part B2 specification.
        """
        norm_at = format_start_at_time(start_at)
        now_str = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        out_path_str = str(Path(out_dir).expanduser().resolve())
        safe_conn = min(max(int(connections), 1), 16)
        mirrors_json = json.dumps(list(mirrors or []))
        options_json = json.dumps(dict(options or {}))

        conn = HistoryManager._connect()
        try:
            cur = conn.execute(
                """
                INSERT INTO queue_jobs (
                    target, target_type, out_dir, filename, mirrors_json,
                    priority, connections, status, require_ac, pause_on_low_power,
                    when_fast_bps, start_at, expected_length, expected_digest,
                    category, bytes_downloaded, error, options_json, created_at, updated_at
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, 0, NULL, ?, ?, ?)
                """,
                (
                    target.strip(),
                    target_type,
                    out_path_str,
                    filename,
                    mirrors_json,
                    int(priority),
                    safe_conn,
                    1 if require_ac else 0,
                    1 if pause_on_low_power else 0,
                    float(when_fast_bps),
                    norm_at,
                    expected_length,
                    expected_digest,
                    str(category),
                    options_json,
                    now_str,
                    now_str,
                ),
            )
            job_id = int(cur.lastrowid or 0)
            conn.commit()
            row = conn.execute("SELECT * FROM queue_jobs WHERE id = ?", (job_id,)).fetchone()
            assert row is not None
            return cls._row_to_job(row)
        finally:
            conn.close()

    @classmethod
    def get_job(cls, job_id: int) -> QueueJob | None:
        conn = HistoryManager._connect()
        try:
            row = conn.execute("SELECT * FROM queue_jobs WHERE id = ?", (int(job_id),)).fetchone()
            return cls._row_to_job(row) if row is not None else None
        finally:
            conn.close()

    @classmethod
    def list_jobs(cls, *, status: str | None = None) -> list[QueueJob]:
        """List queued jobs ordered by priority DESC, id ASC."""
        conn = HistoryManager._connect()
        try:
            if status:
                rows = conn.execute(
                    "SELECT * FROM queue_jobs WHERE status = ? ORDER BY priority DESC, id ASC",
                    (status,),
                ).fetchall()
            else:
                rows = conn.execute(
                    "SELECT * FROM queue_jobs ORDER BY priority DESC, id ASC"
                ).fetchall()
            return [cls._row_to_job(r) for r in rows]
        finally:
            conn.close()

    @classmethod
    def update_job_status(
        cls,
        job_id: int,
        status: str,
        *,
        bytes_downloaded: int | None = None,
        error: str | None = None,
        filename: str | None = None,
    ) -> None:
        now_str = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        conn = HistoryManager._connect()
        try:
            row = conn.execute("SELECT * FROM queue_jobs WHERE id = ?", (int(job_id),)).fetchone()
            if row is None:
                return
            b_done = (
                int(bytes_downloaded)
                if bytes_downloaded is not None
                else int(row["bytes_downloaded"] or 0)
            )
            fname = filename if filename is not None else row["filename"]
            conn.execute(
                """
                UPDATE queue_jobs
                SET status = ?, bytes_downloaded = ?, error = ?, filename = ?, updated_at = ?
                WHERE id = ?
                """,
                (status, b_done, error, fname, now_str, int(job_id)),
            )
            conn.commit()
        finally:
            conn.close()

    @classmethod
    def remove_jobs(cls, job_ids: list[int] | None = None, *, remove_all: bool = False) -> int:
        conn = HistoryManager._connect()
        try:
            if remove_all:
                cur = conn.execute("DELETE FROM queue_jobs")
            elif job_ids:
                placeholders = ",".join("?" for _ in job_ids)
                cur = conn.execute(
                    f"DELETE FROM queue_jobs WHERE id IN ({placeholders})",  # nosec B608
                    [int(x) for x in job_ids],
                )
            else:
                return 0
            conn.commit()
            return int(cur.rowcount or 0)
        finally:
            conn.close()

    @classmethod
    def pause_jobs(cls, job_ids: list[int] | None = None, *, pause_all: bool = False) -> int:
        now_str = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        conn = HistoryManager._connect()
        try:
            if pause_all:
                cur = conn.execute(
                    "UPDATE queue_jobs SET status = 'paused', updated_at = ? WHERE status IN ('queued', 'running')",
                    (now_str,),
                )
            elif job_ids:
                placeholders = ",".join("?" for _ in job_ids)
                cur = conn.execute(
                    f"UPDATE queue_jobs SET status = 'paused', updated_at = ? "
                    f"WHERE id IN ({placeholders}) AND status != 'completed'",  # nosec B608
                    [now_str, *(int(x) for x in job_ids)],
                )
            else:
                return 0
            conn.commit()
            return int(cur.rowcount or 0)
        finally:
            conn.close()

    @classmethod
    def resume_jobs(cls, job_ids: list[int] | None = None, *, resume_all: bool = False) -> int:
        """Mark paused, running (crashed), or failed jobs back to 'queued'."""
        now_str = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        conn = HistoryManager._connect()
        try:
            if resume_all:
                cur = conn.execute(
                    "UPDATE queue_jobs SET status = 'queued', error = NULL, updated_at = ? "
                    "WHERE status IN ('paused', 'running', 'failed')",
                    (now_str,),
                )
            elif job_ids:
                placeholders = ",".join("?" for _ in job_ids)
                cur = conn.execute(
                    f"UPDATE queue_jobs SET status = 'queued', error = NULL, updated_at = ? "
                    f"WHERE id IN ({placeholders}) AND status != 'completed'",  # nosec B608
                    [now_str, *(int(x) for x in job_ids)],
                )
            else:
                return 0
            conn.commit()
            return int(cur.rowcount or 0)
        finally:
            conn.close()

    @classmethod
    def recover_crashed_jobs(cls) -> int:
        """Transition any unfinished jobs left in `status = 'running'` after a crash or reboot back to `'queued'`."""
        now_str = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        conn = HistoryManager._connect()
        try:
            cur = conn.execute(
                "UPDATE queue_jobs SET status = 'queued', updated_at = ? WHERE status = 'running'",
                (now_str,),
            )
            conn.commit()
            recovered = int(cur.rowcount or 0)
            if recovered > 0:
                UI.emit_event("queue_crash_recovered", recovered_count=recovered)
            return recovered
        finally:
            conn.close()

    @classmethod
    def display_queue(cls) -> None:
        jobs = cls.list_jobs()
        if UI.json_mode:
            UI.emit_event(
                "queue_list",
                jobs=[
                    {
                        "id": j.id,
                        "target": j.target,
                        "status": j.status,
                        "priority": j.priority,
                        "connections": j.connections,
                        "require_ac": j.require_ac,
                        "when_fast_bps": j.when_fast_bps,
                        "start_at": j.start_at,
                        "bytes_downloaded": j.bytes_downloaded,
                        "expected_length": j.expected_length,
                        "out_dir": j.out_dir,
                        "filename": j.filename,
                        "error": j.error,
                    }
                    for j in jobs
                ],
            )
            return

        if not jobs:
            print_plain("  [dim]Queue is empty. Add jobs with 'pdl queue add <url>'.[/dim]")
            return

        from rich import box
        from rich.table import Table

        table = Table(
            title=f"Persistent Download Queue ({len(jobs)} job(s))",
            box=box.HORIZONTALS,
            pad_edge=False,
        )
        table.add_column("ID", justify="right", style="cyan")
        table.add_column("Prio", justify="right", style="bold white")
        table.add_column("Status", justify="center")
        table.add_column("Target / Filename", style="white")
        table.add_column("Progress", justify="right", style="dim")
        table.add_column("Guards / Schedule", style="dim")

        for j in jobs:
            st_style = {
                "completed": "[green]completed[/green]",
                "running": "[bold cyan]running[/bold cyan]",
                "queued": "[yellow]queued[/yellow]",
                "paused": "[dim]paused[/dim]",
                "failed": "[red]failed[/red]",
            }.get(j.status, j.status)
            label = j.filename or j.target
            prog = format_size(j.bytes_downloaded)
            if j.expected_length:
                prog = f"{prog} / {format_size(j.expected_length)}"
            guards: list[str] = []
            if j.require_ac:
                guards.append("AC")
            if j.when_fast_bps > 0:
                guards.append(f">={format_speed(j.when_fast_bps)}")
            if j.start_at:
                guards.append(f"@{j.start_at}")
            table.add_row(
                str(j.id),
                str(j.priority),
                st_style,
                label[:48],
                prog,
                ", ".join(guards) or "-",
            )

        UI.print()
        UI.print_renderable(table)
        UI.print()

    @classmethod
    def run_queue(
        cls,
        *,
        global_connections: int = 16,
        when_fast_override_bps: float | None = None,
        at_override: str | None = None,
        require_ac_override: bool | None = None,
        pause_on_low_power_override: bool | None = None,
        caffeinate: bool = True,
        net_watch: bool = True,
        stall_timeout: float = 30.0,
        notify: bool = False,
        poll_interval: float = 0.25,
        min_cooldown_sec: float = 5.0,
        when_fast_max_polls: int = 1,
        when_fast_poll_interval: float = 0.2,
        when_fast_retest_interval_sec: float = 15.0,
        speed_prober: Callable[[str], float] | None = None,
        now_fn: Callable[[], datetime] | None = None,
        on_job_start: Callable[[QueueJob], None] | None = None,
    ) -> tuple[int, list[QueueJob]]:
        """Run queued jobs in the foreground ordered by priority DESC, id ASC.

        - Automatically recovers any job left in `status = 'running'` after a crash or reboot.
        - Enforces global connection budget (`<= 16`).
        - Applies Part A guards (`--require-ac` default ON for queued jobs, free-disk check, WAKE/NET_CHANGE/STALL).
        - Enforces `--at HH:MM` and `--when-fast` gating.
        """
        from paradl.engines.aria2 import Aria2Engine

        cls.recover_crashed_jobs()
        safe_global_conn = min(max(int(global_connections), 1), 16)
        runnable = [j for j in cls.list_jobs() if j.status == "queued"]

        if not runnable:
            print_plain("  [dim]No runnable jobs in queue.[/dim]")
            return int(ExitCode.OK), []

        executed_jobs: list[QueueJob] = []
        overall_code = int(ExitCode.OK)

        with platform.get_sleep_assertion(enabled=caffeinate):
            for job in runnable:
                eff_at = at_override if at_override is not None else job.start_at
                eff_when_fast = (
                    float(when_fast_override_bps)
                    if when_fast_override_bps is not None and when_fast_override_bps > 0
                    else job.when_fast_bps
                )
                eff_require_ac = (
                    require_ac_override if require_ac_override is not None else job.require_ac
                )
                eff_low_power = (
                    pause_on_low_power_override
                    if pause_on_low_power_override is not None
                    else job.pause_on_low_power
                )
                eff_conn = min(max(1, job.connections), safe_global_conn)

                # 1. Check --at HH:MM fixed start time gate
                now_dt = now_fn() if now_fn is not None else datetime.now()
                due, secs_wait = is_schedule_due(eff_at, now_dt=now_dt)
                if not due:
                    msg = f"Job #{job.id} scheduled for --at {eff_at} ({int(secs_wait)}s from now); keeping queued."
                    print_plain(f"  [dim]• {msg}[/dim]")
                    UI.emit_event(
                        "queue_job_deferred", job_id=job.id, reason="start_at", start_at=eff_at
                    )
                    continue

                # 2. Check --when-fast short speed test gate
                if eff_when_fast > 0:
                    passed, measured_bps, fast_err = check_when_fast_gate(
                        job.target,
                        eff_when_fast,
                        max_polls=when_fast_max_polls,
                        poll_interval_sec=when_fast_poll_interval,
                        speed_prober=speed_prober,
                    )
                    if not passed:
                        print_plain(f"  [warning]⏸ Job #{job.id}: {fast_err}[/warning]", force=True)
                        UI.emit_event(
                            "queue_job_deferred",
                            job_id=job.id,
                            reason="when_fast",
                            measured_bps=round(measured_bps, 2),
                            threshold_bps=round(eff_when_fast, 2),
                        )
                        continue

                # Transition job to 'running' in SQLite before starting transfer (enables crash-resume)
                cls.update_job_status(job.id, "running")
                job.status = "running"
                if on_job_start is not None:
                    on_job_start(job)

                UI.emit_event(
                    "queue_job_started",
                    job_id=job.id,
                    target=job.target,
                    priority=job.priority,
                    connections=eff_conn,
                )

                out_dir_path = Path(job.out_dir)
                out_dir_path.mkdir(parents=True, exist_ok=True)

                # Check if partial file exists from a prior crashed/interrupted run
                existing_partial_bytes = 0
                if job.filename:
                    part_p = out_dir_path / job.filename
                    if part_p.is_file():
                        with contextlib.suppress(OSError):
                            existing_partial_bytes = part_p.stat().st_size

                t_info = probe_target(job.target)
                if job.filename:
                    t_info.filename_hint = job.filename
                if job.expected_length is not None:
                    t_info.content_length = job.expected_length
                if job.expected_digest is not None:
                    t_info.expected_digest = job.expected_digest
                if job.mirrors:
                    t_info.mirrors = list(job.mirrors)

                opts = job.options
                res: TransferResult = Aria2Engine.download(
                    t_info,
                    out_dir_path,
                    connections=eff_conn,
                    limit_rate=opts.get("limit_rate"),
                    anti_throttle=opts.get("anti_throttle", "4MBps"),
                    grace_period=int(opts.get("grace_period", 5)),
                    watchdog_mode=opts.get("watchdog_mode", WatchdogMode.V2),
                    poll_interval=poll_interval,
                    min_cooldown_sec=min_cooldown_sec,
                    caffeinate=bool(opts.get("caffeinate", caffeinate)),
                    net_watch=bool(opts.get("net_watch", net_watch)),
                    stall_timeout=float(opts.get("stall_timeout", stall_timeout)),
                    require_ac=eff_require_ac,
                    pause_on_low_power=eff_low_power,
                    mirrors=list(job.mirrors),
                    when_fast_bps=eff_when_fast,
                    when_fast_retest_interval_sec=when_fast_retest_interval_sec,
                    when_fast_prober=speed_prober,
                    http3=bool(opts.get("http3", True)),
                    proxy=opts.get("proxy"),
                )

                final_fname = (
                    res.file_path.name
                    if (res.file_path and res.file_path.is_file())
                    else job.filename
                )
                bytes_done = res.bytes_downloaded or existing_partial_bytes
                if res.success:
                    cls.update_job_status(
                        job.id,
                        "completed",
                        bytes_downloaded=bytes_done,
                        error=None,
                        filename=final_fname,
                    )
                    job.status = "completed"
                    job.bytes_downloaded = bytes_done
                    job.filename = final_fname
                elif res.error and "Deferred by --when-fast" in res.error:
                    # Mid-transfer --when-fast re-test paused/deferred the job; keep queued for next run
                    cls.update_job_status(
                        job.id,
                        "queued",
                        bytes_downloaded=bytes_done,
                        error=res.error,
                        filename=final_fname,
                    )
                    job.status = "queued"
                    job.bytes_downloaded = bytes_done
                    job.error = res.error
                else:
                    cls.update_job_status(
                        job.id,
                        "failed",
                        bytes_downloaded=bytes_done,
                        error=res.error,
                        filename=final_fname,
                    )
                    job.status = "failed"
                    job.bytes_downloaded = bytes_done
                    job.error = res.error
                    if overall_code == int(ExitCode.OK):
                        overall_code = res.exit_code or int(ExitCode.FAILURE)

                executed_jobs.append(job)

        if notify:
            from paradl.hooks import HookRunner

            tot = len(executed_jobs)
            succ = sum(1 for j in executed_jobs if j.status == "completed")
            fail = sum(1 for j in executed_jobs if j.status == "failed")
            HookRunner.notify_queue_summary(tot, succ, fail, enabled=notify)

        return overall_code, executed_jobs


# ---------------------------------------------------------------------------
# B3: LaunchAgent Plist Generator (`pdl queue launchd-plist`)
# ---------------------------------------------------------------------------


def generate_launchd_plist(
    dest_path: Path | None = None,
    *,
    start_at: str | None = None,
    interval_sec: int = 900,
    label: str = "com.paradl.queue",
) -> Path:
    """Generate a macOS LaunchAgent plist inside the workspace and print load instructions.

    Never invokes `launchctl` or installs anything system-wide.
    """
    paradl_home = get_paradl_home()
    logs_dir = get_logs_dir()
    logs_dir.mkdir(parents=True, exist_ok=True)

    if dest_path is None:
        out_plist = Path.cwd() / f"{label}.plist"
    else:
        cand = Path(dest_path).expanduser().resolve()
        out_plist = cand / f"{label}.plist" if cand.is_dir() else cand

    out_plist.parent.mkdir(parents=True, exist_ok=True)

    prog_args = [
        sys.executable,
        "-m",
        "paradl.cli",
        "queue",
        "run",
        "--non-interactive",
    ]

    plist_dict: dict[str, Any] = {
        "Label": label,
        "ProgramArguments": prog_args,
        "RunAtLoad": True,
        "WorkingDirectory": str(out_plist.parent),
        "EnvironmentVariables": {
            "PARADL_HOME": str(paradl_home),
            "PATH": os.environ.get(
                "PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
            ),
        },
        "StandardOutPath": str(logs_dir / "queue.out.log"),
        "StandardErrorPath": str(logs_dir / "queue.err.log"),
    }

    parsed_at = parse_start_at_time(start_at)
    if parsed_at is not None:
        hh, mm = parsed_at
        plist_dict["StartCalendarInterval"] = {"Hour": hh, "Minute": mm}
    else:
        plist_dict["StartInterval"] = max(60, int(interval_sec))

    xml_bytes = plistlib.dumps(plist_dict, fmt=plistlib.FMT_XML, sort_keys=True)
    out_plist.write_bytes(xml_bytes)

    ok, lint_msg = macos.validate_plist(out_plist)
    if not ok:
        raise ValueError(f"Generated plist failed validation: {lint_msg}")

    UI.emit_event(
        "launchd_plist_generated",
        path=str(out_plist),
        label=label,
        logs_dir=str(logs_dir),
        lint=lint_msg,
    )

    if not UI.json_mode and not UI.quiet:
        print_plain(
            f"  [green]✓ Generated LaunchAgent plist:[/green] [bold cyan]{out_plist}[/bold cyan]"
        )
        print_plain(f"  [dim]• Validation:[/dim] [white]{lint_msg}[/white]")
        print_plain(f"  [dim]• Logs:[/dim]       [dim]{logs_dir / 'queue.out.log'}[/dim]")
        print_plain(
            "\n  [bold white]To load this LaunchAgent yourself (not run automatically):[/bold white]"
        )
        print_plain(f"    1. cp {out_plist} ~/Library/LaunchAgents/{label}.plist")
        print_plain(f"    2. launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{label}.plist")
        print_plain(f"    3. To unload later: launchctl bootout gui/$(id -u)/{label}\n")

    return out_plist
