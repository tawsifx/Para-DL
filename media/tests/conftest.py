"""Shared pytest fixtures for media worker tests (no live network)."""

from __future__ import annotations

from pathlib import Path

import pytest
from paradl_media.toolchain import TOOLCHAIN

FIXTURES = Path(__file__).parent / "fixtures"


@pytest.fixture(autouse=True)
def reset_toolchain_cache():
    """Each test starts with a fresh binary/JS-runtime discovery cache."""
    TOOLCHAIN.reset_cache()
    yield
    TOOLCHAIN.reset_cache()
