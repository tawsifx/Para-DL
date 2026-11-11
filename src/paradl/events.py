"""Backward-compatible shim: re-exports from paradl.resilience.events.

Deprecated: import from paradl.resilience.events instead.
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

__all__ = [
    "ResilienceEvent",
    "ResilienceEventSource",
    "ResilienceEventType",
    "check_free_disk_space",
    "check_power_policy",
    "compute_disk_margin",
]
