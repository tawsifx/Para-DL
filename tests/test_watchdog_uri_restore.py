"""Regression: watchdog pause/unpause must restore aria2's URI pool."""

from __future__ import annotations

from types import SimpleNamespace

from paradl.engines.aria2 import _restore_uris_after_recycle


class _Daemon:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, object]] = []

    def change_uri(self, gid: str, uris: list[str]) -> str:
        self.calls.append(("change_uri", gid, uris))
        return gid


def test_recycle_restores_redirected_uri() -> None:
    daemon = _Daemon()
    item = SimpleNamespace(
        url="https://github.com/x/archive/main.zip", mirrors=["https://mirror.example/x.zip"]
    )
    _restore_uris_after_recycle(daemon, {"gid1": 0}, [item])
    assert daemon.calls == [
        (
            "change_uri",
            "gid1",
            ["https://github.com/x/archive/main.zip", "https://mirror.example/x.zip"],
        )
    ]


def test_recycle_restores_all_active_gids() -> None:
    daemon = _Daemon()
    items = [SimpleNamespace(url=f"https://h/{i}.zip", mirrors=[]) for i in range(2)]
    _restore_uris_after_recycle(daemon, {"g0": 0, "g1": 1}, items)
    assert [c[1] for c in daemon.calls] == ["g0", "g1"]
