"""Backward-compatible shim: re-exports from paradl.resilience.hooks.

Deprecated: import from paradl.resilience.hooks instead.
"""

from __future__ import annotations

from paradl.resilience.hooks import HookRunner

__all__ = ["HookRunner"]
