"""Reusable loopback-only HTTP server with byte-range and bandwidth-throttle support.

This is NEW infrastructure: no existing test spins up an inline HTTP server, so
nothing is being consolidated here. It gives integration/e2e tests a real
``http.server`` endpoint on an ephemeral loopback port without ever touching a
WAN interface.
"""

from __future__ import annotations

import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from importlib.util import find_spec
from typing import Any

import pytest

DEFAULT_BODY: bytes = b"paradl-throttle-body-" * 32

# pytest-xdist registers a per-worker "worker" scope. Without xdist, a single
# pytest process is the worker, so "session" is the equivalent single instance.
_SERVER_SCOPE = "worker" if find_spec("xdist") is not None else "session"


def _parse_range(value: str, total: int) -> tuple[int, int] | None:
    """Parse a single ``bytes=start-end`` range into inclusive bounds."""
    if not value.startswith("bytes=") or total <= 0:
        return None
    spec = value[6:].split(",", 1)[0].strip()
    if "-" not in spec:
        return None
    raw_start, _, raw_end = spec.partition("-")
    try:
        if raw_start == "":
            suffix = int(raw_end)
            if suffix <= 0:
                return None
            return max(0, total - suffix), total - 1
        start = int(raw_start)
        end = int(raw_end) if raw_end else total - 1
    except ValueError:
        return None
    if start < 0 or start >= total:
        return None
    end = min(end, total - 1)
    return (start, end) if end >= start else None


class _ThrottleHandler(BaseHTTPRequestHandler):
    """Serves the parent server's byte body, honoring a single Range header."""

    protocol_version = "HTTP/1.1"

    @property
    def _server(self) -> ThrottleHTTPServer:
        return self.server  # type: ignore[return-value]

    def do_GET(self) -> None:
        server = self._server
        body = server.body
        total = len(body)
        start, end = 0, total - 1
        status = 200

        range_header = self.headers.get("Range")
        if range_header:
            bounds = _parse_range(range_header, total)
            if bounds is None:
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{total}")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            start, end = bounds
            status = 206

        chunk = body[start : end + 1]
        self.send_response(status)
        self.send_header("Content-Type", server.content_type)
        self.send_header("Accept-Ranges", "bytes")
        if status == 206:
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
        self.send_header("Content-Length", str(len(chunk)))
        self.end_headers()
        self._write_throttled(chunk)

    def _write_throttled(self, chunk: bytes) -> None:
        bps = self._server.throttle_bps
        if bps <= 0:
            self.wfile.write(chunk)
            return
        # Send in ~100 ms slices so throttling stays responsive to shutdown.
        slice_size = max(1, int(bps // 10))
        for offset in range(0, len(chunk), slice_size):
            part = chunk[offset : offset + slice_size]
            time.sleep(len(part) / bps)
            self.wfile.write(part)

    def log_message(self, fmt: str, *args: Any) -> None:
        """Silence per-request logging; tests assert on responses, not access logs."""


class ThrottleHTTPServer(ThreadingHTTPServer):
    """A ``ThreadingHTTPServer`` bound to ``127.0.0.1:0`` with a throttle knob.

    Use as a context manager to start/stop the serving thread, or rely on the
    :func:`throttle_server` fixture which does exactly that.
    """

    daemon_threads = True
    allow_reuse_address = True

    def __init__(
        self,
        body: bytes = DEFAULT_BODY,
        *,
        throttle_bps: float = 0.0,
        content_type: str = "application/octet-stream",
    ) -> None:
        self.body = bytes(body)
        self.throttle_bps = float(throttle_bps)
        self.content_type = content_type
        self._thread: threading.Thread | None = None
        super().__init__(("127.0.0.1", 0), _ThrottleHandler)

    @property
    def host(self) -> str:
        return "127.0.0.1"

    @property
    def port(self) -> int:
        return int(self.server_address[1])

    @property
    def url(self) -> str:
        return f"http://{self.host}:{self.port}/"

    def __enter__(self) -> ThrottleHTTPServer:
        self._thread = threading.Thread(target=self.serve_forever, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.shutdown()
        self.server_close()
        if self._thread is not None:
            self._thread.join(timeout=5.0)
            self._thread = None


@pytest.fixture(scope=_SERVER_SCOPE)
def throttle_server():
    """Yield one shared, loopback-only throttle server for the whole run."""
    with ThrottleHTTPServer() as server:
        yield server


__all__ = ["ThrottleHTTPServer", "throttle_server"]
