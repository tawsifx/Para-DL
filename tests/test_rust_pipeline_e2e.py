"""End-to-end pipeline proof: URL -> native Rust engine -> filesystem.

Task 3 of the Rust-engine audit.  Every download scenario asserts the five
observable contracts of the pipeline:

* final file bytes are byte-identical to the server payload;
* the SHA-256 digest reported by the adapter matches the real bytes;
* the ``.mdm.part`` / ``.mdm.json`` sidecars are cleaned up on success;
* JSONL events (``verification``) are emitted with the right verdict and no
  ``error`` event leaks on a successful transfer;
* the ``ExitCode`` is 0 (or the documented failure code for the failure case).

The loopback server, throttler and native helpers are reused from
``test_rust_engine_parity`` (flat ``tests/`` layout).  No external network is
touched: the GitHub-archive scenario asserts only the URL-routing decision.
"""

from __future__ import annotations

import hashlib
import importlib
import json
import socket
import time
from pathlib import Path
from typing import Any

import pytest

_RUST = pytest.importorskip("paradl._rust_engine", reason="native engine shim not present")
if not callable(getattr(_RUST, "is_available", None)) or not _RUST.is_available():
    pytest.skip("native Rust engine not built yet", allow_module_level=True)
try:
    importlib.import_module("paradl.engines.rust_engine")
except ImportError:
    pytest.skip("RustEngine adapter not written yet", allow_module_level=True)

from test_rust_engine_parity import (  # noqa: E402  (availability gate first)
    ThrottlingServer,
    adapter_engine,
    start_native,
    target_info,
)

# ---------------------------------------------------------------------------
# Shared fixtures / assertion helpers
# ---------------------------------------------------------------------------


@pytest.fixture
def payload() -> bytes:
    return bytes(((i * 37 + 11) % 256) for i in range(1024 * 1024))


@pytest.fixture
def server(payload: bytes) -> Any:
    srv = ThrottlingServer(payload=payload)
    srv.start()
    try:
        yield srv
    finally:
        srv.close()


def assert_clean_pipeline_result(
    result: Any,
    out: Path,
    payload: bytes,
    *,
    events: list[dict[str, Any]],
) -> Path:
    """Assert the five pipeline contracts for a successful native transfer."""
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()

    assert result.success is True, result.error
    assert result.exit_code == 0

    target = Path(result.file_path) if result.file_path else out / "payload.bin"
    assert target.is_file(), f"final file missing: {target}"
    assert target.read_bytes() == payload, "final file is not byte-identical"

    # SHA-256 is unconditional: every scenario using this helper supplies
    # expected_digest, so the adapter MUST report the computed digest and a
    # successful verification that matches the real bytes.
    assert result.checksum_digest, "no SHA-256 digest reported for expected_digest transfer"
    assert result.checksum_digest == hashlib.sha256(target.read_bytes()).hexdigest()
    assert result.checksum_verified is True
    assert result.checksum_algo == "sha256"
    assert digest.endswith(result.checksum_digest)

    # Sidecar cleanup: nothing may be left for a spurious resume.
    leftovers = list(out.rglob("*.mdm.part")) + list(out.rglob("*.mdm.json"))
    assert leftovers == [], f"sidecars not cleaned up: {leftovers}"

    # JSONL events: a verification event with a correct verdict and no error.
    verification = [e for e in events if e.get("event") == "verification"]
    assert verification, f"no verification JSONL event: {events}"
    assert any(e.get("verified") is True for e in verification), verification
    errors = [e for e in events if e.get("event") == "error"]
    assert errors == [], f"error JSONL event on success: {errors}"

    return target


