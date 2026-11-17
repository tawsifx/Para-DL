"""Safety guards that keep the test suite hermetic.

``forbid_wan_sockets`` is registered autouse from the root conftest.
``forbid_untracked_subprocess`` is OPT-IN (a named fixture only) so that
legitimate subprocess tests -- notably the mypy regression guard -- keep working.
"""

from __future__ import annotations

import ipaddress
import os
import socket
import subprocess
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any
from unittest.mock import patch

import pytest


def _is_loopback_host(host: str) -> bool:
    if host in ("localhost", "::1") or host.startswith("127."):
        return True
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


def _is_loopback_address(address: Any) -> bool:
    # AF_UNIX connections pass a filesystem path; those are local by definition.
    if isinstance(address, (str, bytes, os.PathLike)):
        return True
    if isinstance(address, tuple) and address:
        return _is_loopback_host(str(address[0]))
    return False


@pytest.fixture(scope="session", autouse=True)
def forbid_wan_sockets() -> Iterator[None]:
    """Block every socket connection whose destination is not loopback.

    Session-scoped so the patch is installed once instead of once per test;
    behavior is identical because the guard must hold for the whole run.
    """
    original_connect = socket.socket.connect
    original_connect_ex = socket.socket.connect_ex

    def guarded_connect(self: socket.socket, address: Any) -> Any:
        if not _is_loopback_address(address):
            raise RuntimeError(f"Blocked non-loopback socket connect to {address!r}")
        return original_connect(self, address)

    def guarded_connect_ex(self: socket.socket, address: Any) -> int:
        if not _is_loopback_address(address):
            raise RuntimeError(f"Blocked non-loopback socket connect_ex to {address!r}")
        return original_connect_ex(self, address)

    with (
        patch.object(socket.socket, "connect", guarded_connect),
        patch.object(socket.socket, "connect_ex", guarded_connect_ex),
    ):
        yield


class SubprocessGuard:
    """Records subprocess calls and rejects any program not on the allowlist."""

    def __init__(self, allowed: frozenset[str] = frozenset()) -> None:
        self.allowed: set[str] = set(allowed)
        self.calls: list[str] = []

    def allow(self, *programs: str) -> None:
        """Add program basenames (e.g. ``'mypy'``) to the allowlist."""
        self.allowed.update(programs)

    @staticmethod
    def _program_of(args: Any, kwargs: dict[str, Any]) -> str:
        target = kwargs.get("args", args[0] if args else None)
        if target is None:
            return "<unknown>"
        if isinstance(target, (str, bytes, os.PathLike)):
            first = os.fspath(target)
        else:
            seq = list(target)
            first = os.fspath(seq[0]) if seq else "<unknown>"
        return Path(first).name

    def _wrap(self, func: Callable[..., Any]) -> Callable[..., Any]:
        def guarded(*args: Any, **kwargs: Any) -> Any:
            program = self._program_of(args, kwargs)
            self.calls.append(program)
            if program not in self.allowed:
                raise AssertionError(
                    f"Untracked subprocess call {program!r}; allowed={sorted(self.allowed)}"
                )
            return func(*args, **kwargs)

        return guarded


@pytest.fixture
def forbid_untracked_subprocess(monkeypatch: pytest.MonkeyPatch) -> SubprocessGuard:
    """Opt-in guard: fail on any subprocess call to a non-allowlisted program.

    Tests that legitimately spawn a process must request this fixture and call
    ``guard.allow('program')`` first. It is deliberately NOT autouse.
    """
    guard = SubprocessGuard()
    for name in ("run", "Popen", "call", "check_call", "check_output"):
        monkeypatch.setattr(subprocess, name, guard._wrap(getattr(subprocess, name)))
    return guard


__all__ = ["SubprocessGuard", "forbid_untracked_subprocess", "forbid_wan_sockets"]
