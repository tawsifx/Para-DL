"""Core data models, enums, exit codes, and rate parsing utilities."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from enum import IntEnum, StrEnum
from pathlib import Path
from typing import Any
from urllib.parse import urlparse


class TargetType(StrEnum):
    TORRENT = "torrent"
    DIRECT = "direct"
    STREAM = "stream"
    SEARCH = "search"
    GITHUB = "github"
    HF = "huggingface"
    IPSW = "ipsw"
    PYPI = "pypi"
    BREW = "brew"


class WatchdogMode(StrEnum):
    V1 = "v1"
    V2 = "v2"
    OFF = "off"


class ExitCode(IntEnum):
    OK = 0
    FAILURE = 1
    USAGE = 2
    VERIFICATION_FAILED = 3
    MISSING_DEPENDENCY = 4
    INTERRUPTED = 130


# Known direct download file extensions
DIRECT_EXTENSIONS: set[str] = {
    ".zip",
    ".tar",
    ".gz",
    ".tgz",
    ".bz2",
    ".tbz2",
    ".xz",
    ".txz",
    ".7z",
    ".rar",
    ".iso",
    ".img",
    ".dmg",
    ".pkg",
    ".deb",
    ".rpm",
    ".apk",
    ".ipa",
    ".ipsw",
    ".bin",
    ".exe",
    ".msi",
    ".appimage",
    ".safetensors",
    ".gguf",
    ".pt",
    ".pth",
    ".onnx",
    ".ckpt",
    ".parquet",
    ".csv",
    ".jsonl",
    ".mp4",
    ".mkv",
    ".avi",
    ".mov",
    ".webm",
    ".flv",
    ".mp3",
    ".m4a",
    ".flac",
    ".wav",
    ".opus",
    ".aac",
    ".ogg",
    ".pdf",
    ".epub",
    ".doc",
    ".docx",
    ".png",
    ".jpg",
    ".jpeg",
    ".webp",
    ".gif",
    ".svg",
    ".whl",
    ".meta4",
    ".metalink",
}

# Known streaming media domains (matched by hostname suffix)
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

# Known image generation / image CDN domains
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
    if (host == "pollinations.ai" or host.endswith(".pollinations.ai")) and ("image" in host or "/prompt/" in url):
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


_RATE_RE = re.compile(
    r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([kKmMgGtT])?(i)?(B|b|byte|bytes|bit|bits)?(?:\s*(?:ps|/s(?:ec)?))?\s*$"
)


def parse_rate(rate_str: str | int | float | None) -> float:
    """Parse a human-readable transfer rate into bytes per second (B/s).

    Distinguishes bytes (B, KB, MB, MB/s, MBps, MiB/s) from bits (b, Kbps, Mbps, Mbit/s, Mbits/s).
    Uses binary multipliers (1 K = 1024) consistent with aria2 and pdl history.
    Raises ValueError on unparseable or negative values.
    """
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

    # Determine whether bits or bytes:
    # - 'b' (lowercase) or 'bit' / 'bits' (case-insensitive) -> bits (divide by 8)
    # - 'B' (uppercase) or 'byte' / 'bytes' or omitted (e.g. '10M', '500K') -> bytes
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


def format_rate_for_aria2(rate_str: str | None) -> str | None:
    """Validate and convert a rate string into aria2's byte-rate syntax (e.g. '1048576')."""
    if rate_str is None:
        return None
    bps = parse_rate(rate_str)
    if bps <= 0:
        return None
    return str(int(bps))


def sanitize_filename(name: str) -> str:
    """Sanitize a filename by replacing unsafe characters and blocking traversal/reserved names."""
    for ch in ["/", "\\", ":", "*", "?", '"', "<", ">", "|", "\0"]:
        name = name.replace(ch, "_")
    cleaned = name.strip()[:200]
    # Guard against dot-only names and directory traversal primitives
    stripped_dots = cleaned.strip(" .")
    if not stripped_dots or set(stripped_dots) <= {"."}:
        return "download"
    # Guard against Windows reserved device names (e.g. CON, PRN, AUX, NUL, COM1..9, LPT1..9)
    base_stem = stripped_dots.split(".", 1)[0].upper()
    if base_stem in {
        "CON",
        "PRN",
        "AUX",
        "NUL",
        *(f"COM{i}" for i in range(1, 10)),
        *(f"LPT{i}" for i in range(1, 10)),
    }:
        return f"_{cleaned}"
    return cleaned or "download"


def format_size(bytes_sz: int | float | None) -> str:
    """Format byte count into human-readable string."""
    if bytes_sz is None:
        return "Unknown size"
    if bytes_sz <= 0:
        return "0 B"
    val = float(bytes_sz)
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if val < 1024.0:
            return f"{val:.1f} {unit}"
        val /= 1024.0
    return f"{val:.1f} PB"


def format_speed(bps: float) -> str:
    """Format speed in B/s to human-readable string."""
    if bps >= 1024 * 1024:
        return f"{bps / (1024 * 1024):.1f} MB/s"
    if bps >= 1024:
        return f"{bps / 1024.0:.1f} KB/s"
    return f"{bps:.0f} B/s"


@dataclass
class TargetInfo:
    raw: str
    target_type: TargetType
    clean_url: str
    filename_hint: str | None = None
    content_length: int | None = None
    mime_type: str | None = None
    accept_ranges: bool | None = None
    etag_sha256: str | None = None
    expected_digest: str | None = None
    headers: dict[str, str] = field(default_factory=dict)
    mirrors: list[str] = field(default_factory=list)
    metalink_path: Path | None = None
    cookie_file: str | None = None
    referer: str | None = None
    user_agent: str | None = None
    github_meta: dict[str, Any] | None = None
    hf_meta: dict[str, Any] | None = None
    ipsw_meta: dict[str, Any] | None = None
    pypi_meta: dict[str, Any] | None = None
    brew_meta: dict[str, Any] | None = None
    probe_error: str | None = None
    status_code: int | None = None


@dataclass
class DownloadItem:
    url: str
    out_dir: Path
    filename: str | None = None
    headers: dict[str, str] = field(default_factory=dict)
    expected_length: int | None = None
    expected_digest: str | None = None
    etag_sha256: str | None = None
    mirrors: list[str] = field(default_factory=list)
    metalink_path: Path | None = None
    cookie_file: str | None = None
    referer: str | None = None
    user_agent: str | None = None


@dataclass
class TransferResult:
    success: bool
    file_path: Path | None = None
    files: list[Path] = field(default_factory=list)
    bytes_downloaded: int = 0
    duration: float = 0.0
    avg_speed_bps: float = 0.0
    p10_speed_bps: float = 0.0
    recycle_count: int = 0
    audio_format_produced: str | None = None
    checksum_verified: bool | None = None
    checksum_algo: str | None = None
    checksum_digest: str | None = None
    error: str | None = None
    exit_code: int = int(ExitCode.OK)
