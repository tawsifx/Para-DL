"""Centralized, deterministic fakes shared across the paradl test tiers.

The clock is a thin subclass of :class:`paradl.core.clock.FakeClock` so there is
a single source of truth for fake-time semantics; the other fakes are small
in-memory stand-ins for keyring and the aria2 JSON-RPC status surface.
"""

from __future__ import annotations

from typing import Any

from paradl.core.clock import FakeClock as _CoreFakeClock
from paradl.models import TargetInfo, TargetType


class FakeClock(_CoreFakeClock):
    """Test-facing alias of the production fake clock (identical semantics).

    Subclassing keeps :mod:`paradl.core.clock` the single source of truth while
    giving migrated tests a stable import path under ``tests.support``.
    """


class FakeKeychain:
    """In-memory stand-in for keyring's ``get/set/delete_password`` API.

    Keys are ``(service, username)`` pairs, matching keyring's addressing model.
    No real keychain or SecretService is touched.
    """

    def __init__(self) -> None:
        self._store: dict[tuple[str, str], str] = {}

    # keyring-compatible API
    def set_password(self, service: str, username: str, password: str) -> None:
        self._store[(service, username)] = password

    def get_password(self, service: str, username: str) -> str | None:
        return self._store.get((service, username))

    def delete_password(self, service: str, username: str) -> None:
        self._store.pop((service, username), None)

    # short aliases used by tests that prefer get/set/delete naming
    def set(self, service: str, username: str, password: str) -> None:
        self.set_password(service, username, password)

    def get(self, service: str, username: str) -> str | None:
        return self.get_password(service, username)

    def delete(self, service: str, username: str) -> None:
        self.delete_password(service, username)

    def clear(self) -> None:
        self._store.clear()


class FakeAria2Server:
    """Deterministic in-memory stand-in for an aria2 JSON-RPC daemon.

    Mirrors the status surface consumed by :class:`paradl.engines.aria2.Aria2Engine`
    (``tell_status`` / ``get_servers`` / ``get_global_stat``) and the small
    control surface needed to drive a watchdog recycle workflow end-to-end.
    No aria2c process or socket is ever created.
    """

    def __init__(
        self,
        *,
        gid: str = "gid-1",
        status: str = "complete",
        completed_length: int = 100,
        total_length: int = 100,
        download_speed: float = 1024,
        files: list[dict[str, Any]] | None = None,
        servers: list[dict[str, Any]] | None = None,
    ) -> None:
        self.gid = gid
        self.status = status
        self.completed_length = completed_length
        self.total_length = total_length
        self.download_speed = download_speed
        self.files: list[dict[str, Any]] = list(files) if files is not None else []
        self.servers: list[dict[str, Any]] = list(servers) if servers is not None else []
        self.calls: list[str] = []

    def update(
        self,
        *,
        status: str | None = None,
        completed_length: int | None = None,
        total_length: int | None = None,
        download_speed: float | None = None,
        files: list[dict[str, Any]] | None = None,
        servers: list[dict[str, Any]] | None = None,
    ) -> None:
        """Mutate only the supplied fields so tests can drive state transitions."""
        if status is not None:
            self.status = status
        if completed_length is not None:
            self.completed_length = completed_length
        if total_length is not None:
            self.total_length = total_length
        if download_speed is not None:
            self.download_speed = download_speed
        if files is not None:
            self.files = list(files)
        if servers is not None:
            self.servers = list(servers)

    def tell_status(self, gid: str | None = None) -> dict[str, Any]:
        self.calls.append("tell_status")
        return {
            "status": self.status,
            "completedLength": self.completed_length,
            "totalLength": self.total_length,
            "files": list(self.files),
        }

    def get_servers(self, gid: str | None = None) -> list[dict[str, Any]]:
        self.calls.append("get_servers")
        return list(self.servers)

    def get_global_stat(self) -> dict[str, Any]:
        self.calls.append("get_global_stat")
        return {"downloadSpeed": self.download_speed}

    def add_uri(self, uris: list[str], options: dict[str, Any]) -> str:
        self.calls.append("add_uri")
        return self.gid

    def force_pause(self, gid: str) -> str:
        self.calls.append("force_pause")
        return gid

    def unpause(self, gid: str) -> str:
        self.calls.append("unpause")
        return gid

    def change_option(self, gid: str, options: dict[str, Any]) -> str:
        self.calls.append("change_option")
        return gid

    def save_session(self) -> None:
        self.calls.append("save_session")

    def stop(self, *, graceful: bool = True) -> None:
        self.calls.append("stop")

    def __enter__(self) -> FakeAria2Server:
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop()


def make_target_info(**overrides: Any) -> TargetInfo:
    """Build a :class:`paradl.models.TargetInfo` mirroring the legacy fixture.

    Any field can be overridden by keyword (e.g. ``filename_hint="x.zip"``).
    The function returns a fresh object every call, so tests never share mutable
    ``headers`` / ``mirrors`` lists.
    """
    defaults: dict[str, Any] = {
        "raw": "https://example.com/file.zip",
        "target_type": TargetType.DIRECT,
        "clean_url": "https://example.com/file.zip",
        "filename_hint": "file.zip",
        "content_length": 1048576,
        "accept_ranges": True,
    }
    defaults.update(overrides)
    return TargetInfo(**defaults)


class NoSleepClock(FakeClock):
    """FakeClock whose sleep() is instantaneous but still recorded.

    Use in unit tests that drive real engine poll loops: removes real waits
    without advancing monotonic time (so watchdog intervals behave as they do
    with a real clock over a few milliseconds of wall time).
    """

    def sleep(self, seconds: float) -> None:
        self.sleep_history.append(max(0.0, float(seconds)))


__all__ = [
    "FakeAria2Server",
    "FakeClock",
    "FakeKeychain",
    "NoSleepClock",
    "make_target_info",
]
