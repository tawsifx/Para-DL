"""Backward-compatible shim: re-exports from paradl.api.server.

Deprecated: import from paradl.api.server instead.
"""

from __future__ import annotations

from paradl.api.server import run_server

__all__ = ["run_server"]
