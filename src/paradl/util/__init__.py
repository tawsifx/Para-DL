"""Utility modules for pdl — cURL parsing and toolchain detection.

Provides:
- curl_parser: parse 'Copy as cURL' command strings into URL/headers/data.
- toolchain: detect installed binaries (aria2c, curl, brew, git, yt-dlp, etc.).
"""

from __future__ import annotations

from paradl.util.curl_parser import parse_curl_command
from paradl.util.toolchain import TOOLCHAIN

__all__ = [
    "TOOLCHAIN",
    "parse_curl_command",
]
