"""Backward-compatible shim: re-exports from paradl.util.curl_parser.

Deprecated: import from paradl.util.curl_parser instead.
"""

from __future__ import annotations

from paradl.util.curl_parser import parse_curl_command

__all__ = ["parse_curl_command"]
