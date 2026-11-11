"""Core abstractions for pdl — zero-dependency data models, configuration, clock, paths, and errors.

This layer defines the shared vocabulary used by every other module in the package:
data models (TargetInfo, TransferResult, TargetType, WatchdogMode, ExitCode),
runtime configuration (PDLConfig), monotonic/wall clock utilities, filesystem path
helpers, and custom exception types. No module here imports from any other pdl
subpackage.
"""

from __future__ import annotations

from paradl.core.clock import Clock, get_clock
from paradl.core.config import PDLConfig
from paradl.core.errors import PDLError
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
from paradl.core.paths import get_logs_dir, get_paradl_home

__all__ = [
    "DIRECT_EXTENSIONS",
    "Clock",
    "DownloadItem",
    "ExitCode",
    "PDLConfig",
    "PDLError",
    "TargetInfo",
    "TargetType",
    "TransferResult",
    "WatchdogMode",
    "format_size",
    "format_speed",
    "get_clock",
    "get_logs_dir",
    "get_paradl_home",
    "is_image_domain",
    "is_media_domain",
    "parse_rate",
    "sanitize_filename",
]
