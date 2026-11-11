"""Persistence layer for pdl — SQLite download history and per-host mirror/strategy store.

Provides:
- HistoryManager: append-only download history with querying, filtering, and cleanup.
- MirrorManager: mirror resolution, validation, failure tracking, and metalink support.
- HostStrategy persistence for learned watchdog profiles (throttle class, connections, RTT, loss).
"""

from __future__ import annotations

from paradl.storage.history import HistoryManager
from paradl.storage.mirrors import MetalinkCompanion, MirrorManager

__all__ = [
    "HistoryManager",
    "MetalinkCompanion",
    "MirrorManager",
]
