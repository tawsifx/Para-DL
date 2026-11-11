"""Backward-compatible shim: re-exports from paradl.util.toolchain.

Deprecated: import from paradl.util.toolchain instead.
"""

from __future__ import annotations

from paradl.util.toolchain import TOOLCHAIN, Toolchain

__all__ = ["TOOLCHAIN", "Toolchain"]
