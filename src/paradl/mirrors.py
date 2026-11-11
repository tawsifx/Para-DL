"""Backward-compatible shim: re-exports from paradl.storage.mirrors.

Deprecated: import from paradl.storage.mirrors instead.
"""

from __future__ import annotations

from paradl.storage.mirrors import (
    MetalinkCompanion,
    MirrorManager,
    MirrorResolution,
    find_or_fetch_metalink,
    parse_metalink_bytes,
)

__all__ = ["MetalinkCompanion", "MirrorManager", "MirrorResolution", "find_or_fetch_metalink", "parse_metalink_bytes"]
