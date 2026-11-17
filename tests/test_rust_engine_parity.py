"""Parity tests for the native Rust download engine vs the legacy aria2 path.

TDD contract (plan Task 3): these tests specify the behaviour the PyO3 engine
must expose before ``paradl._rust_engine`` exists.  The whole module is skipped
via ``importorskip`` until Subagent 1 builds the extension, so the suite stays
green during the migration.  Once the extension is present the tests exercise
the *real* native engine (and the real Python adapter) against a loopback HTTP
server -- no external network is ever touched.

Exclusive scope: this file plus ``test_rust_watchdog_recycle.py`` only.  The
throttling server and compatibility helpers below are shared by importing this
module from the watchdog test.
"""

from __future__ import annotations

import hashlib
import importlib
import inspect
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import pytest

# The native engine may not be compiled yet.  Keep the module importable (and
# skipped) until Subagent 1's ``maturin develop --release`` lands.
#
# NOTE: S2 ships an importable *shim* that raises RustEngineUnavailable on use,
# so mere importability is not sufficient -- gate on is_available() too.
_RUST = pytest.importorskip("paradl._rust_engine", reason="native engine shim not present")
if not callable(getattr(_RUST, "is_available", None)) or not _RUST.is_available():
    pytest.skip("native Rust engine not built yet", allow_module_level=True)
try:
    importlib.import_module("paradl.engines.rust_engine")
except ImportError:
    pytest.skip("RustEngine adapter not written yet", allow_module_level=True)


# ---------------------------------------------------------------------------
# Loopback HTTP server with range + throttle + redirect + mirror behaviour
# ---------------------------------------------------------------------------


@dataclass
class SeenRequest:
    """One HTTP request observed by the loopback server."""

    path: str
    host: str | None
    cookie: str | None
    range_header: str | None
    authorization: str | None = None


