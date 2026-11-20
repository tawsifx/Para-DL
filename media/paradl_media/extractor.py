"""yt-dlp extractor-claim logic moved from ``src/paradl/net/client.py:480-502``.

The Rust side delegates ``claims`` to this worker because the live yt-dlp
extractor registry cannot be replicated in pure routing rules. ``claims=false``
means the Rust client falls back to DIRECT download.
"""

from __future__ import annotations

import logging
from functools import lru_cache
from typing import Any

from .models import is_media_domain

logger = logging.getLogger(__name__)


@lru_cache(maxsize=1)
def _cached_extractor_classes() -> tuple[tuple[str, Any], ...]:
    """Build ``(IE_NAME, class)`` pairs once; ``gen_extractor_classes()`` is expensive."""
    from yt_dlp.extractor import gen_extractor_classes

    return tuple(
        (name, ie_cls)
        for ie_cls in gen_extractor_classes()
        if (name := getattr(ie_cls, "IE_NAME", "")) != "generic"
    )


def ytdlp_extractor_claims_url(url: str) -> bool:
    """Return True if a non-generic yt-dlp extractor explicitly matches the URL."""
    if is_media_domain(url):
        return True
    try:
        for _ie_name, ie_cls in _cached_extractor_classes():
            if ie_cls.suitable(url):
                return True
    except Exception as exc:
        logger.debug("yt-dlp extractor check skipped: %s", exc)
    return False
