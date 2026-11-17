"""S2 Task 2 contract guards (PyO3 boundary / adapter).

Every guard here is a verification/regression guard for behaviour that the
audit found already correct; they pin the contract so a future regression is
caught.  They live in S2-owned ``rust/paradl-rs/contract_tests/`` because
``tests/`` is owned by S3 in the audit swarm.
"""

from __future__ import annotations

import http.server
import json
import os
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

_SRC = Path(__file__).resolve().parents[3] / "src"


# ---------------------------------------------------------------------------
# (b) .so shadows the shim in a fresh subprocess
# ---------------------------------------------------------------------------


def test_so_shadows_shim_in_fresh_subprocess() -> None:
    script = (
        "import json, paradl._rust_engine as n; "
        "print(json.dumps({'file': n.__file__, 'avail': n.is_available(), "
        "'version': n.version(), '__version__': n.__version__, "
        "'has_start': callable(getattr(n, 'start_download', None))}))"
    )
    env = {**os.environ, "PYTHONPATH": str(_SRC)}
    proc = subprocess.run(
        [sys.executable, "-c", script],
        capture_output=True,
        text=True,
        env=env,
        check=True,
    )
    data: dict[str, Any] = json.loads(proc.stdout.strip().splitlines()[-1])
    assert data["file"].endswith(".so"), data
    assert data["avail"] is True, data
    assert data["version"] == "0.1.0", data
    assert data["__version__"] == "0.1.0", data
    assert data["has_start"] is True, data


# ---------------------------------------------------------------------------
# (c) every native exception maps to an ExitCode; none escapes the protocol
# ---------------------------------------------------------------------------


def test_native_exception_maps_to_every_exit_code() -> None:
    from paradl._rust_engine import DownloadError, VerificationError
    from paradl.core.models import ExitCode
    from paradl.engines.rust_engine import _native_error_exit_code

    cases = [
        (DownloadError("network error: boom"), int(ExitCode.FAILURE)),
        (VerificationError("checksum mismatch after download"), int(ExitCode.VERIFICATION_FAILED)),
        (DownloadError("download cancelled"), int(ExitCode.INTERRUPTED)),
        (ImportError("native engine not built"), int(ExitCode.MISSING_DEPENDENCY)),
        (RuntimeError("generic runtime failure"), int(ExitCode.FAILURE)),
    ]
    for exc, expected in cases:
        assert _native_error_exit_code(exc) == expected, (exc, expected)


class _Handle:
    def __init__(self, wait_error: BaseException) -> None:
        self.recycle_connections = MagicMock()
        self.pause = MagicMock()
        self.resume = MagicMock()
        self.cancel = MagicMock()
        self.wait = MagicMock(side_effect=wait_error)
        self._n = 0

    def poll_snapshot(self) -> dict[str, Any]:
        self._n += 1
        if self._n == 1:
            return {"downloaded": 1, "total": 2, "speed_bps": 0.0, "connections": 2, "state": "running"}
        return {"downloaded": 2, "total": 2, "speed_bps": 1.0, "connections": 2, "state": "complete"}


class _Native:
    def __init__(self, handle: _Handle) -> None:
        self._handle = handle
        self.start_download = MagicMock(return_value=handle)

    def is_available(self) -> bool:
        return True


@pytest.mark.parametrize(
    ("error_name", "expected"),
    [
        ("VerificationError", 3),
        ("DownloadError", 1),
    ],
)
def test_no_native_exception_escapes_engine_protocol(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, error_name: str, expected: int
) -> None:
    from paradl._rust_engine import DownloadError, VerificationError
    from paradl.core.models import TargetInfo, TargetType, TransferResult
    from paradl.engines import rust_engine as re

    cls = VerificationError if error_name == "VerificationError" else DownloadError
    message = (
        "checksum mismatch after download"
        if error_name == "VerificationError"
        else "network error: HTTP 500"
    )
    handle = _Handle(cls(message))
    monkeypatch.setattr(re, "_native", _Native(handle))
    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
    )
    result = re.RustEngine().execute(
        info, tmp_path, connections=2, poll_interval=0.0, net_watch=False
    )
    assert isinstance(result, TransferResult)
    assert result.success is False
    assert result.exit_code == expected