@dataclass
class ThrottlingServer:
    """Owns a ``ThreadingHTTPServer`` bound to 127.0.0.1:0.

    ``throttle_bps`` is a *knob*: 0 means unlimited, any positive value makes
    body writes sleep per chunk.  It is re-read per chunk, so changing it
    mid-transfer affects the in-flight response -- which is what the
    watchdog/recycle tests rely on.
    """

    payload: bytes
    throttle_bps: float = 0.0
    chunk_size: int = 4096
    bytes_sent: int = 0
    seen: list[SeenRequest] = field(default_factory=list)
    _httpd: ThreadingHTTPServer | None = field(default=None, repr=False)
    _thread: threading.Thread | None = field(default=None, repr=False)

    @property
    def port(self) -> int:
        assert self._httpd is not None
        return int(self._httpd.server_address[1])

    def url(self, path: str = "/file", *, host: str = "127.0.0.1") -> str:
        return f"http://{host}:{self.port}{path}"

    @property
    def ranges(self) -> list[tuple[int, int]]:
        """All (start, end) byte ranges the server was asked for."""
        out: list[tuple[int, int]] = []
        for req in self.seen:
            rh = req.range_header
            if not rh or not rh.startswith("bytes="):
                continue
            span = rh.split("=", 1)[1].split(",", 1)[0]
            start_s, _, end_s = span.partition("-")
            start = int(start_s)
            end = int(end_s) if end_s else len(self.payload) - 1
            out.append((start, end))
        return out

    def requests_for(self, path: str) -> list[SeenRequest]:
        return [r for r in self.seen if r.path == path]

    def start(self) -> None:
        server = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, fmt: str, *args: Any) -> None:
                return  # keep pytest output quiet

            def handle_one_request(self) -> None:
                # A client that abandons an HTTP/1.1 keep-alive connection (the
                # native engine closing its socket after reading a response, or
                # a test client after following a 302) makes rfile.readline()
                # raise on the next iteration of BaseHTTPRequestHandler.handle.
                # That is a normal peer disconnect, not a server fault: close
                # quietly so socketserver does not print a teardown traceback.
                try:
                    super().handle_one_request()
                except (ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
                    self.close_connection = True

            def _record(self) -> None:
                server.seen.append(
                    SeenRequest(
                        path=urlparse(self.path).path,
                        host=self.headers.get("Host"),
                        cookie=self.headers.get("Cookie"),
                        range_header=self.headers.get("Range"),
                        authorization=self.headers.get("Authorization"),
                    )
                )

            def _send_body(
                self,
                body: bytes,
                *,
                status: int,
                accept_ranges: bool,
                include_length: bool = True,
            ) -> None:
                self.send_response(status)
                if include_length:
                    self.send_header("Content-Length", str(len(body)))
                else:
                    # Close-delimited 200, like codeload's final response: the
                    # client cannot know the size until the socket closes.
                    self.close_connection = True
                    self.send_header("Connection", "close")
                if accept_ranges:
                    self.send_header("Accept-Ranges", "bytes")
                self.end_headers()
                self._write_throttled(body)

            def _write_throttled(self, body: bytes) -> None:
                chunk = max(1, int(server.chunk_size))
                for offset in range(0, len(body), chunk):
                    # Re-read the knob per chunk: a single close-delimited GET may
                    # stream the whole body, and tests unthrottle mid-transfer to
                    # let it finish. Capturing the value once would ignore that.
                    throttle = server.throttle_bps
                    pause = chunk / throttle if throttle and throttle > 0 else 0.0
                    if pause and offset:
                        time.sleep(pause)
                    try:
                        self.wfile.write(body[offset : offset + chunk])
                    except (BrokenPipeError, ConnectionResetError):
                        return
                    server.bytes_sent += min(chunk, len(body) - offset)
                self.wfile.flush()

            def do_GET(self) -> None:
                self._record()
                path = urlparse(self.path).path

                if path == "/fail500":
                    self._send_body(b"upstream failure", status=500, accept_ranges=False)
                    return
                if path == "/redirect":
                    target = f"http://localhost:{server.port}/final"
                    self.send_response(302)
                    self.send_header("Location", target)
                    # Host-only cookie: scoped to 127.0.0.1 by the browser rules,
                    # so it must NOT be sent to the `localhost` redirect target.
                    self.send_header("Set-Cookie", "session=secret123; Path=/")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                if path == "/redirect-ranged":
                    # 302 to a *range-capable* loopback host. This keeps the
                    # redirect-derived URI (the thing aria2 dropped in the
                    # diagnosis) while still allowing a 16-channel segmented,
                    # throttled transfer for the recycle regression test.
                    self.send_response(302)
                    self.send_header("Location", f"http://localhost:{server.port}/file")
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                if path in {"/file", "/mirror", "/final"}:
                    data = server.payload
                    if path == "/final":
                        # Mimic codeload's final 200: NO Accept-Ranges AND no
                        # Content-Length (close-delimited). The size is learned
                        # only at EOF, which is what produced the false 100%
                        # progress tick in the diagnosed aria2 failure.
                        self._send_body(
                            data,
                            status=200,
                            accept_ranges=False,
                            include_length=False,
                        )
                        return
                    rng = self.headers.get("Range")
                    if rng and rng.startswith("bytes="):
                        span = rng.split("=", 1)[1].split(",", 1)[0]
                        start_s, _, end_s = span.partition("-")
                        start = int(start_s)
                        end = int(end_s) if end_s else len(data) - 1
                        end = min(end, len(data) - 1)
                        piece = data[start : end + 1]
                        self.send_response(206)
                        self.send_header("Content-Length", str(len(piece)))
                        self.send_header("Accept-Ranges", "bytes")
                        self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
                        self.end_headers()
                        self._write_throttled(piece)
                        return
                    self._send_body(data, status=200, accept_ranges=True)
                    return

                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()

        httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        httpd.daemon_threads = True
        server._httpd = httpd
        server._thread = threading.Thread(target=httpd.serve_forever, daemon=True)
        server._thread.start()

    def close(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
        if self._thread is not None:
            self._thread.join(timeout=2)


@pytest.fixture
def payload() -> bytes:
    """Deterministic pseudo-random 1 MiB payload (compressible content avoided)."""
    return bytes(((i * 37 + 11) % 256) for i in range(1024 * 1024))


@pytest.fixture
def server(payload: bytes) -> Any:
    srv = ThrottlingServer(payload=payload)
    srv.start()
    try:
        yield srv
    finally:
        srv.close()


# ---------------------------------------------------------------------------
# Compatibility helpers -- bind to the evolving native/adapter interface.
# They only flatten plausible naming differences; they NEVER relax behaviour
# assertions, so a missing capability still fails the test.
# ---------------------------------------------------------------------------

_SNAPSHOT_ALIASES: dict[str, tuple[str, ...]] = {
    "downloaded": ("downloaded", "downloaded_bytes", "bytes_downloaded", "completed"),
    "total": ("total", "total_bytes", "length", "size"),
    "connections": ("connections", "num_connections", "active_connections", "channels"),
    "state": ("state", "status"),
}


def snap(snapshot: Any, field_name: str) -> Any:
    """Read a Snapshot field from a dict, object or as_dict()/to_dict() mapping."""
    obj: Any = snapshot
    for method in ("as_dict", "to_dict", "_asdict"):
        if hasattr(obj, method) and callable(getattr(obj, method)):
            obj = getattr(obj, method)()
            break
    for name in _SNAPSHOT_ALIASES.get(field_name, (field_name,)):
        if isinstance(obj, dict) and name in obj:
            return obj[name]
        if hasattr(obj, name):
            return getattr(obj, name)
    raise AssertionError(f"Snapshot lacks field {field_name!r}: {snapshot!r}")


_SUCCESS_STATES = {"complete", "completed", "done", "success", "ok", "finished", "succeeded"}


def _success_signal(value: Any) -> bool | None:
    """Return an explicit success verdict from a status object, or None if absent."""
    if value is None:
        return None
    obj: Any = value
    if not isinstance(obj, dict):
        for method in ("as_dict", "to_dict", "_asdict"):
            candidate = getattr(obj, method, None)
            if callable(candidate):
                try:
                    converted = candidate()
                except Exception:  # pragma: no cover - defensive around native code
                    converted = None
                if isinstance(converted, dict):
                    obj = converted
                    break
        if not isinstance(obj, dict) and hasattr(obj, "__dict__"):
            obj = dict(vars(obj))
    if not isinstance(obj, dict):
        return None
    for key in ("success", "ok", "completed"):
        if obj.get(key) is not None:
            return bool(obj[key])
    for key in ("state", "status"):
        if obj.get(key) is not None:
            return str(obj[key]).lower() in _SUCCESS_STATES
    return None


def terminal_success(result: Any, snapshot: Any = None) -> bool:
    """True when a native ``wait()`` result or final snapshot reports success.

    Prefers the wait() status; falls back to the final snapshot when the native
    handle returns no explicit status object.
    """
    for candidate in (result, snapshot):
        verdict = _success_signal(candidate)
        if verdict is not None:
            return verdict
    return False


def call_flexible(
    fn: Callable[..., Any], positional: tuple[Any, ...], candidates: dict[str, Any]
) -> Any:
    """Call ``fn`` passing only the kwargs its signature accepts.

    The native API is still being finalised by other subagents; this keeps the
    parity tests binding to the documented surface without hand-editing each
    test when a keyword is renamed.
    """
    try:
        params = inspect.signature(fn).parameters
    except (TypeError, ValueError):
        return fn(
            *positional, **{k: v for k, v in candidates.items() if k not in ("url", "out_path")}
        )
    # PyO3 builtins expose **_kwargs, so a naive VAR_KEYWORD pass forwards the
    # same `url`/`out_path` that are already positional. Drop every candidate
    # whose name matches a positionally supplied parameter.
    positional_names = [
        name for name, p in params.items() if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
    ]
    supplied = set(positional_names[: len(positional)])
    candidates = {k: v for k, v in candidates.items() if k not in supplied}
    if any(p.kind is p.VAR_KEYWORD for p in params.values()):
        return fn(*positional, **candidates)
    kwargs = {k: v for k, v in candidates.items() if k in params}
    return fn(*positional, **kwargs)


def adapter_engine() -> Any:
    """Import and instantiate the Python RustEngine adapter lazily."""
    module = importlib.import_module("paradl.engines.rust_engine")
    for name in ("RustEngine", "RustDownloadEngine", "Engine"):
        cls = getattr(module, name, None)
        if cls is not None:
            return cls()
    raise AssertionError(f"No engine class found in {module.__name__}")


def target_info(url: str, **overrides: Any) -> Any:
    from paradl.core.models import TargetInfo, TargetType

    return TargetInfo(
        raw=url,
        target_type=TargetType.DIRECT,
        clean_url=url,
        filename_hint="payload.bin",
        accept_ranges=True,
        **overrides,
    )


def wait_for(
    predicate: Callable[[], bool], *, timeout: float = 5.0, interval: float = 0.02
) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


def start_native(url: str, dest: Path, **opts: Any) -> Any:
    """Start a native download, tolerating plausible kwarg names."""
    return call_flexible(
        _RUST.start_download,
        (url, str(dest)),
        {
            "connections": opts.get("connections", 16),
            "url": url,
            "out_path": str(dest),
            "dest": str(dest),
            "headers": opts.get("headers"),
            "mirrors": opts.get("mirrors"),
            "expected_digest": opts.get("expected_digest"),
            "cookie_file": opts.get("cookie_file"),
        },
    )


def resume_native(url: str, dest: Path, **opts: Any) -> Any:
    """Resume a native download, falling back through plausible entry points."""
    resume = getattr(_RUST, "resume_download", None)
    if resume is not None:
        try:
            return call_flexible(
                resume,
                (url, str(dest)),
                {
                    "connections": opts.get("connections", 16),
                    "url": url,
                    "out_path": str(dest),
                    "dest": str(dest),
                    "expected_digest": opts.get("expected_digest"),
                },
            )
        except TypeError:
            pass
        try:
            return call_flexible(resume, (str(dest),), {"url": url})
        except TypeError:
            pass
    # Last resort: a fresh handle continuing from on-disk .part state.
    return start_native(url, dest, **opts)


def wait_handle(handle: Any, *, timeout: float = 10.0) -> Any:
    waiter = getattr(handle, "wait", None)
    assert waiter is not None, "DownloadHandle.wait() is required"
    return call_flexible(waiter, (), {"timeout": timeout})


# ---------------------------------------------------------------------------
# 1. Segmented download: 16 channels, byte-exact, SHA-256 verified
# ---------------------------------------------------------------------------


def test_segmented_download_16_channels_bytes_and_sha256(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()
    info = target_info(
        server.url("/file"),
        content_length=len(payload),
        expected_digest=digest,
    )
    out = tmp_path / "seg"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=16)

    assert result.success is True
    assert result.exit_code == 0
    assert result.file_path is not None
    target = Path(result.file_path)
    assert target.is_file()
    assert target.read_bytes() == payload
    assert result.checksum_verified is True

    # A correct planner may coalesce segments or split them further, and retries
    # add extra ranged GETs. The ≤16 cap is on CONCURRENT connections, not on the
    # total request count. What must hold: segmentation actually happened, the
    # union covers every byte (no truncation), and the file is byte-exact.
    ranges = server.ranges
    assert 2 <= len(ranges) <= 64, f"expected 2..64 ranged requests, got {len(ranges)}"
    covered = set()
    for start, end in ranges:
        covered.update(range(start, end + 1))
    assert covered == set(range(len(payload))), (
        f"segment coverage has holes: {len(covered)} of {len(payload)} bytes"
    )


# ---------------------------------------------------------------------------
# 4. Mirror failover: first mirror 500, second succeeds
# ---------------------------------------------------------------------------


def test_mirror_failover_returns_success_like_aria2(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    info = target_info(
        server.url("/fail500"),
        mirrors=[server.url("/mirror")],
        content_length=len(payload),
    )
    out = tmp_path / "mirrors"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=4)

    assert result.success is True, result.error
    assert result.exit_code == 0
    assert result.file_path is not None
    assert Path(result.file_path).read_bytes() == payload
    assert server.requests_for("/fail500"), "primary mirror was never tried"
    assert server.requests_for("/mirror"), "fallback mirror was never tried"


# ---------------------------------------------------------------------------
# 5. Host-scoped cookie must not leak across a 302 to a different host
# ---------------------------------------------------------------------------


def test_cookie_does_not_leak_to_cross_host_redirect(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    cookie_file = tmp_path / "cookies.txt"
    cookie_file.write_text(
        "# Netscape HTTP Cookie File\n127.0.0.1\tFALSE\t/\tFALSE\t2000000000\tsession\tsecret123\n"
    )
    info = target_info(
        server.url("/redirect"),
        cookie_file=str(cookie_file),
        content_length=len(payload),
    )
    out = tmp_path / "cookies"
    out.mkdir()

    # Throttle the close-delimited 200 so multiple progress ticks fire before
    # EOF -- that is exactly where the diagnosed false 100% appeared.
    server.throttle_bps = 512 * 1024
    events: list[tuple[int, int | None, int]] = []

    def on_progress(done: int, total: int | None, speed: float, conns: int) -> None:
        # Record how many bytes the server had actually written at tick time.
        events.append((done, total, server.bytes_sent))

    t0 = time.monotonic()
    try:
        result = adapter_engine().execute(
            info,
            out,
            connections=4,
            cookie_file=str(cookie_file),
            on_progress=on_progress,
        )
    finally:
        server.throttle_bps = 0.0
    elapsed = time.monotonic() - t0

    # L1 timing guard (audit Task 3c): 1 MiB at 512 KB/s is ~2 s of wall-clock
    # work. A close-delimited, non-Range stream must be read once to EOF -- if
    # the engine re-GETs the whole body per bounded window it takes ~30 s.
    assert elapsed < 8.0, (
        f"close-delimited cookie-redirect transfer took {elapsed:.1f}s; "
        "expected ~2s for 1 MiB at 512 KB/s (redundant re-GETs / L1)"
    )

    assert result.success is True, result.error
    assert result.file_path is not None
    assert Path(result.file_path).read_bytes() == payload

    initial = server.requests_for("/redirect")
    final = server.requests_for("/final")
    assert initial and final, "redirect chain was not followed"
    assert any("session=secret123" in (r.cookie or "") for r in initial), (
        "host-scoped cookie was not sent to its own host"
    )
    assert all("session=secret123" not in (r.cookie or "") for r in final), (
        "cookie leaked to a different host after 302"
    )

    # /final is close-delimited (no Content-Length, no Accept-Ranges), just like
    # codeload in the diagnosis. A progress event must never claim completion
    # before the server has actually written every byte.
    assert events, "no progress events captured for the close-delimited response"
    for done, total, sent_at_event in events:
        if done >= len(payload):
            assert sent_at_event + server.chunk_size >= len(payload), (
                "false-complete: all bytes reported before the server sent them"
            )
        if total is not None and total > 0:
            assert not (done >= total and done < len(payload)), (
                "false-complete: 100% reported while bytes were still missing"
            )


# ---------------------------------------------------------------------------
# 6. ExitCode parity (shim contract + engine-driven outcomes)
# ---------------------------------------------------------------------------


def test_exit_code_contract_values() -> None:
    from paradl.core.models import ExitCode, TransferResult

    assert int(ExitCode.OK) == 0
    assert int(ExitCode.FAILURE) == 1
    assert int(ExitCode.VERIFICATION_FAILED) == 3
    assert int(ExitCode.MISSING_DEPENDENCY) == 4
    assert TransferResult(success=True).exit_code == 0


def test_checksum_mismatch_yields_verification_failed(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    bad_digest = "sha256:" + "0" * 64
    info = target_info(
        server.url("/file"),
        expected_digest=bad_digest,
        content_length=len(payload),
    )
    out = tmp_path / "badsum"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=4)

    assert result.success is False
    assert result.exit_code == 3, result.error
    assert result.checksum_verified is False


def test_generic_failure_yields_exit_code_one(server: ThrottlingServer, tmp_path: Path) -> None:
    info = target_info(server.url("/fail500"))
    out = tmp_path / "failure"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=2)

    assert result.success is False
    assert result.exit_code == 1
