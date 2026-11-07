"""End-to-end resilience tests (clock jump / wake, network change, socket stall)."""

from __future__ import annotations

import hashlib
import threading
import time
from pathlib import Path

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.engines.aria2 import Aria2Engine
from pdl.events import ResilienceEventSource
from pdl.models import TargetInfo, TargetType, WatchdogMode


@pytest.mark.slow
def test_simulator_clock_jump_triggers_single_debounced_recycle_and_completes(tmp_path: Path) -> None:
    """In the simulator, a simulated clock jump (burst) triggers exactly one recycle (debounced)
    and the download still completes with a correct SHA-256 hash.
    """
    profile = SimulatorProfile(
        name="per_flow_cap",
        file_size=1536 * 1024,
        per_flow_bps=240 * 1024,
    )
    ev_src = ResilienceEventSource(
        net_watch=False,
        stall_timeout_sec=30.0,
        wake_jump_threshold_sec=3.0,
        debounce_sec=2.0,
    )

    def _inject_wake_burst() -> None:
        time.sleep(0.35)
        ev_src.simulate_clock_jump(45.0)
        time.sleep(0.10)
        ev_src.simulate_clock_jump(15.0)

    injector = threading.Thread(target=_inject_wake_burst, daemon=True)

    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.file_url,
            filename_hint="wake_sim.bin",
            content_length=profile.file_size,
            accept_ranges=True,
            etag_sha256=srv.sha256_hex,
            expected_digest=f"sha256:{srv.sha256_hex}",
        )
        injector.start()
        res = Aria2Engine.download(
            t_info,
            tmp_path / "wake_out",
            connections=4,
            anti_throttle="50KB/s",
            grace_period=10,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.15,
            min_cooldown_sec=5.0,
            event_source=ev_src,
        )
        injector.join(timeout=1.0)

        assert res.success is True
        assert res.recycle_count == 1
        assert res.checksum_verified is True
        assert res.file_path is not None
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == srv.sha256_hex


@pytest.mark.slow
def test_simulator_net_change_burst_triggers_single_debounced_recycle_and_completes(tmp_path: Path) -> None:
    """In the simulator, a burst of simulated network-change notifications triggers
    exactly one recycle (debounced) and the download completes with a verified hash.
    """
    profile = SimulatorProfile(
        name="per_flow_cap",
        file_size=1536 * 1024,
        per_flow_bps=240 * 1024,
    )
    ev_src = ResilienceEventSource(
        net_watch=True,
        stall_timeout_sec=30.0,
        debounce_sec=2.0,
    )

    def _inject_net_burst() -> None:
        time.sleep(0.35)
        ev_src.simulate_net_change("scutil: changed key [0] = State:/Network/Global/IPv4")
        ev_src.simulate_net_change("scutil: changed key [1] = State:/Network/Interface/en0/AirPort")
        time.sleep(0.08)
        ev_src.simulate_net_change("scutil: changed key [0] = State:/Network/Interface/en0/IPv4")
        time.sleep(0.08)
        ev_src.simulate_net_change("scutil: changed key [0] = State:/Network/Global/IPv6")

    injector = threading.Thread(target=_inject_net_burst, daemon=True)

    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.file_url,
            filename_hint="netchange_sim.bin",
            content_length=profile.file_size,
            accept_ranges=True,
            etag_sha256=srv.sha256_hex,
            expected_digest=f"sha256:{srv.sha256_hex}",
        )
        injector.start()
        res = Aria2Engine.download(
            t_info,
            tmp_path / "net_out",
            connections=4,
            anti_throttle="50KB/s",
            grace_period=10,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.15,
            min_cooldown_sec=5.0,
            event_source=ev_src,
        )
        injector.join(timeout=1.0)

        assert res.success is True
        assert res.recycle_count == 1
        assert res.checksum_verified is True
        assert res.file_path is not None
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == srv.sha256_hex


@pytest.mark.slow
def test_simulator_stall_triggers_single_debounced_recycle_and_completes(tmp_path: Path) -> None:
    """In the simulator, a mid-download stall triggers exactly one recycle (debounced)
    and resumes on new sockets to complete with a valid SHA-256 hash.
    """
    profile = SimulatorProfile(
        name="stall_until_recycle",
        file_size=1024 * 1024,
        stall_after_bytes=192 * 1024,
    )
    ev_src = ResilienceEventSource(
        net_watch=False,
        stall_timeout_sec=0.65,
        debounce_sec=2.0,
    )

    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.file_url,
            filename_hint="stall_sim.bin",
            content_length=profile.file_size,
            accept_ranges=True,
            etag_sha256=srv.sha256_hex,
            expected_digest=f"sha256:{srv.sha256_hex}",
        )
        res = Aria2Engine.download(
            t_info,
            tmp_path / "stall_out",
            connections=4,
            anti_throttle="10KB/s",
            grace_period=15,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.15,
            min_cooldown_sec=5.0,
            stall_timeout=0.65,
            event_source=ev_src,
        )

        assert res.success is True
        assert res.recycle_count == 1
        assert res.checksum_verified is True
        assert res.file_path is not None
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == srv.sha256_hex
