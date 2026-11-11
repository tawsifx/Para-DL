"""Backward-compatible shim: re-exports from paradl.diagnostics.doctor.

Deprecated: import from paradl.diagnostics.doctor instead.
"""

from __future__ import annotations

import sys

from paradl.diagnostics.doctor import run_doctor


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


__all__ = ["is_windows", "run_doctor"]
