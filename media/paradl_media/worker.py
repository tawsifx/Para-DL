"""One-shot request dispatch and process lifecycle for the media worker.

``run()`` reads exactly one JSON request line from stdin, enforces the
``paradl-media/1`` handshake, dispatches the requested op, and emits zero or
more ``progress`` frames followed by exactly one terminal ``result`` or
``error`` frame. Any human output is routed to stderr via :func:`log`.

The worker never emits exit code 3. SIGINT/SIGTERM yield ``error.kind =
"interrupted"`` and exit 130 without touching ``.pdl.part`` / ``.pdl.json``
files owned by the Rust client.
"""

from __future__ import annotations

import contextlib
import json
import signal
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any, TextIO

from . import WORKER_VERSION
from .extractor import ytdlp_extractor_claims_url
from .media import MediaEngine
from .protocol import (
    OPS,
    WorkerError,
    check_protocol_version,
    emit,
    error_frame,
    log,
    progress_frame,
    result_frame,
)
from .toolchain import TOOLCHAIN, binary_version, mutagen_version, yt_dlp_version

ProgressCallback = Callable[[int, int | None, float, float | None], None]


def collect_versions() -> dict[str, str]:
    """Return the four version keys required in every terminal frame.

    All values are strings so they are always parseable by the Rust client.
    A missing weak dependency is reported as ``"unknown"`` (never omitted).
    """
    return {
        "worker_version": WORKER_VERSION,
        "yt_dlp_version": yt_dlp_version(),
        "mutagen_version": mutagen_version(),
        "ffmpeg_version": binary_version(TOOLCHAIN.ffmpeg),
    }


def collect_caps() -> list[str]:
    """Return the capability list from ``['ejs', 'mutagen', 'ffmpeg']``."""
    caps: list[str] = []
    if TOOLCHAIN.yt_dlp_ejs_version:
        caps.append("ejs")
    try:
        import mutagen  # noqa: F401

        caps.append("mutagen")
    except Exception:
        pass
    if TOOLCHAIN.ffmpeg:
        caps.append("ffmpeg")
    return caps


def validate_request(request: dict[str, Any]) -> tuple[str, str, str, Path, dict[str, Any]]:
    """Validate request shape and return ``(request_id, op, target, out_dir, options)``."""
    request_id = str(request.get("request_id") or "")
    op = request.get("op")
    if op not in OPS:
        raise WorkerError("usage", f"unsupported op: {op!r}")
    target = request.get("target")
    if not isinstance(target, str) or not target:
        raise WorkerError("usage", "request 'target' must be a non-empty string")
    out_dir = request.get("out_dir")
    if not isinstance(out_dir, str) or not out_dir:
        raise WorkerError("usage", "request 'out_dir' must be a non-empty absolute path")
    options = request.get("options") or {}
    if not isinstance(options, dict):
        raise WorkerError("usage", "request 'options' must be an object")
    return request_id, str(op), target, Path(out_dir), options


def handle_request(
    request: dict[str, Any],
    emit_progress: ProgressCallback,
) -> dict[str, Any]:
    """Dispatch a validated request to the appropriate operation."""
    _request_id, op, target, out_dir, options = validate_request(request)
    log(f"op={op} target={target}")
    engine = MediaEngine()
    if op == "claims":
        return {
            "filename": "",
            "http_headers": {},
            "audio_format_produced": None,
            "paths": [],
            "claims": ytdlp_extractor_claims_url(target),
        }
    if op == "resolve":
        return engine.resolve(target, options)
    if op == "download_managed":
        return engine.download_managed(target, out_dir, options, emit_progress)
    if op == "finalize":
        return engine.finalize(target, out_dir, options)
    raise WorkerError("usage", f"unsupported op: {op!r}")


def _noop_progress(
    _downloaded_bytes: int,
    _total_bytes: int | None,
    _speed_bps: float,
    _eta_seconds: float | None,
) -> None:
    """Default progress callback (overridden by ``run`` to emit frames)."""


def install_signal_handlers() -> None:
    """Convert SIGINT/SIGTERM into ``KeyboardInterrupt`` (exit 130 path)."""

    def _handler(signum: int, _frame: Any) -> None:
        log(f"received signal {signum}; interrupting (partial files preserved)")
        raise KeyboardInterrupt

    for sig in (signal.SIGINT, signal.SIGTERM):
        with contextlib.suppress(ValueError, OSError, RuntimeError):
            signal.signal(sig, _handler)


def run(
    stdin: TextIO | None = None,
    stdout: TextIO | None = None,
    *,
    emit_progress_fn: ProgressCallback | None = None,
) -> int:
    """Run one request/response cycle and return the process exit code."""
    inp = stdin if stdin is not None else sys.stdin
    versions = collect_versions()

    def progress(
        downloaded_bytes: int,
        total_bytes: int | None,
        speed_bps: float,
        eta_seconds: float | None,
    ) -> None:
        if emit_progress_fn is not None:
            emit_progress_fn(downloaded_bytes, total_bytes, speed_bps, eta_seconds)
            return
        emit(
            progress_frame(
                request_id,
                downloaded_bytes=downloaded_bytes,
                total_bytes=total_bytes,
                speed_bps=speed_bps,
                eta_seconds=eta_seconds,
            ),
            stdout,
        )

    request_id = ""
    try:
        line = inp.readline()
        if not line:
            log("protocol version missing (empty request)")
            emit(
                error_frame(
                    "",
                    kind="dependency",
                    message="protocol version missing",
                    versions=versions,
                ),
                stdout,
            )
            return 4

        try:
            request = json.loads(line)
        except (ValueError, TypeError) as exc:
            log(f"unparseable request line: {exc}")
            emit(
                error_frame(
                    "",
                    kind="dependency",
                    message="unparseable protocol version",
                    versions=versions,
                ),
                stdout,
            )
            return 4

        if not isinstance(request, dict):
            emit(
                error_frame(
                    "",
                    kind="dependency",
                    message="unparseable protocol version",
                    versions=versions,
                ),
                stdout,
            )
            return 4

        request_id = str(request.get("request_id") or "")
        protocol = request.get("protocol")
        accepted, diagnostic, code = check_protocol_version(protocol)
        if not accepted:
            log(diagnostic or "unsupported protocol")
            emit(
                error_frame(
                    request_id,
                    kind="dependency",
                    message=diagnostic or "unsupported protocol",
                    versions=versions,
                ),
                stdout,
            )
            return code

        data = handle_request(request, progress)
        emit(
            result_frame(
                request_id,
                versions=versions,
                caps=collect_caps(),
                data=data,
            ),
            stdout,
        )
        return 0
    except KeyboardInterrupt:
        emit(
            error_frame(
                request_id,
                kind="interrupted",
                message="interrupted by signal",
                versions=versions,
            ),
            stdout,
        )
        return 130
    except WorkerError as exc:
        emit(
            error_frame(
                request_id,
                kind=exc.kind,
                message=exc.message,
                versions=versions,
            ),
            stdout,
        )
        return exc.exit_code
    except Exception as exc:  # defensive: never leak a traceback to stdout
        log(f"unhandled worker error: {exc}")
        emit(
            error_frame(
                request_id,
                kind="metadata",
                message=str(exc),
                versions=versions,
            ),
            stdout,
        )
        return 1
