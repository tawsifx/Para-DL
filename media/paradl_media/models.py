"""Pure media model helpers extracted from the legacy Python application.

Byte-exact copies of:

* ``sanitize_filename`` (``src/paradl/core/models.py``) — 200-char cap and
  Windows reserved device names.
* ``is_media_domain`` / ``is_image_domain`` and their domain tables
  (``src/paradl/core/models.py:111-175``).
* ``classify_audio_production`` (``src/paradl/engines/media.py:48-72``) —
  the exact ``audio_format_produced`` strings asserted by the protocol goldens.
* ``parse_rate`` (``src/paradl/core/models.py``) — used when building yt-dlp
  options.

No ``paradl.*`` imports: everything here is self-contained.
"""

from __future__ import annotations

import re
from urllib.parse import urlparse

# --- sanitize_filename --------------------------------------------------------

_UNSAFE_FILENAME_CHARS = '/\\:*?"<>|\0'
_UNSAFE_FILENAME_TABLE = str.maketrans(dict.fromkeys(_UNSAFE_FILENAME_CHARS, "_"))
_WINDOWS_RESERVED_NAMES = frozenset(
    {"CON", "PRN", "AUX", "NUL"}
    | {f"COM{i}" for i in range(1, 10)}
    | {f"LPT{i}" for i in range(1, 10)}
)


def sanitize_filename(name: str) -> str:
    """Sanitize a filename by replacing unsafe characters and blocking traversal/reserved names."""
    name = name.translate(_UNSAFE_FILENAME_TABLE)
    cleaned = name.strip()[:200]
    # Guard against dot-only names and directory traversal primitives
    stripped_dots = cleaned.strip(" .")
    if not stripped_dots or set(stripped_dots) <= {"."}:
        return "download"
    # Guard against Windows reserved device names (e.g. CON, PRN, AUX, NUL, COM1..9, LPT1..9)
    base_stem = stripped_dots.split(".", 1)[0].upper()
    if base_stem in _WINDOWS_RESERVED_NAMES:
        return f"_{cleaned}"
    return cleaned or "download"


# --- domain tables ------------------------------------------------------------

MEDIA_DOMAINS: tuple[str, ...] = (
    "youtube.com",
    "youtu.be",
    "soundcloud.com",
    "vimeo.com",
    "twitch.tv",
    "bandcamp.com",
    "bilibili.com",
    "dailymotion.com",
    "mixcloud.com",
    "facebook.com",
    "instagram.com",
    "tiktok.com",
    "twitter.com",
    "x.com",
)

IMAGE_DOMAINS: tuple[str, ...] = (
    "image.pollinations.ai",
    "images.unsplash.com",
    "i.imgur.com",
    "i.redd.it",
    "cdn.discordapp.com",
)


def is_image_domain(url: str) -> bool:
    """Return True if url's hostname matches an image generation or image CDN service."""
    try:
        parsed = urlparse(url.strip())
        host = (parsed.hostname or "").lower().rstrip(".")
    except ValueError:
        return False
    if not host:
        return False
    if (host == "pollinations.ai" or host.endswith(".pollinations.ai")) and (
        "image" in host or "/prompt/" in url
    ):
        return True
    for domain in IMAGE_DOMAINS:
        d = domain.lower().rstrip(".")
        if host == d or host.endswith("." + d):
            return True
    return False


def is_media_domain(url: str) -> bool:
    """Return True if url's hostname matches or is a subdomain of any MEDIA_DOMAINS entry."""
    try:
        parsed = urlparse(url.strip())
        host = (parsed.hostname or "").lower().rstrip(".")
    except ValueError:
        return False
    if not host:
        return False
    for domain in MEDIA_DOMAINS:
        d = domain.lower().rstrip(".")
        if host == d or host.endswith("." + d):
            return True
    return False


# --- audio format production --------------------------------------------------

LOSSLESS_SOURCE_CODECS = {
    "flac",
    "alac",
    "wav",
    "pcm",
    "pcm_s16le",
    "pcm_s24le",
    "pcm_f32le",
    "ape",
    "wv",
}


def classify_audio_production(
    requested_codec: str,
    source_acodec: str | None,
    conversion_mode: str,
) -> str:
    """Return an honest human-readable description of the audio format actually produced.

    Byte-exact copy of ``media.py:48-72``. The returned strings are wire
    contract literals asserted verbatim by the protocol goldens.
    """
    src = (source_acodec or "unknown").lower().split(".")[0]
    is_src_lossless = src in LOSSLESS_SOURCE_CODECS

    if conversion_mode == "passthrough_aac":
        return f"passthrough AAC (from {src} source, no re-encode)"
    if conversion_mode == "alac":
        if is_src_lossless:
            return f"true lossless ALAC (from lossless {src} source)"
        return f"ALAC-wrapped lossy (transcoded from lossy {src} source)"
    if requested_codec == "flac":
        if is_src_lossless:
            return f"true lossless FLAC (from {src} source)"
        return f"FLAC (transcoded from lossy {src} source)"
    if requested_codec == "wav":
        return f"uncompressed WAV PCM (from {src} source)"
    return f"{requested_codec.upper()} (from {src} source)"


# --- rate parsing -------------------------------------------------------------

_RATE_RE = re.compile(
    r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([kKmMgGtT])?(i)?(B|b|byte|bytes|bit|bits)?(?:\s*(?:ps|/s(?:ec)?))?\s*$"
)


def parse_rate(rate_str: str | int | float | None) -> float:
    """Parse a human-readable transfer rate into bytes per second (B/s)."""
    if rate_str is None:
        return 0.0
    if isinstance(rate_str, (int, float)):
        if rate_str < 0:
            raise ValueError(f"Rate cannot be negative: {rate_str}")
        return float(rate_str)

    s = rate_str.strip()
    if not s:
        raise ValueError("Empty rate string")
    if s.lower() in ("0", "off", "none", "false", "disable", "disabled"):
        return 0.0

    m = _RATE_RE.match(s)
    if not m:
        raise ValueError(f"Unparseable rate specification: {rate_str!r}")

    val_str, prefix, _iec_i, unit_kind = m.groups()
    val = float(val_str)
    if val <= 0:
        if val == 0:
            return 0.0
        raise ValueError(f"Rate must be positive: {rate_str!r}")

    mult_map = {
        None: 1.0,
        "K": 1024.0,
        "M": 1024.0**2,
        "G": 1024.0**3,
        "T": 1024.0**4,
    }
    mult = mult_map[prefix.upper() if prefix else None]

    is_bits = False
    if unit_kind is not None:
        if unit_kind == "b" or unit_kind.lower() in ("bit", "bits"):
            is_bits = True
        elif unit_kind == "B" or unit_kind.lower() in ("byte", "bytes"):
            is_bits = False

    bps = val * mult
    if is_bits:
        bps /= 8.0
    return bps
