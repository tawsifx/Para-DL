"""Wave-2 Task 2 regression tests: aria2 RPC dedup, thread-safe singletons,
cached yt-dlp extractor patterns, and the daemon-pool mypy regression.

All tests use fakes only -- no real network and no aria2c process.
"""

from __future__ import annotations

import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from paradl.engines.aria2 import Aria2Engine
from paradl.models import TargetInfo, TargetType


class _CountingDaemon:
    """Fake Aria2RpcDaemon that reports `active` for two ticks, then `complete`."""

    def __init__(self, gid: str, out_file: Path) -> None:
        self.gid = gid
        self.out_file = out_file
        self.tell_calls = 0
        self.server_calls = 0
        self.global_stat_calls = 0
        self.current_tick = 0

    def add_uri(self, uris: list[str], options: dict) -> str:
        return self.gid

    def get_global_stat(self) -> dict:
        # Called exactly once per poll tick -- use it to advance the fake clock.
        self.global_stat_calls += 1
        self.current_tick = self.global_stat_calls
        return {"downloadSpeed": "1024"}

    def tell_status(self, gid: str) -> dict:
        self.tell_calls += 1
        if self.current_tick >= 3:
            return {
                "status": "complete",
                "completedLength": "1000",
                "totalLength": "1000",
                "files": [{"path": str(self.out_file)}],
            }
        return {
            "status": "active",
            "completedLength": "500",
            "totalLength": "1000",
            "files": [
                {
                    "path": str(self.out_file),
                    "completedLength": "500",
                    "length": "1000",
                }
            ],
        }

    def get_servers(self, gid: str) -> list[dict]:
        self.server_calls += 1
        return [{"servers": [{"downloadSpeed": "500"}]}]

    def stop(self, *, graceful: bool = True) -> None:
        return None


class _FakeClock:
    def sleep(self, _seconds: float) -> None:
        return None


class _FakeToolchain:
    aria2c = "/usr/bin/aria2c"


def test_one_tell_status_and_get_servers_per_gid_per_tick(tmp_path, monkeypatch):
    """Each active GID must incur exactly one tellStatus + one getServers per tick."""
    from unittest.mock import MagicMock

    from paradl import engines

    out_file = tmp_path / "file.bin"
    out_file.write_bytes(b"x" * 1000)

    daemon = _CountingDaemon("gid-1", out_file)
    fake_pool = MagicMock()
    fake_pool.acquire.return_value = daemon
    fake_toolchain = _FakeToolchain()

    snapshots: list[tuple[int, int]] = []

    def on_progress(downloaded, total, speed, connections):
        snapshots.append((daemon.tell_calls, daemon.server_calls))

    # This test exercises the legacy aria2 control plane (RPC dedup). With the Rust
    # engine as the default, force the aria2 path so the daemon mocks are used.
    from paradl.core import config as core_config

    monkeypatch.setattr(core_config, "get_active_engine", lambda: "aria2")
    monkeypatch.setattr(engines.aria2, "TOOLCHAIN", fake_toolchain)
    monkeypatch.setattr(engines.aria2, "get_daemon_pool", lambda: fake_pool)
    monkeypatch.setattr(engines.aria2, "get_clock", _FakeClock)

    result = Aria2Engine.download(
        TargetInfo(
            raw="https://example.com/file.bin",
            target_type=TargetType.DIRECT,
            clean_url="https://example.com/file.bin",
            filename_hint="file.bin",
            content_length=1000,
            accept_ranges=True,
        ),
        out_dir=tmp_path,
        connections=16,
        write_xattr=False,
        on_progress=on_progress,
    )

    assert result.success, result.error
    # Three ticks: two active, one complete. on_progress is called once per tick.
    assert len(snapshots) >= 2, snapshots
    # First active tick: exactly one of each RPC.
    assert snapshots[0] == (1, 1), snapshots
    # Second active tick: exactly one more of each RPC.
    assert snapshots[1] == (2, 2), snapshots


def test_concurrent_get_http_client_returns_single_instance(monkeypatch):
    """Concurrent first calls to get_http_client() must build one httpx.Client."""
    import httpx

    from paradl.net import client as client_mod

    constructed: list[object] = []

    class _SlowClient:
        def __init__(self, *args, **kwargs):
            time.sleep(0.02)  # widen the race window deterministically
            constructed.append(self)

        def close(self) -> None:
            return None

    monkeypatch.setattr(client_mod, "_SHARED_CLIENT", None)
    monkeypatch.setattr(httpx, "Client", _SlowClient)

    n = 12
    barrier = threading.Barrier(n)

    def worker() -> object:
        barrier.wait()
        return client_mod.get_http_client()

    with ThreadPoolExecutor(max_workers=n) as pool:
        results = list(pool.map(lambda _: worker(), range(n)))

    assert len(constructed) == 1, f"built {len(constructed)} clients"
    assert all(r is constructed[0] for r in results)


def test_concurrent_get_daemon_pool_returns_single_instance(monkeypatch):
    """Concurrent first calls to get_daemon_pool() must build one pool."""
    from paradl.engines import aria2 as aria2_mod

    constructed: list[object] = []

    class _SlowPool:
        def __init__(self) -> None:
            time.sleep(0.02)  # widen the race window deterministically
            constructed.append(self)

    monkeypatch.setattr(aria2_mod, "_DAEMON_POOL", None)
    monkeypatch.setattr(aria2_mod, "Aria2DaemonPool", _SlowPool)

    n = 12
    barrier = threading.Barrier(n)

    def worker() -> object:
        barrier.wait()
        return aria2_mod.get_daemon_pool()

    with ThreadPoolExecutor(max_workers=n) as pool:
        results = list(pool.map(lambda _: worker(), range(n)))

    assert len(constructed) == 1, f"built {len(constructed)} pools"
    assert all(r is constructed[0] for r in results)


def test_gen_extractor_classes_cached_across_claims_url_calls(monkeypatch):
    """gen_extractor_classes() must be invoked once across many claims_url() calls."""
    from yt_dlp import extractor as yt_extractor

    from paradl.net import client as client_mod

    calls: list[int] = []

    class _FakeIE:
        IE_NAME = "examplelive"

        @classmethod
        def suitable(cls, url: str) -> bool:
            return "example.org/watch" in url

    class _GenericIE:
        IE_NAME = "generic"

        @classmethod
        def suitable(cls, url: str) -> bool:
            raise AssertionError("generic extractor must be skipped")

    def fake_gen():
        calls.append(1)
        return (_GenericIE, _FakeIE)

    monkeypatch.setattr(yt_extractor, "gen_extractor_classes", fake_gen)
    cached = getattr(client_mod, "_cached_extractor_classes", None)
    if cached is not None:
        cached.cache_clear()
    try:
        for i in range(10):
            assert client_mod.ytdlp_extractor_claims_url(f"https://example.org/watch?v={i}")
        assert len(calls) == 1, f"gen_extractor_classes called {len(calls)} times"
    finally:
        if cached is not None:
            cached.cache_clear()
