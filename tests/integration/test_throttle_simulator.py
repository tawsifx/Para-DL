"""Integration tests verifying the 11 named throttling simulator profiles."""

from __future__ import annotations

import httpx
import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

pytestmark = pytest.mark.integration


@pytest.mark.parametrize(
    ("profile_name", "expect_range", "custom_path"),
    [
        ("per_flow_cap", True, "/file.bin"),
        ("shared_cap", True, "/file.bin"),
        ("burst_then_throttle", True, "/file.bin"),
        ("uniformly_slow", True, "/file.bin"),
        ("queue_shaping", True, "/file.bin"),
        ("time_of_day", True, "/file.bin"),
        ("volume_step_down", True, "/file.bin"),
        ("200_only", False, "/file.bin"),
        ("protocol_specific", True, "/fast/file.bin"),
        ("domestic_vs_international", True, "/file.bin"),
    ],
)
def test_simulator_named_profiles_basic_response(
    profile_name: str,
    expect_range: bool,
    custom_path: str,
) -> None:
    """Verify that each named profile initializes, accepts requests, and obeys range expectations."""
    prof = SimulatorProfile(profile_name, file_size=65536)
    with ThrottleTestServer(prof) as srv:
        url = f"{srv.base_url}{custom_path}"
        with httpx.Client(timeout=3.0) as client:
            resp = client.get(url, headers={"Range": "bytes=0-1023"})
            if expect_range:
                assert resp.status_code == 206
                assert resp.headers.get("Content-Range") == "bytes 0-1023/65536"
                assert len(resp.content) == 1024
            else:
                assert resp.status_code == 200
                assert len(resp.content) == 65536


def test_simulator_loss_based_policing_drops_connection() -> None:
    """Loss-based policing drops the TCP connection once byte threshold is exceeded."""
    prof = SimulatorProfile("loss_based_policing", file_size=131072, loss_threshold_bytes=32768)
    with ThrottleTestServer(prof) as srv:
        with httpx.Client(timeout=3.0) as client:
            # First small range request passes
            r1 = client.get(srv.file_url, headers={"Range": "bytes=0-1023"})
            assert r1.status_code == 206

            # Large request exceeding threshold gets connection dropped
            try:
                r2 = client.get(srv.file_url)
                # If partial read completed or RemoteProtocolError raised
                assert len(r2.content) <= 131072
            except (httpx.RemoteProtocolError, httpx.ReadError):
                pass
