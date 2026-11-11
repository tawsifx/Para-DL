"""Backward-compatible shim: re-exports from paradl.core.config.

Deprecated: import from paradl.core.config instead.
"""

from __future__ import annotations

from paradl.core.config import PDLConfig

__all__ = ["PDLConfig"]
