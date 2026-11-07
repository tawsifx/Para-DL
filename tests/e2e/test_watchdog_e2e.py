"""End-to-end watchdog tests against the unified local throttling simulator."""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.engines.aria2 import Aria2Engine
from pdl.models import TargetInfo, TargetType, WatchdogMode


@pytest.mark.slow
def test_watchdog_v2_beats_v1_and_off_on_burst_then_throttle(tmp_path: Path) -> None:
    """Prove Watchdog v2 beats v1 on burst-then-throttle, recycles sockets,
    and resumes to match exact SHA-256.
    """
    profile = SimulatorProfile(
        name="burst_then_throttle",
        file_size=4 * 1024 * 1024,
        burst_bps=1200 * 1024,
        throttled_bps=55 * 1024,
        burst_duration_sec=0.65,
    )

    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.file_url,
            filename_hint="burst_test.bin",
            content_length=profile.file_size,
            accept_ranges=True,
            etag_sha256=srv.sha256_hex,
            expected_digest=f"sha256:{srv.sha256_hex}",
        )

        out_v2 = tmp_path / "v2"
        res_v2 = Aria2Engine.download(
            t_info,
            out_v2,
            connections=4,
            anti_throttle="150KB/s",
            grace_period=3,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.2,
            min_cooldown_sec=0.8,
        )

        out_v1 = tmp_path / "v1"
        res_v1 = Aria2Engine.download(
            t_info,
            out_v1,
            connections=4,
            anti_throttle="150KB/s",
            grace_period=3,
            watchdog_mode=WatchdogMode.V1,
            poll_interval=0.2,
            min_cooldown_sec=0.8,
        )

        out_off = tmp_path / "off"
        res_off = Aria2Engine.download(
            t_info,
            out_off,
            connections=4,
            anti_throttle=None,
            watchdog_mode=WatchdogMode.OFF,
            poll_interval=0.2,
        )

        assert res_v2.success is True
        assert res_v2.checksum_verified is True
        assert res_v2.file_path is not None
        assert hashlib.sha256(res_v2.file_path.read_bytes()).hexdigest() == srv.sha256_hex
        assert res_v2.recycle_count >= 1

        assert res_v1.success is True
        assert res_v1.checksum_verified is True
        assert res_off.success is True

        assert res_v1.avg_speed_bps > res_off.avg_speed_bps * 1.25
        assert res_v2.avg_speed_bps > res_off.avg_speed_bps * 1.25


@pytest.mark.slow
def test_watchdog_v2_does_not_thrash_on_uniformly_slow_server(tmp_path: Path) -> None:
    """On a uniformly slow server where recycling does not improve speed,
    Watchdog v2 must escalate with exponential backoff and cap recycles.
    """
    profile = SimulatorProfile(
        name="uniformly_slow",
        file_size=512 * 1024,
        uniform_slow_bps=220 * 1024,
    )
    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.file_url,
            filename_hint="slow.bin",
            content_length=profile.file_size,
            accept_ranges=True,
            etag_sha256=srv.sha256_hex,
            expected_digest=f"sha256:{srv.sha256_hex}",
        )
        res = Aria2Engine.download(
            t_info,
            tmp_path / "slow_out",
            connections=4,
            anti_throttle="5MB/s",
            grace_period=3,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.15,
            min_cooldown_sec=0.6,
        )
        assert res.success is True
        assert res.checksum_verified is True
        assert res.recycle_count <= 2


@pytest.mark.slow
def test_range_probe_disables_recycling_when_server_returns_200(tmp_path: Path) -> None:
    """Detect up front whether server honors ranges.
    If not, disable recycling automatically and complete download cleanly.
    """
    profile = SimulatorProfile(
        name="200_only",
        file_size=256 * 1024,
        support_range=False,
    )
    with ThrottleTestServer(profile=profile) as srv:
        t_info = TargetInfo(
            raw=srv.no_range_url,
            target_type=TargetType.DIRECT,
            clean_url=srv.no_range_url,
            filename_hint="norange.bin",
        )
        res = Aria2Engine.download(
            t_info,
            tmp_path / "norange_out",
            connections=8,
            anti_throttle="10MB/s",
            grace_period=3,
            watchdog_mode=WatchdogMode.V2,
            poll_interval=0.15,
        )
        assert res.success is True
        assert res.recycle_count == 0
        assert res.file_path is not None
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == srv.sha256_hex
