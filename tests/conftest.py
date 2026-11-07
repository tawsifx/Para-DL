"""Root conftest for pdl test suite.

Installs autouse isolation guards:
1. Temp isolated PDL_HOME directory for every test.
2. Loopback-only socket guard preventing real WAN calls.
3. Subprocess guard intercepting external macOS system CLI calls.
4. Resets caches, clock, and failure state between tests.
5. Provides shared session-scoped worker ThrottleTestServer.
"""

from __future__ import annotations

import socket
import subprocess
from collections.abc import Iterator
from pathlib import Path

import pytest

from pdl.clock import reset_clock
from pdl.engines.github import GitHubEngine
from pdl.mirrors import MirrorManager
from pdl.platform import macos
from pdl.toolchain import TOOLCHAIN
from pdl.ui import UI
from tests.support.guards import (
    _ORIGINAL_CONNECT,
    _ORIGINAL_POPEN,
    _ORIGINAL_RUN,
    guarded_connect,
    make_guarded_popen,
    make_guarded_run,
)
from tests.support.macos_fake import get_fake_runner
from tests.support.server import SimulatorProfile, ThrottleTestServer


def pytest_configure(config: pytest.Config) -> None:
    """Register custom markers."""
    config.addinivalue_line("markers", "unit: Fast pure logic unit test without external I/O")
    config.addinivalue_line("markers", "integration: Simulator-backed test using local sockets & SQLite")
    config.addinivalue_line("markers", "slow: Long-running end-to-end benchmark or flow")
    config.addinivalue_line("markers", "macos: Test requiring real host macOS execution")


@pytest.fixture(autouse=True)
def isolated_pdl_home(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    request: pytest.FixtureRequest,
) -> Iterator[Path]:
    """Isolate PDL_HOME, loopback network, subprocess calls, and global state."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))

    UI.quiet = True
    UI.json_mode = False
    UI.non_interactive = False
    macos.set_noop_fallback(True)
    GitHubEngine.reset_token_cache()
    MirrorManager.reset_failures()
    TOOLCHAIN.reset_cache()
    reset_clock()

    # Reset fake runner state
    fake_runner = get_fake_runner()
    fake_runner.recorded_calls.clear()
    fake_runner.keychain_store.clear()
    fake_runner.xattr_store.clear()
    fake_runner.notifications.clear()
    fake_runner.battery_status = "ac"
    fake_runner.low_power_mode = False
    fake_runner.codesign_valid = True
    fake_runner.spctl_valid = True

    # Check if test has 'macos' marker
    is_macos_marked = request.node.get_closest_marker("macos") is not None

    # Install guards
    socket.socket.connect = guarded_connect  # type: ignore[assignment]
    subprocess.run = make_guarded_run(allow_macos=is_macos_marked)
    subprocess.Popen = make_guarded_popen(allow_macos=is_macos_marked)

    try:
        yield pdl_home
    finally:
        socket.socket.connect = _ORIGINAL_CONNECT  # type: ignore[assignment]
        subprocess.run = _ORIGINAL_RUN
        subprocess.Popen = _ORIGINAL_POPEN
        MirrorManager.reset_failures()
        macos.set_noop_fallback(False)
        reset_clock()


@pytest.fixture(scope="session")
def session_throttle_server(tmp_path_factory: pytest.TempPathFactory) -> Iterator[ThrottleTestServer]:
    """Session-scoped (or xdist worker scoped) throttle server instance."""
    server = ThrottleTestServer(SimulatorProfile("none", file_size=1024 * 1024))
    server.start()
    try:
        yield server
    finally:
        server.stop()


@pytest.fixture
def throttle_server(session_throttle_server: ThrottleTestServer) -> ThrottleTestServer:
    """Per-test wrapper providing a cleanly reset throttle server."""
    session_throttle_server.reset()
    session_throttle_server.set_profile(SimulatorProfile("none", file_size=1024 * 1024))
    return session_throttle_server
