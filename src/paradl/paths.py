"""Backward-compatible shim: re-exports from paradl.core.paths.

Deprecated: import from paradl.core.paths instead.
"""

from __future__ import annotations

import sys

from paradl.core.paths import (
    get_config_file,
    get_history_db_file,
    get_legacy_history_file,
    get_logs_dir,
    get_mirrors_file,
    get_paradl_home,
    get_targets_file,
    get_throttle_dir,
)


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


__all__ = [
    "get_config_file",
    "get_history_db_file",
    "get_legacy_history_file",
    "get_logs_dir",
    "get_mirrors_file",
    "get_paradl_home",
    "get_targets_file",
    "get_throttle_dir",
    "is_windows",
]
