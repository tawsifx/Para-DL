"""End-to-end netprobe and bench tests against the throttling simulator."""

from __future__ import annotations

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.bench import run_bench
from pdl.history import HistoryManager
from pdl.models import ExitCode
from pdl.netprobe import run_netprobe


@pytest.mark.slow
def test_netprobe_and_bench_subcommands() -> None:
    """Verify pdl netprobe classifies throttling & persists to SQLite, and pdl bench compares modes."""
    profile = SimulatorProfile(
        name="per_flow_cap",
        file_size=2 * 1024 * 1024,
        per_flow_bps=120 * 1024,
    )
    with ThrottleTestServer(profile=profile) as srv:
        code, report = run_netprobe(srv.file_url, duration_sec=2.1, http3_probe=False)
        assert code == int(ExitCode.OK)
        assert report.throttle_class == "per-flow cap"
        assert report.recommended_connections == 16

        saved = HistoryManager.get_host_strategy("127.0.0.1")
        assert saved is not None
        assert saved.throttle_class == "per-flow cap"

        bench_code, rows = run_bench(
            srv.file_url,
            modes_str="v1,v2,off",
            duration_sec=1.2,
            connections=4,
            anti_throttle="500KB/s",
            grace_period=3,
        )
        assert bench_code == int(ExitCode.OK)
        assert [r.mode for r in rows] == ["v1", "v2", "off"]
        assert all(r.avg_speed_bps > 0 for r in rows)
