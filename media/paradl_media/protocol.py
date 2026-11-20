"""The ``paradl-media/1`` wire protocol: handshake, frame builders, and errors.

The authoritative specification is ``docs/superpowers/framework-charter.md`` §4
and the golden fixture ``rust/crates/paradl-oracle/fixtures/media_protocol.json``.

Design rules enforced here:

* Every frame carries ``protocol`` and ``request_id``.
* ``stdout`` is protocol-only; human logs go to stderr via :func:`log`.
* The worker emits ``error.kind`` only — it never emits exit code 3.
* Handshake failures fail closed with exit code 4 and a clear diagnostic.
"""

from __future__ import annotations

import json
import sys
from typing import Any, TextIO

from . import CURRENT_MAJOR, PROTOCOL_ID

REQUEST_FIELDS = ("protocol", "request_id", "op", "target", "out_dir", "options")
OPTIONS_FIELDS = ("audio_only", "format", "cookies_from_browser")
PROGRESS_FIELDS = (
    "protocol",
    "request_id",
    "frame",
    "downloaded_bytes",
    "total_bytes",
    "speed_bps",
    "eta_seconds",
)
RESULT_FIELDS = ("protocol", "request_id", "frame", "ok", "versions", "caps", "data")
ERROR_FIELDS = ("protocol", "request_id", "frame", "ok", "error", "versions")
VERSIONS_FIELDS = ("worker_version", "yt_dlp_version", "mutagen_version", "ffmpeg_version")
DATA_FIELDS = ("filename", "http_headers", "audio_format_produced", "paths")
OPS = ("resolve", "download_managed", "finalize", "claims")
FRAMES = ("progress", "result", "error")
CAPS = ("ejs", "mutagen", "ffmpeg")
ERROR_KINDS = ("metadata", "download", "conversion", "dependency", "usage", "interrupted")

# Worker emits error.kind only; Rust owns the ENTIRE ExitCode mapping.
ERROR_KIND_TO_EXIT_CODE: dict[str, int] = {
    "metadata": 1,
    "download": 1,
    "conversion": 1,
    "dependency": 4,
    "usage": 2,
    "interrupted": 130,
}
# Worker NEVER emits 3 (checksums are Rust-owned after ``finalize``).
WORKER_NEVER_EMITS = 3

EXIT_OK = 0
EXIT_DEPENDENCY = 4
EXIT_INTERRUPTED = 130

DIAG_MISSING = "protocol version missing"
DIAG_UNPARSEABLE = "unparseable protocol version"
DIAG_UNSUPPORTED_MAJOR = "unsupported protocol major version"


class WorkerError(Exception):
    """Structured failure carrying the wire-level ``error.kind``.

    ``exit_code`` is derived from :data:`ERROR_KIND_TO_EXIT_CODE`. Handshake /
    dependency failures use ``dependency`` so the Rust client maps them to
    exit code 4 (fail closed), as required by the charter.
    """

    def __init__(self, kind: str, message: str) -> None:
        if kind not in ERROR_KIND_TO_EXIT_CODE:
            raise ValueError(f"invalid error.kind: {kind!r}")
        super().__init__(message)
        self.kind = kind
        self.message = message

    @property
    def exit_code(self) -> int:
        """The Rust ExitCode value this kind maps to (never 3)."""
        return ERROR_KIND_TO_EXIT_CODE[self.kind]


def check_protocol_version(protocol: str | None) -> tuple[bool, str | None, int]:
    """Validate an incoming ``protocol`` string.

    Mirrors the Rust reference implementation byte-for-byte:

    * ``paradl-media/1`` (and ``/1.1``, ``/1.999``) are accepted;
    * any other major is rejected with :data:`DIAG_UNSUPPORTED_MAJOR`;
    * a missing value is rejected with :data:`DIAG_MISSING`;
    * a malformed value is rejected with :data:`DIAG_UNPARSEABLE`.

    Returns ``(accepted, diagnostic_or_None, exit_code)``.
    """
    if protocol is None:
        return False, DIAG_MISSING, EXIT_DEPENDENCY
    if not isinstance(protocol, str):
        return False, DIAG_UNPARSEABLE, EXIT_DEPENDENCY
    rest = protocol[len("paradl-media/") :] if protocol.startswith("paradl-media/") else None
    if rest is None:
        return False, DIAG_UNPARSEABLE, EXIT_DEPENDENCY
    major = rest.split(".", 1)[0]
    try:
        parsed = int(major)
    except ValueError:
        return False, DIAG_UNPARSEABLE, EXIT_DEPENDENCY
    if parsed == CURRENT_MAJOR:
        return True, None, EXIT_OK
    return False, DIAG_UNSUPPORTED_MAJOR, EXIT_DEPENDENCY


def log(message: str) -> None:
    """Write a human-readable diagnostic to stderr (never stdout)."""
    sys.stderr.write(f"[paradl_media] {message}\n")
    sys.stderr.flush()


def emit(frame: dict[str, Any], stream: TextIO | None = None) -> None:
    """Emit exactly one compact JSON line to ``stdout`` (or the given stream)."""
    out = stream if stream is not None else sys.stdout
    out.write(json.dumps(frame, ensure_ascii=False, separators=(",", ":")) + "\n")
    out.flush()


def progress_frame(
    request_id: str,
    *,
    downloaded_bytes: int,
    total_bytes: int | None,
    speed_bps: float,
    eta_seconds: float | None,
) -> dict[str, Any]:
    """Build a ``progress`` frame in the exact key order from the charter."""
    return {
        "protocol": PROTOCOL_ID,
        "request_id": request_id,
        "frame": "progress",
        "downloaded_bytes": int(downloaded_bytes),
        "total_bytes": int(total_bytes) if total_bytes is not None else None,
        "speed_bps": speed_bps,
        "eta_seconds": eta_seconds,
    }


def result_frame(
    request_id: str,
    *,
    versions: dict[str, str],
    caps: list[str],
    data: dict[str, Any],
) -> dict[str, Any]:
    """Build the single terminal ``result`` frame."""
    return {
        "protocol": PROTOCOL_ID,
        "request_id": request_id,
        "frame": "result",
        "ok": True,
        "versions": versions,
        "caps": caps,
        "data": data,
    }


def error_frame(
    request_id: str,
    *,
    kind: str,
    message: str,
    versions: dict[str, str],
) -> dict[str, Any]:
    """Build the single terminal ``error`` frame."""
    return {
        "protocol": PROTOCOL_ID,
        "request_id": request_id,
        "frame": "error",
        "ok": False,
        "error": {"kind": kind, "message": message},
        "versions": versions,
    }
