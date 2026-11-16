"""End-to-end / slow regression guards.

Holds the tier's ``@pytest.mark.slow`` tests: the real-mypy daemon-pool guard,
the Windows doctor audit, and a full watchdog recycle workflow that composes
the fake clock with the fake aria2 RPC server (zero real sleeps).
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest
from tests.support.fakes import FakeAria2Server, FakeClock, make_target_info

from paradl.core.models import WatchdogMode
from paradl.engines.watchdog import WatchdogActionType, WatchdogController, WatchdogDecision
from paradl.ui import UI


@pytest.mark.slow
def test_aria2_has_no_mypy_daemon_attribute_error():
    """The Wave-1 ``list[_DaemonPoolEntry]`` attribute regression must be gone."""
    # This file lives in tests/e2e/, so the repo root is parents[2] (e2e -> tests -> root).
    src = Path(__file__).resolve().parents[2] / "src"
    repo = src.parent
    env = dict(os.environ, PYTHONPATH=str(src))
    proc = subprocess.run(
        [sys.executable, "-m", "mypy", str(src / "paradl" / "engines" / "aria2.py")],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(repo),
        check=False,
    )
    combined = proc.stdout + proc.stderr
    if "No module named mypy" in combined or proc.returncode not in (0, 1):
        pytest.skip(f"mypy unavailable: {combined[:400]}")
    assert 'has no attribute "daemon"' not in combined, combined
    assert "list[_DaemonPoolEntry]" not in combined, combined


@pytest.mark.slow
def test_doctor_audits_windows_utilities_on_windows() -> None:
    """Verify doctor checks Windows tools on Windows and does not fail on macOS tools."""
    from paradl import doctor

    fake_win_utils = {
        "powershell": {
            "available": True,
            "path": "C:\\Windows\\System32\\powershell.exe",
            "fix": None,
            "required": True,
        },
        "curl": {
            "available": True,
            "path": "C:\\Windows\\System32\\curl.exe",
            "fix": None,
            "required": True,
        },
        "tar": {
            "available": True,
            "path": "C:\\Windows\\System32\\tar.exe",
            "fix": None,
            "required": False,
        },
        "winget": {
            "available": True,
            "path": "C:\\Users\\Alice\\AppData\\Local\\Microsoft\\WinGet\\Links\\winget.exe",
            "fix": None,
            "required": False,
        },
    }

    with (
        patch("paradl.diagnostics.doctor.is_windows", return_value=True),
        patch(
            "paradl.diagnostics.doctor.check_platform_utilities",
            return_value=fake_win_utils,
        ),
    ):
        UI.json_mode = True
        try:
            ret = doctor.run_doctor()
            assert ret == 0
        finally:
            UI.json_mode = False


@pytest.mark.slow
def test_watchdog_recycle_workflow_end_to_end(monkeypatch: pytest.MonkeyPatch) -> None:
    """Compose fake time + fake aria2 RPC into one full watchdog recycle workflow.

    Drives a real ``WatchdogController`` across a multi-second window using
    ``FakeClock`` (no real sleeps). A ``FakeAria2Server`` supplies per-tick
    progress/status and receives the force-pause/unpause recycle commands. A
    ``watchdog_action`` event is emitted through the real ``UI.emit_event``
    surface (captured by a spy). The composed state sequence must be
    Active -> Recycling -> Active.
    """
    clock = FakeClock()
    target = make_target_info(filename_hint="file.bin", content_length=1_000_000)
    server = FakeAria2Server(
        gid="gid-e2e",
        status="active",
        completed_length=0,
        total_length=target.content_length,
        download_speed=200_000.0,
        servers=[{"servers": [{"downloadSpeed": "200000"}]}],
    )
    watchdog = WatchdogController(
        mode=WatchdogMode.V2,
        user_floor_bps=100_000.0,
        grace_period=2,
        connections=16,
        supports_range=True,
        min_cooldown_sec=2.0,
        recycle_interval_sec=5.0,
        hour_bucket=0,
        network_id="e2e-test-net",
        apply_learned_profile=False,
    )

    emitted: list[tuple[str, dict[str, Any]]] = []

    def _capture(event_type: str, **payload: Any) -> None:
        emitted.append((event_type, payload))

    monkeypatch.setattr("paradl.ui.UI.emit_event", _capture)

    states: list[str] = []
    observed_statuses: list[str] = []
    paused_snapshot: dict[str, Any] = {}
    t0 = clock.monotonic()

    def _tick(elapsed_sec: float) -> WatchdogDecision:
        nonlocal paused_snapshot
        # Simulate multi-second elapsed time via the fake clock: ZERO real sleeps.
        clock.advance(elapsed_sec - (clock.monotonic() - t0))
        # Per-tick progress/status supplied by the fake aria2 daemon.
        stat = server.get_global_stat()
        status = server.tell_status(server.gid)
        observed_statuses.append(str(status["status"]))
        per_conn = [float(s["downloadSpeed"]) for s in server.get_servers()[0]["servers"]]
        decision = watchdog.step(
            elapsed_sec=clock.monotonic() - t0,
            speed_bps=float(stat["downloadSpeed"]),
            per_conn_speeds=per_conn,
        )
        if decision.action != WatchdogActionType.NONE:
            states.append("Recycling")
            # Emit the recycle event through the real UI surface (mirrors aria2.py).
            UI.emit_event(
                "watchdog_action",
                cycle=decision.cycle,
                mode=watchdog.mode.value,
                action=decision.action.value,
                connections=watchdog.connections,
            )
            # Execute the recycle against the fake daemon.
            server.force_pause(server.gid)
            server.update(status="paused")
            paused_snapshot = server.tell_status(server.gid)
            clock.sleep(0.15)  # fake pause/unpause gap -- no real sleep
            server.unpause(server.gid)
            server.update(status="active")
        else:
            states.append("Active" if watchdog.is_active else "Inactive")
        return decision

    # Active: healthy traffic below the periodic recycle interval.
    assert watchdog.is_active
    assert _tick(0.0).action == WatchdogActionType.NONE

    # Recycling: after >5 s of fake elapsed time the periodic socket cycle fires.
    recycle = _tick(5.5)
    assert recycle.action == WatchdogActionType.PAUSE_UNPAUSE
    assert recycle.cycle == 1

    # Active again: cooldown elapsed, monitoring resumes without another cycle.
    assert _tick(8.0).action == WatchdogActionType.NONE
    assert watchdog.is_active

    # End-to-end composition assertions (no single unit-assertion duplicate).
    assert states == ["Active", "Recycling", "Active"]
    watchdog_events = [payload for name, payload in emitted if name == "watchdog_action"]
    assert len(watchdog_events) == 1
    assert watchdog_events[0]["action"] == WatchdogActionType.PAUSE_UNPAUSE.value

    # FakeAria2Server was actually exercised for progress/status and controls.
    assert "get_global_stat" in server.calls
    assert "tell_status" in server.calls
    assert "get_servers" in server.calls
    assert server.calls.count("force_pause") == 1
    assert server.calls.count("unpause") == 1
    assert paused_snapshot["status"] == "paused"
    assert observed_statuses == ["active", "active", "active"]

    # Multi-second time was simulated entirely with the fake clock.
    assert clock.monotonic() - t0 == pytest.approx(8.0)
    assert 0.15 in clock.sleep_history
