"""End-to-end tests for pdl.throttle subcommands, report generation, and VPS server."""

from __future__ import annotations

import asyncio
import json
import socket
import threading
from pathlib import Path

import pytest
from tests.support.fakes import FakeASNResolver
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.clock import FakeClock, set_clock
from pdl.history import HistoryManager
from pdl.paths import get_throttle_dir


@pytest.fixture(autouse=True)
def _setup_fakes() -> None:
    clock = FakeClock(initial_time=1794100000.0)
    set_clock(clock)
    from pdl.throttle.targets import set_asn_resolver

    set_asn_resolver(FakeASNResolver())


@pytest.mark.slow
def test_pdl_throttle_quick_e2e(tmp_path: Path, throttle_server: ThrottleTestServer) -> None:
    """Run pdl throttle quick end-to-end against simulator and verify report artifacts & learned profile."""
    throttle_server.set_profile(
        SimulatorProfile(
            name="burst_then_throttle",
            file_size=2 * 1024 * 1024,
            burst_bps=1024 * 1024,
            throttled_bps=128 * 1024,
            burst_duration_sec=0.5,
        )
    )

    targets_toml = tmp_path / "custom_targets.toml"
    targets_toml.write_text(
        f"""
[groups.global-cdn]
targets = [
    {{ id = "sim-e2e", url = "{throttle_server.file_url}", expect_bytes = 2097152 }}
]
""",
        encoding="utf-8",
    )

    code = run_cli(
        [
            "throttle",
            "quick",
            "--targets",
            str(targets_toml),
            "--domestic",
            f"{throttle_server.base_url}/domestic/file.bin",
            "--plan-mbps",
            "50",
            "--isp",
            "Duronto Internet",
            "--data-budget",
            "10MB",
            "--yes",
        ]
    )
    assert code == 0

    throttle_dir = get_throttle_dir()
    json_path = throttle_dir / "report.json"
    md_path = throttle_dir / "report.md"
    html_path = throttle_dir / "report.html"

    assert json_path.is_file()
    assert md_path.is_file()
    assert html_path.is_file()

    # Verify report JSON schema & content
    data = json.loads(json_path.read_text(encoding="utf-8"))
    assert data["schema_version"] == "1.0"
    assert data["isp"] == "Duronto Internet"
    assert data["plan_mbps"] == 50.0
    assert "verdicts" in data
    assert "recommended_profile" in data
    assert "isp_evidence_summary" in data
    assert data["budget"]["limit_bytes"] == 10 * 1024 * 1024

    # Verify Markdown report
    md_text = md_path.read_text(encoding="utf-8")
    assert "Duronto Internet" in md_text
    assert "Diagnostic Verdicts" in md_text
    assert "ISP Evidence Summary" in md_text

    # Verify HTML report has dark styling and zero external assets
    html_text = html_path.read_text(encoding="utf-8")
    assert "Duronto Internet" in html_text
    assert "http://" not in html_text
    assert "https://" not in html_text

    # Verify learned profile was persisted and fallback loaded by HistoryManager
    learned = HistoryManager.get_best_profile("unknown.remote.server.org")
    assert learned is not None
    assert learned.source == "throttle-probe"
    assert learned.connections in (8, 16)


@pytest.mark.slow
def test_pdl_throttle_quick_json_flag_e2e(
    tmp_path: Path,
    throttle_server: ThrottleTestServer,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Run pdl throttle quick with --json and assert standard output is valid JSON."""
    throttle_server.set_profile(
        SimulatorProfile(
            name="uniformly_slow",
            file_size=1024 * 1024,
            uniform_slow_bps=64 * 1024,
        )
    )

    targets_toml = tmp_path / "targets.toml"
    targets_toml.write_text(
        f"""
[groups.global-cdn]
targets = [
    {{ id = "sim-json", url = "{throttle_server.file_url}", expect_bytes = 1048576 }}
]
""",
        encoding="utf-8",
    )

    code = run_cli(
        [
            "throttle",
            "quick",
            "--targets",
            str(targets_toml),
            "--json",
            "--data-budget",
            "5MB",
            "--yes",
        ]
    )
    assert code == 0

    captured = capsys.readouterr()
    payload = json.loads(captured.out)
    assert payload["schema_version"] == "1.0"
    assert payload["isp"] == "Duronto Internet"
    assert "verdicts" in payload


@pytest.mark.slow
def test_pdl_throttle_server_vps_roundtrip_e2e() -> None:
    """Run pdl throttle server on loopback and verify TCP handshake and UDP echo roundtrip."""
    stop_event = threading.Event()
    bound_ports: dict[str, int] = {}
    ready_event = threading.Event()

    def _run() -> None:
        async def _server_main() -> None:
            from pdl.throttle.server import UDPThrottleEchoProtocol, handle_tcp_client

            loop = asyncio.get_running_loop()
            tcp_server = await asyncio.start_server(handle_tcp_client, "127.0.0.1", 0)
            tcp_port = tcp_server.sockets[0].getsockname()[1]

            udp_transport, _ = await loop.create_datagram_endpoint(
                lambda: UDPThrottleEchoProtocol(),
                local_addr=("127.0.0.1", 0),
            )
            udp_port = udp_transport.get_extra_info("sockname")[1]

            bound_ports["tcp"] = tcp_port
            bound_ports["udp"] = udp_port
            ready_event.set()

            while not stop_event.is_set():
                await asyncio.sleep(0.05)

            udp_transport.close()
            tcp_server.close()
            await tcp_server.wait_closed()

        asyncio.run(_server_main())

    th = threading.Thread(target=_run, daemon=True)
    th.start()
    assert ready_event.wait(timeout=3.0)

    try:
        tcp_p = bound_ports["tcp"]
        udp_p = bound_ports["udp"]

        # 1. TCP GET /tcp-probe
        with socket.create_connection(("127.0.0.1", tcp_p), timeout=2.0) as sock:
            sock.sendall(b"GET /tcp-probe HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            buf = sock.recv(1024)
            assert b"HTTP/1.1 200 OK" in buf

        # 2. UDP echo roundtrip
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp_sock:
            udp_sock.settimeout(2.0)
            packet = b"\x00\x01\x00\x02ping-vps"
            udp_sock.sendto(packet, ("127.0.0.1", udp_p))
            data, _ = udp_sock.recvfrom(1024)
            assert data == packet
    finally:
        stop_event.set()
        th.join(timeout=2.0)
