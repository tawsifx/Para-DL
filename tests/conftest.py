"""Shared pytest fixtures and configuration for the paradl test suite."""

from __future__ import annotations

import os
import tempfile
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# Autouse hermeticity guard: block every non-loopback socket connection.
from tests.support.guards import forbid_wan_sockets  # noqa: F401

from paradl.net import reset_isp_detector


@pytest.fixture(autouse=True)
def isolate_paradl_home():
    """Force PARADL_HOME to a temp directory for each test to avoid polluting user state."""
    with (
        tempfile.TemporaryDirectory(prefix="paradl_test_home_") as tmp,
        patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False),
    ):
        yield Path(tmp)


@pytest.fixture
def temp_config_file():
    """Provide a temporary config file path in an isolated PARADL_HOME."""
    pass


@pytest.fixture
def mock_httpx_client():
    """Provide a mock httpx.Client for network-layer tests."""
    with patch("paradl.net.get_http_client") as mock:
        client = MagicMock()
        mock.return_value = client
        yield client


@pytest.fixture
def sample_target_info():
    """Provide a sample TargetInfo for direct download tests."""
    from paradl.models import TargetInfo, TargetType

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
    reset_isp_detector()
    yield
    reset_isp_detector()
