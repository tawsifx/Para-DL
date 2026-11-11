"""Backward-compatible shim: re-exports from paradl.auth.manager.

Deprecated: import from paradl.auth.manager instead.
"""

from __future__ import annotations

from paradl.auth.manager import handle_auth_command

__all__ = ["handle_auth_command"]
