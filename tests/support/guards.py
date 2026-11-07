"""Autouse isolation guards for pdl test suite.

Guards:
1. Loopback-only socket guard: Prevents real WAN / internet socket connections.
2. Subprocess guard: Blocks unauthorized external tools unless test is marked 'macos'.
"""

from __future__ import annotations

import socket
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from tests.support.macos_fake import FakePopen, get_fake_runner

ALLOWED_HOSTS = {"127.0.0.1", "localhost", "::1", "0.0.0.0"}
_ORIGINAL_CONNECT = socket.socket.connect
_ORIGINAL_POPEN = subprocess.Popen
_ORIGINAL_RUN = subprocess.run


def is_loopback(host: str) -> bool:
    if host in ALLOWED_HOSTS:
        return True
    if host.startswith("127."):
        return True
    return False


def guarded_connect(self: socket.socket, address: Any) -> None:
    host: str = ""
    if isinstance(address, tuple | list) and len(address) > 0:
        host = str(address[0])
    elif isinstance(address, str):
        host = address

    if host and not is_loopback(host):
        raise RuntimeError(
            f"External WAN socket connection blocked by pdl test guard: {address}. "
            "Tests must only interact with local loopback or test fakes."
        )
    return _ORIGINAL_CONNECT(self, address)


@contextmanager
def socket_loopback_guard() -> Iterator[None]:
    """Context manager enforcing that socket connections are strictly local loopback."""
    socket.socket.connect = guarded_connect  # type: ignore[assignment]
    try:
        yield
    finally:
        socket.socket.connect = _ORIGINAL_CONNECT  # type: ignore[assignment]


# Binaries intercepted and dispatched to the fake runner unless marked macos
INTERCEPTED_SYSTEM_TOOLS = {
    "pmset",
    "scutil",
    "networksetup",
    "security",
    "codesign",
    "spctl",
    "xattr",
    "osascript",
    "launchctl",
    "caffeinate",
    "ping",
    "traceroute",
    "netstat",
    "nettop",
    "plutil",
}


def make_guarded_run(allow_macos: bool = False) -> Any:
    def _run(args: Any, *run_args: Any, **run_kwargs: Any) -> Any:
        cmd_list = [str(a) for a in (args if isinstance(args, list | tuple) else str(args).split())]
        if cmd_list:
            prog = Path(cmd_list[0]).name
            if prog in INTERCEPTED_SYSTEM_TOOLS:
                if not allow_macos:
                    runner = get_fake_runner()
                    stdin_data = run_kwargs.get("input")
                    check_flag = bool(run_kwargs.get("check", False))
                    fake_res = runner.handle_call(cmd_list, stdin_input=stdin_data, check=check_flag)
                    if check_flag and fake_res.returncode != 0:
                        raise subprocess.CalledProcessError(
                            fake_res.returncode, cmd_list, fake_res.stdout, fake_res.stderr
                        )
                    return subprocess.CompletedProcess(
                        args=cmd_list,
                        returncode=fake_res.returncode,
                        stdout=fake_res.stdout,
                        stderr=fake_res.stderr,
                    )
        return _ORIGINAL_RUN(args, *run_args, **run_kwargs)

    return _run


def make_guarded_popen(allow_macos: bool = False) -> Any:
    def _popen(args: Any, *p_args: Any, **p_kwargs: Any) -> Any:
        cmd_list = [str(a) for a in (args if isinstance(args, list | tuple) else str(args).split())]
        if cmd_list:
            prog = Path(cmd_list[0]).name
            if prog in INTERCEPTED_SYSTEM_TOOLS:
                if not allow_macos:
                    runner = get_fake_runner()
                    fake_res = runner.handle_call(cmd_list)
                    return FakePopen(
                        cmd_list,
                        returncode=fake_res.returncode,
                        stdout=fake_res.stdout,
                        stderr=fake_res.stderr,
                    )
        return _ORIGINAL_POPEN(args, *p_args, **p_kwargs)

    return _popen
