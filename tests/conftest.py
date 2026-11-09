"""Shared pytest fixtures and configuration for the pdl test suite."""

from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# Ensure src/pdl is importable without installation
SRC_DIR = Path(__file__).resolve().parent.parent / "src"
if str(SRC_DIR) not in sys.path:
    sys.path.insert(0, str(SRC_DIR))


@pytest.fixture(autouse=True)
def isolate_pdl_home():
    """Force PDL_HOME to a temp directory for each test to avoid polluting user state."""
    with tempfile.TemporaryDirectory(prefix="pdl_test_home_") as tmp:
        with patch.dict(os.environ, {"PDL_HOME": tmp}, clear=False):
            yield Path(tmp)


@pytest.fixture
def temp_config_file():
    """Provide a temporary config file path in an isolated PDL_HOME."""
    pass


@pytest.fixture
def mock_httpx_client():
    """Provide a mock httpx.Client for network-layer tests."""
    with patch("pdl.net.get_http_client") as mock:
        client = MagicMock()
        mock.return_value = client
        yield client


@pytest.fixture
def mock_aria2_daemon():
    """Provide a mock Aria2RpcDaemon for engine tests."""
    with patch("pdl.engines.aria2.Aria2RpcDaemon") as mock:
        daemon = MagicMock()
        mock.return_value = daemon
        daemon.__enter__.return_value = daemon
        daemon.__exit__.return_value = None
        daemon.call.return_value = {"result": "ok"}
        daemon.tell_status.return_value = {"status": "complete", "completedLength": 100, "totalLength": 100}
        daemon.get_global_stat.return_value = {"downloadSpeed": 1024}
        daemon.get_servers.return_value = []
        yield daemon


@pytest.fixture
def sample_target_info():
    """Provide a sample TargetInfo for direct download tests."""
    from pdl.models import TargetInfo, TargetType

    return TargetInfo(
        raw="https://example.com/file.zip",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.zip",
        filename_hint="file.zip",
        content_length=1048576,
        accept_ranges=True,
    )


@pytest.fixture(autouse=True)
def reset_isp_cache():
    """Reset ISP detection cache between tests."""
    from pdl.net import reset_isp_detector, _CACHED_ISP

    _CACHED_ISP = None
    reset_isp_detector()
    yield
    _CACHED_ISP = None
    reset_isp_detector()
