"""Backward-compatible shim: re-exports from paradl.storage.history.

Deprecated: import from paradl.storage.history instead.
"""

from __future__ import annotations

from paradl.storage.history import (
    HistoryManager,
    HostStrategy,
    compute_hour_bucket,
)

__all__ = ["HistoryManager", "HostStrategy", "compute_hour_bucket"]
