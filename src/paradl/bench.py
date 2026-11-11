"""Backward-compatible shim: re-exports from paradl.diagnostics.bench.

Deprecated: import from paradl.diagnostics.bench instead.
"""

from __future__ import annotations

from paradl.diagnostics.bench import run_bench

__all__ = ["run_bench"]
