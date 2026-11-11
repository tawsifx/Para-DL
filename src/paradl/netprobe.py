"""Backward-compatible shim: re-exports from paradl.diagnostics.netprobe.

Deprecated: import from paradl.diagnostics.netprobe instead.
"""

from __future__ import annotations

from paradl.diagnostics.netprobe import run_netprobe

__all__ = ["run_netprobe"]
