"""Regression tests for the downloader bugs found during toolchain provisioning.

Bugs covered:
  1. Watchdog v2 recycled connections even when disabled, and repeatedly on
     back-to-back resilience events (cycle storms amplified throttling).
  2. Progress reached a false 100% when an aria2 GID errored with a known size.
  3. Progress underreported real bytes because aria2's completedLength lagged.
"""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

from paradl.engines.aria2 import _disk_progress, _safe_progress
from paradl.engines.watchdog import (
    WatchdogActionType,
    WatchdogController,
    WatchdogMode,
)


class _Event:
    event_type = SimpleNamespace(value="net_change")
    reason = "network changed"


def _controller(mode: WatchdogMode = WatchdogMode.V2) -> WatchdogController:
    return WatchdogController(
        mode=mode,
        user_floor_bps=100_000,
        connections=16,
    )


# ---------------------------------------------------------------------------
# 1. Watchdog OFF must not recycle; back-to-back events need a cooldown
# ---------------------------------------------------------------------------


def test_watchdog_off_ignores_resilience_event() -> None:
    wd = _controller(WatchdogMode.OFF)
    decision = wd.step(elapsed_sec=1.0, speed_bps=0.0, resilience_event=_Event())
    assert decision.action == WatchdogActionType.NONE
    assert wd.cycle_count == 0


def test_watchdog_resilience_events_have_cooldown() -> None:
    wd = _controller(WatchdogMode.V2)
    first = wd.step(elapsed_sec=10.0, speed_bps=0.0, resilience_event=_Event())
    second = wd.step(elapsed_sec=10.5, speed_bps=0.0, resilience_event=_Event())
    third = wd.step(elapsed_sec=95.0, speed_bps=0.0, resilience_event=_Event())
    assert first.action == WatchdogActionType.RESILIENCE_RECYCLE
    assert second.action == WatchdogActionType.NONE, "cycle storm must be suppressed"
    assert third.action == WatchdogActionType.RESILIENCE_RECYCLE
    assert wd.cycle_count == 2


# ---------------------------------------------------------------------------
# 2. No false 100% on an errored / stalled GID
# ---------------------------------------------------------------------------


def test_error_gid_never_reports_complete_progress() -> None:
    done, total = _safe_progress(
        downloaded=33_304_529, total=33_304_529, all_complete=False, has_error=True
    )
    assert (done, total) == (33_304_528, 33_304_529)


def test_progress_clamped_without_error() -> None:
    done, total = _safe_progress(downloaded=200, total=100, all_complete=True, has_error=False)
    assert (done, total) == (100, 100)


# ---------------------------------------------------------------------------
# 3. Progress uses actual bytes on disk when aria2's counters lag
# ---------------------------------------------------------------------------


def test_disk_progress_reads_part_files(tmp_path: Path) -> None:
    f1 = tmp_path / "a.bin"
    f1.write_bytes(b"x" * 4096)
    items = [
        SimpleNamespace(out_dir=tmp_path, filename="a.bin"),
        SimpleNamespace(out_dir=tmp_path, filename="missing.bin"),
    ]
    assert _disk_progress(items, {}) == 4096


def test_disk_progress_uses_completed_path_override(tmp_path: Path) -> None:
    actual = tmp_path / "resolved.bin"
    actual.write_bytes(b"y" * 8192)
    items = [SimpleNamespace(out_dir=tmp_path, filename="expected.bin")]
    assert _disk_progress(items, {0: actual}) == 8192
