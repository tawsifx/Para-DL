"""Backward-compatible shim: re-exports from paradl.resilience.queue.

Deprecated: import from paradl.resilience.queue instead.
"""

from __future__ import annotations

from paradl.resilience.queue import QueueManager

__all__ = ["QueueManager"]
