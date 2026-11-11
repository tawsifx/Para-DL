"""Resilience and recovery layer for pdl — watchdog, resilience events, hooks, and queue.

Provides:
- ResilienceEvent / ResilienceEventSource (WAKE, NET_CHANGE, STALL detection).
- Start-up guards: free-disk-space and power-policy checks.
- WatchdogController (anti-throttle v1/v2 with pause/unpause socket cycling).
- HookRunner for result and event notifications.
- Persistent SQLite download queue with --when-fast / --at scheduling.

This layer is used by the Aria2 engine and the application orchestrator.
"""

from __future__ import annotations

from paradl.resilience.events import (
    ResilienceEvent,
    ResilienceEventSource,
    ResilienceEventType,
    check_free_disk_space,
    check_power_policy,
    compute_disk_margin,
)
from paradl.resilience.hooks import HookRunner
from paradl.resilience.queue import QueueManager

__all__ = [
    "HookRunner",
    "QueueManager",
    "ResilienceEvent",
    "ResilienceEventSource",
    "ResilienceEventType",
    "check_free_disk_space",
    "check_power_policy",
    "compute_disk_margin",
]
