"""Integration tests for learned profiles, 30-day staleness revalidation, and backward compatibility toggles."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.engines.watchdog import WatchdogActionType, WatchdogController
from pdl.events import ResilienceEventSource
from pdl.history import HistoryManager, LearnedProfile
from pdl.models import ExitCode, WatchdogMode
from pdl.ui import UI

pytestmark = pytest.mark.integration


def test_learned_profile_selection_staleness_revalidation_and_reset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Verify profile selection by (host, hour_bucket, network_id), 30-day staleness
    ramp-up re-validation (pass and fail), and `pdl profiles` / `pdl profiles reset <host>`.
    """
    now_ts = 1700000000.0
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))

    # Save profiles into SQLite
    p1 = LearnedProfile(
        host="cdn.example.com",
        hour_bucket=2,
        network_id="HomeWiFi",
        connections=8,
        ip_family="ipv4",
        recovery_action="tier2_pause_unpause",
        speed_before=400000.0,
        speed_after=8000000.0,
        supports_range=True,
        updated_epoch=now_ts - 3600.0,  # 1 hour old (fresh)
    )
    p2 = LearnedProfile(
        host="cdn.example.com",
        hour_bucket=5,
        network_id="CafeHotspot",
        connections=4,
        ip_family="ipv6",
        recovery_action="adjust_connections",
        speed_before=200000.0,
        speed_after=2500000.0,
        supports_range=True,
        updated_epoch=now_ts - 1800.0,
    )
    HistoryManager.save_learned_profile(p1)
    HistoryManager.save_learned_profile(p2)

    # 1. Selection by (host, hour_bucket, network_id)
    p_home = HistoryManager.get_best_profile("cdn.example.com", hour_bucket=2, network_id="HomeWiFi")
    assert p_home is not None
    assert p_home.connections == 8
    assert p_home.ip_family == "ipv4"
    assert p_home.is_stale(now_epoch=now_ts) is False

    # 2. Watchdog controller with fresh profile
    wd_fresh = WatchdogController(
        mode=WatchdogMode.V2,
        user_floor_bps=500000.0,
        connections=16,
        host="cdn.example.com",
        hour_bucket=2,
        network_id="HomeWiFi",
        now_epoch=now_ts,
    )
    assert wd_fresh.revalidation_pending is False
    assert wd_fresh.revalidation_passed is True

    # 3. Staleness re-validation (>30 days old)
    p_stale = LearnedProfile(
        host="stale.example.com",
        hour_bucket=3,
        network_id="OfficeLAN",
        connections=6,
        ip_family="ipv4",
        recovery_action="adjust_connections",
        speed_before=300000.0,
        speed_after=4000000.0,
        supports_range=True,
        updated_epoch=now_ts - 35 * 86400.0,  # 35 days old
    )
    HistoryManager.save_learned_profile(p_stale)
    loaded_stale = HistoryManager.get_best_profile("stale.example.com", hour_bucket=3, network_id="OfficeLAN")
    assert loaded_stale is not None
    assert loaded_stale.is_stale(now_epoch=now_ts) is True

    # 3a. Re-validation FAIL: transfer starts slower than expected
    wd_stale_fail = WatchdogController(
        mode=WatchdogMode.V2,
        user_floor_bps=500000.0,
        connections=6,
        host="stale.example.com",
        hour_bucket=3,
        network_id="OfficeLAN",
        revalidation_ticks=2,
        now_epoch=now_ts,
    )
    assert wd_stale_fail.revalidation_pending is True
    dec1 = wd_stale_fail.step(elapsed_sec=1.0, speed_bps=200000.0)
    assert dec1.action == WatchdogActionType.NONE
    dec2 = wd_stale_fail.step(elapsed_sec=2.0, speed_bps=220000.0)
    assert dec2.action == WatchdogActionType.ADJUST_CONNECTIONS
    assert wd_stale_fail.revalidation_pending is False
    assert wd_stale_fail.revalidation_passed is False

    # 3b. Re-validation PASS
    wd_stale_pass = WatchdogController(
        mode=WatchdogMode.V2,
        user_floor_bps=500000.0,
        connections=6,
        host="stale.example.com",
        hour_bucket=3,
        network_id="OfficeLAN",
        revalidation_ticks=2,
        now_epoch=now_ts,
    )
    wd_stale_pass.step(elapsed_sec=1.0, speed_bps=3500000.0)
    dec_pass = wd_stale_pass.step(elapsed_sec=2.0, speed_bps=420000.0)
    assert dec_pass.action == WatchdogActionType.NONE
    assert wd_stale_pass.revalidation_passed is True

    # 4. CLI: pdl profiles & pdl profiles reset <host>
    events: list[dict[str, Any]] = []
    monkeypatch.setattr(UI, "emit_event", lambda ev, **kw: events.append({"event": ev, **kw}))

    code_list = run_cli(["profiles", "--json"])
    assert code_list == int(ExitCode.OK)
    list_ev = next(e for e in events if e.get("event") == "profiles")
    assert list_ev.get("count", 0) >= 2

    code_reset_host = run_cli(["profiles", "reset", "cdn.example.com", "--json"])
    assert code_reset_host == int(ExitCode.OK)
    assert len(HistoryManager.list_learned_profiles(host="cdn.example.com")) == 0


def test_all_new_toggles_off_matches_phase6_behavior(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify that passing --no-caffeinate --no-net-watch --stall-timeout 0
    preserves exact Phase 6 behavior and completes cleanly.
    """
    events: list[dict[str, Any]] = []
    monkeypatch.setattr(UI, "emit_event", lambda ev, **kw: events.append({"event": ev, **kw}))

    with ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as srv:
        out_dir = tmp_path / "phase6_parity"
        code = run_cli(
            [
                srv.file_url,
                "-o",
                str(out_dir),
                "-c",
                "4",
                "--no-caffeinate",
                "--no-net-watch",
                "--stall-timeout",
                "0",
                "--json",
            ]
        )
        assert code == int(ExitCode.OK)
        res_ev = next(e for e in events if e.get("event") == "result")
        assert res_ev.get("success") is True
        assert res_ev.get("recycle_count") == 0
        assert res_ev.get("checksum_verified") is True
        assert res_ev.get("checksum_digest") == srv.sha256_hex

    ev_off = ResilienceEventSource(net_watch=False, stall_timeout_sec=0.0, wake_watch=False)
    ev_off.simulate_net_change("scutil: changed key")
    assert ev_off.tick(downloaded_bytes=100.0) is None
