"""Backward-compatible shim: re-exports from paradl.ui.tui.

Deprecated: import from paradl.ui.tui instead.
"""

from __future__ import annotations

from paradl.ui.display import LiveTransferDisplay
from paradl.ui.tui import run_tui

__all__ = ["LiveTransferDisplay", "run_tui"]
