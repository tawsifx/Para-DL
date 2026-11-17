"""Network layer for pdl — pooled HTTP client, Range/HEAD probing, target classification, and DNS resolution.

Provides:
- A lazily initialized, connection-pooled httpx.Client with HTTP/2 and per-host TLS
  session ticket caching.
- HTTP Range support probing (206 Partial Content detection).
- Target URL classification (direct, github, huggingface, ipsw, pypi, brew, media, stream, search).
- DNS resolution and ISP detection helpers.

No module here imports from the engines, resilience, or app layers.
"""

from __future__ import annotations

import socket
import time
from typing import Any

from paradl.net.client import (
    HttpProbeResult,
    clean_isp_name,
    close_http_client,
    detect_isp,
    extract_etag_sha256,
    extract_verified_sha256,
    fetch_json_with_etag,
    get_http_client,
    parse_content_disposition_filename,
    probe_http_url,
    probe_range_support,
    probe_target,
    probe_targets_parallel,
    re_resolve_dns,
    request_with_retry,
    reset_isp_detector,
    resolve_dns,
    resolve_dns_doh,
    set_isp_detector,
    ytdlp_extractor_claims_url,
)

# Cache for ISP detection results (backward-compat: accessible as paradl.net._CACHED_ISP)
_CACHED_ISP: tuple[float, str] | None = None

# Shared HTTP client and TLS session cache (backward-compat: accessible as
# paradl.net._SHARED_CLIENT and paradl.net._TLS_SESSION_CACHE)
_SHARED_CLIENT: Any = None
_TLS_SESSION_CACHE: dict[str, Any] = {}

__all__ = [
    "_CACHED_ISP",
    "_SHARED_CLIENT",
    "_TLS_SESSION_CACHE",
    "HttpProbeResult",
    "clean_isp_name",
    "close_http_client",
    "detect_isp",
    "extract_etag_sha256",
    "extract_verified_sha256",
    "fetch_json_with_etag",
    "get_http_client",
    "parse_content_disposition_filename",
    "probe_http_url",
    "probe_range_support",
    "probe_target",
    "probe_targets_parallel",
    "re_resolve_dns",
    "request_with_retry",
    "reset_isp_detector",
    "resolve_dns",
    "resolve_dns_doh",
    "set_isp_detector",
    "socket",
    "time",
    "ytdlp_extractor_claims_url",
]