def capture_jsonl(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> list[dict[str, Any]]:
    """Enable UI JSON mode and drain emitted JSONL lines from stdout so far."""
    from paradl.ui import UI

    monkeypatch.setattr(UI, "json_mode", True)
    out, _ = capsys.readouterr()
    return [json.loads(line) for line in out.splitlines() if line.strip().startswith("{")]


def all_jsonl(capsys: pytest.CaptureFixture[str]) -> list[dict[str, Any]]:
    out, _ = capsys.readouterr()
    return [json.loads(line) for line in out.splitlines() if line.strip().startswith("{")]


# ---------------------------------------------------------------------------
# L2 regression: loopback server teardown must not log a request exception
# ---------------------------------------------------------------------------


def test_loopback_server_no_request_exception_on_abandoned_keepalive(
    capsys: pytest.CaptureFixture[str], payload: bytes
) -> None:
    """A client abandoning an HTTP/1.1 keep-alive connection must not make the
    shared test server print an "Exception occurred during processing of
    request" traceback when it tries to read the next request from the socket.

    RED before the fix (server prints the traceback), GREEN after the handler
    closes cleanly on a disconnected peer.
    """
    srv = ThrottlingServer(payload=payload[:4096])
    srv.start()
    try:
        for _ in range(6):
            sock = socket.create_connection(("127.0.0.1", srv.port), timeout=2)
            try:
                sock.sendall(b"GET /file HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                sock.recv(64)
            finally:
                sock.close()  # abandon the keep-alive connection
        # Wait until every request has been handled before shutting down.
        deadline = time.monotonic() + 2.0
        while len(srv.seen) < 6 and time.monotonic() < deadline:
            time.sleep(0.01)
        assert len(srv.seen) == 6
    finally:
        srv.close()

    err = capsys.readouterr().err
    assert "Exception occurred during processing of request" not in err, err


# ---------------------------------------------------------------------------
# Scenario 1: direct range-capable URL, 16 streams, byte-identical
# ---------------------------------------------------------------------------


def test_e2e_1_direct_range_capable_16_streams(
    server: ThrottlingServer,
    payload: bytes,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    events = capture_jsonl(monkeypatch, capsys)
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()
    info = target_info(
        server.url("/file"), content_length=len(payload), expected_digest=digest
    )
    out = tmp_path / "direct"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=16)
    events += all_jsonl(capsys)

    assert_clean_pipeline_result(result, out, payload, events=events)
    assert result.checksum_verified is True
    # Segmentation really happened (<=16 CONCURRENT, not total requests).
    assert 2 <= len(server.ranges) <= 64, server.ranges
    covered: set[int] = set()
    for start, end in server.ranges:
        covered.update(range(start, end + 1))
    assert covered == set(range(len(payload))), "segment coverage has holes"


# ---------------------------------------------------------------------------
# Scenario 2: GitHub /archive/ auto-acceleration (URL-routing only, no WAN)
# ---------------------------------------------------------------------------


GITHUB_URL = "https://github.com/anefzaoui/misterdownloadmanager/archive/refs/heads/main.zip"


def test_e2e_2_github_archive_routes_to_mirror_with_canonical_failover(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """The GitHub archive routing decision: mirror chosen, canonical URL kept
    as failover, ranges forced on. No network is touched."""
    import paradl.net.client as net_client
    from paradl.app.commands import PDLApp
    from paradl.cli import build_parser
    from paradl.config import PDLConfig
    from paradl.core.models import TargetInfo, TargetType, TransferResult
    from paradl.engines import aria2 as aria2_mod
    from paradl.net.accelerate import accelerate_url

    mirror = "https://gh-proxy.com/"
    monkeypatch.setenv("PARADL_GITHUB_ARCHIVE_MIRROR", mirror)

    # Pure accelerate.py routing decision with the configured mirror.
    assert accelerate_url(GITHUB_URL) == "https://gh-proxy.com/" + GITHUB_URL

    info = TargetInfo(
        raw=GITHUB_URL,
        target_type=TargetType.DIRECT,
        clean_url=GITHUB_URL,
        filename_hint="main.zip",
        accept_ranges=False,
    )
    captured: dict[str, Any] = {}
    fake_result_file = tmp_path / "main.zip"
    fake_result_file.write_bytes(b"archive-bytes")

    def capture_execute(self: Any, t_info: TargetInfo, out_dir: Path, **kw: Any) -> TransferResult:
        captured["t_info"] = t_info
        return TransferResult(success=True, file_path=fake_result_file, exit_code=0)

    monkeypatch.setattr(net_client, "probe_targets_parallel", lambda targets, **kw: [info])
    monkeypatch.setattr(aria2_mod.Aria2Engine, "execute", capture_execute)

    args = build_parser(PDLConfig()).parse_args(
        [GITHUB_URL, "-o", str(tmp_path), "-c", "16", "--non-interactive"]
    )
    exit_code = PDLApp(PDLConfig()).execute(args)

    assert exit_code == 0
    routed = captured["t_info"]
    assert routed.clean_url == "https://gh-proxy.com/" + GITHUB_URL
    assert GITHUB_URL in routed.mirrors, "canonical URL must be kept as failover"
    assert routed.accept_ranges is True


# ---------------------------------------------------------------------------
# Scenario 3: mirror failover (primary 500 -> mirror success)
# ---------------------------------------------------------------------------


def test_e2e_3_mirror_failover_byte_identical(
    server: ThrottlingServer,
    payload: bytes,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    events = capture_jsonl(monkeypatch, capsys)
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()
    info = target_info(
        server.url("/fail500"),
        mirrors=[server.url("/mirror")],
        content_length=len(payload),
        expected_digest=digest,
    )
    out = tmp_path / "mirrors"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=4)
    events += all_jsonl(capsys)

    assert_clean_pipeline_result(result, out, payload, events=events)
    assert server.requests_for("/fail500"), "primary was never tried"
    assert server.requests_for("/mirror"), "fallback mirror was never tried"


# ---------------------------------------------------------------------------
# Scenario 4: cookie + Bearer scoping across a cross-host 302
# ---------------------------------------------------------------------------


def test_e2e_4_cookie_scoped_across_cross_host_redirect(
    server: ThrottlingServer,
    payload: bytes,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Host-scoped cookie must NOT leak to the localhost redirect target.

    Uses /redirect-ranged (302 127.0.0.1 -> localhost/range-capable) so the
    transfer completes fast; the cookie/false-100% close-delimited scenario
    stays covered by test_cookie_does_not_leak_to_cross_host_redirect.
    """
    events = capture_jsonl(monkeypatch, capsys)
    cookie_file = tmp_path / "cookies.txt"
    cookie_file.write_text(
        "# Netscape HTTP Cookie File\n"
        "127.0.0.1\tFALSE\t/\tFALSE\t2000000000\tsession\tsecret123\n"
    )
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()
    info = target_info(
        server.url("/redirect-ranged"),
        cookie_file=str(cookie_file),
        headers={"Authorization": "Bearer scoped-token"},
        content_length=len(payload),
        expected_digest=digest,
    )
    out = tmp_path / "cookies"
    out.mkdir()

    result = adapter_engine().execute(info, out, connections=4, cookie_file=str(cookie_file))
    events += all_jsonl(capsys)

    assert_clean_pipeline_result(result, out, payload, events=events)

    initial = server.requests_for("/redirect-ranged")
    final = server.requests_for("/file")
    assert initial and final, "redirect chain was not followed"
    assert any("session=secret123" in (r.cookie or "") for r in initial)
    assert all("session=secret123" not in (r.cookie or "") for r in final), (
        "host-scoped cookie leaked to the cross-host redirect target"
    )
    # The user-supplied Bearer header reaches the native call (S2 L4 proof),
    # and S1's native host-scoping keeps it OFF the cross-host redirect target.
    # Both assertions together prove the test is non-vacuous: the token is
    # really supplied, yet really withheld from localhost.
    assert any(r.authorization == "Bearer scoped-token" for r in initial)
    assert all(r.authorization != "Bearer scoped-token" for r in final), (
        "Bearer token leaked to the cross-host redirect target"
    )


# ---------------------------------------------------------------------------
# Scenario 5: resume after interrupt (.mdm.part + .mdm.json -> identical bytes)
# ---------------------------------------------------------------------------


def test_e2e_5_resume_after_interrupt_is_byte_identical(
    server: ThrottlingServer,
    payload: bytes,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    events = capture_jsonl(monkeypatch, capsys)
    digest = "sha256:" + hashlib.sha256(payload).hexdigest()
    out = tmp_path / "resume"
    out.mkdir()
    dest = out / "payload.bin"

    # Native side: start, commit some bytes, interrupt -> sidecars persist.
    server.throttle_bps = 512 * 1024
    handle = start_native(server.url("/file"), dest, connections=8)
    try:
        from test_rust_engine_parity import snap, wait_for

        assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) >= 32 * 1024)
        handle.cancel()
    finally:
        server.throttle_bps = 0.0

    part_files = list(out.rglob("*.mdm.part"))
    state_files = list(out.rglob("*.mdm.json"))
    assert part_files, "no .mdm.part persisted after interruption"
    assert state_files, "no .mdm.json state persisted after interruption"

    # Adapter side: resuming detects the sidecars and completes the transfer.
    info = target_info(
        server.url("/file"), content_length=len(payload), expected_digest=digest
    )
    result = adapter_engine().execute(info, out, connections=8)
    events += all_jsonl(capsys)

    assert_clean_pipeline_result(result, out, payload, events=events)


# ---------------------------------------------------------------------------
# Scenario 6: metalink/torrent stays delegated to aria2 (no native call)
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "url",
    [
        "magnet:?xt=urn:btih:abcdef",
        "https://example.com/releases.meta4",
        "https://example.com/releases.metalink",
    ],
)
def test_e2e_6_metalink_torrent_delegated_to_aria2_no_native_call(
    monkeypatch: pytest.MonkeyPatch, url: str, tmp_path: Path
) -> None:
    from unittest.mock import MagicMock

    import paradl._rust_engine as shim
    from paradl.core.config import get_active_engine, set_active_engine
    from paradl.core.models import DownloadItem
    from paradl.engines import aria2 as aria2_mod

    monkeypatch.setattr(shim, "is_available", lambda: True)
    previous_engine = get_active_engine()
    set_active_engine("rust")
    monkeypatch.setattr("paradl.core.config.get_active_engine", lambda: "rust")

    # A native start_download call would mean native handled a torrent/metalink.
    native_start = MagicMock(name="native_start_download")
    monkeypatch.setattr(shim, "start_download", native_start, raising=False)

    legacy = MagicMock(name="legacy_download_batch")
    rust = MagicMock(name="rust_download_batch")
    monkeypatch.setattr(aria2_mod.Aria2Engine, "_legacy_download_batch", legacy)
    monkeypatch.setattr("paradl.engines.rust_engine.RustEngine.download_batch", rust)

    item = DownloadItem(url=url, out_dir=tmp_path)
    aria2_mod.Aria2Engine.download_batch([item], tmp_path, connections=2)

    legacy.assert_called_once()
    rust.assert_not_called()
    native_start.assert_not_called()
    set_active_engine(previous_engine)
