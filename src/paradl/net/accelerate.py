"""Range-capable acceleration for GitHub source archives.

`codeload.github.com` does not honour HTTP Range requests, so a source archive
can only be fetched single-stream — hundreds of seconds for a 32 MB repo. A
Range-capable mirror turns the same archive into a normal 16-channel download.

The mirror is configurable via ``PARADL_GITHUB_ARCHIVE_MIRROR`` (empty string
disables acceleration). The original GitHub URL is always kept as the canonical
target and as a failover mirror, so this is a pure transport optimisation.
"""

from __future__ import annotations

import os
from urllib.parse import urlparse

DEFAULT_MIRROR = "https://gh-proxy.com/"


def is_github_archive(url: str) -> bool:
    """True for GitHub ``/archive/`` source-archive URLs (zip/tar.gz)."""
    try:
        parsed = urlparse(url)
    except ValueError:
        return False
    host = (parsed.hostname or "").lower()
    if host not in ("github.com", "www.github.com", "codeload.github.com"):
        return False
    return "/archive/" in parsed.path


def accelerate_url(url: str) -> str | None:
    """Return the accelerated URL for a GitHub archive, or None if inapplicable.

    The mirror prefix is read from ``PARADL_GITHUB_ARCHIVE_MIRROR`` at call time
    so tests and users can toggle it without restarting.
    """
    if not is_github_archive(url):
        return None
    prefix = os.environ.get("PARADL_GITHUB_ARCHIVE_MIRROR", DEFAULT_MIRROR)
    if not prefix.strip():
        return None
    # gh-proxy accepts both github.com/archive/... and codeload URLs; pass the
    # original through unchanged so the mirror resolves it exactly as given.
    return prefix.rstrip("/") + "/" + url.lstrip("/")