# ---------------------------------------------------------------------------
# (d) every CLI flag with a native equivalent reaches start_download
# ---------------------------------------------------------------------------


def test_native_flags_reach_start_download(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """A native stub with NO **kwargs proves the signature filter forwards the flags."""
    from paradl._rust_engine import DownloadError
    from paradl.core.models import TargetInfo, TargetType
    from paradl.engines import rust_engine as re

    payload = b"hello"
    dest = tmp_path / "file.bin"
    dest.write_bytes(payload)
    handle = _Handle(DownloadError("unused"))
    handle.wait = MagicMock(
        return_value={"success": True, "file_path": str(dest), "downloaded": len(payload)}
    )

    class StrictNative:
        def __init__(self) -> None:
            self.calls: list[dict[str, Any]] = []

        def start_download(
            self,
            url: str,
            out_path: str,
            connections: int | None = None,
            headers: dict[str, str] | None = None,
            cookies: str | None = None,
            referer: str | None = None,
            user_agent: str | None = None,
            proxy: str | None = None,
            ip_family: str | None = None,
            mirrors: list[str] | None = None,
            checksum: str | None = None,
            rate_limit_bps: int | None = None,
        ) -> Any:
            self.calls.append(
                {
                    "url": url,
                    "connections": connections,
                    "headers": headers,
                    "cookies": cookies,
                    "referer": referer,
                    "user_agent": user_agent,
                    "proxy": proxy,
                    "ip_family": ip_family,
                    "mirrors": mirrors,
                    "checksum": checksum,
                    "rate_limit_bps": rate_limit_bps,
                }
            )
            return handle

        def is_available(self) -> bool:
            return True

    strict = StrictNative()
    monkeypatch.setattr(re, "_native", strict)
    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
        headers={"Authorization": "Bearer t"},
        cookie_file="/tmp/cookies.txt",
        referer="https://example.com/",
        user_agent="paradl-test",
        expected_digest="sha256:" + "0" * 64,
        mirrors=["https://mirror.example/file.bin"],
    )
    re.RustEngine().execute(
        info,
        tmp_path,
        connections=4,
        poll_interval=0.0,
        net_watch=False,
        limit_rate="1MB/s",
        proxy="socks5://127.0.0.1:1080",
        ip_family="ipv4",
    )
    assert len(strict.calls) == 1
    call = strict.calls[0]
    assert call["checksum"] == info.expected_digest
    assert call["cookies"] == "/tmp/cookies.txt"
    assert call["headers"] == {"Authorization": "Bearer t"}
    assert call["referer"] == "https://example.com/"
    assert call["user_agent"] == "paradl-test"
    assert call["proxy"] == "socks5://127.0.0.1:1080"
    assert call["ip_family"] == "ipv4"
    assert call["mirrors"] == ["https://mirror.example/file.bin"]
    assert call["rate_limit_bps"] is not None


# ---------------------------------------------------------------------------
# (e) recycle on terminal handle is a native no-op
# ---------------------------------------------------------------------------


class _OnceHandler(http.server.BaseHTTPRequestHandler):
    body = b"z" * 32768

    def do_GET(self) -> None:
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.body)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(self.body)

    def log_message(self, *args: Any) -> None:
        pass


def test_recycle_terminal_handle_is_noop(tmp_path: Path) -> None:
    from paradl._rust_engine import start_download

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _OnceHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    port = server.server_address[1]
    dest = tmp_path / "done.bin"
    try:
        handle = start_download(f"http://127.0.0.1:{port}/f", str(dest), connections=4)
        handle.wait(timeout=10)
        before = handle.poll_snapshot()
        # A late watchdog tick on a completed handle must not raise or resurrect it.
        handle.recycle_connections()
        after = handle.poll_snapshot()
        assert before["state"] == after["state"] == "done"
        assert after["downloaded"] == before["downloaded"]
    finally:
        server.shutdown()
