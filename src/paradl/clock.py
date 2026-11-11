"""Backward-compatible shim: re-exports from paradl.core.clock.

Deprecated: import from paradl.core.clock instead.
"""

from __future__ import annotations

from paradl.core.clock import Clock, get_clock

__all__ = ["Clock", "get_clock"]
