"""Backward-compatible shim: re-exports from paradl.core.models.

Deprecated: import from paradl.core.models instead.
"""

from __future__ import annotations

from paradl.core.models import (
    DIRECT_EXTENSIONS,
    DownloadItem,
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
    format_size,
    format_speed,
    is_image_domain,
    is_media_domain,
    parse_rate,
    sanitize_filename,
)

__all__ = [
    "DIRECT_EXTENSIONS",
    "DownloadItem",
    "ExitCode",
    "TargetInfo",
    "TargetType",
    "TransferResult",
    "WatchdogMode",
    "format_size",
    "format_speed",
    "is_image_domain",
    "is_media_domain",
    "parse_rate",
    "sanitize_filename",
]
