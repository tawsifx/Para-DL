"""Watchdog v2 socket-recycle parity tests for the native Rust engine.

These are the regression tests for the bug diagnosed in
``docs/diagnostics/watchdog-cycle-aria2-bug.md``: a forced socket recycle on a
redirected/throttled transfer used to make aria2 drop its (redirect-derived)
URI and emit ``errorCode=8, 'No URI available.'``, leaving a truncated file.

The Rust engine must recycle *in process* at committed byte offsets: the
offset must never regress, the connection pool must be restored, progress must
continue after the recycle, and the final file must be byte-identical.

The loopback server and compatibility helpers are defined in
``test_rust_engine_parity`` (flat tests/ layout, so it is importable directly).
"""

from __future__ import annotations

import importlib
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

from test_rust_engine_parity import (  # noqa: E402  (deliberately after availability gate)
    ThrottlingServer,
    resume_native,
    snap,
    start_native,
    terminal_success,
    wait_for,
    wait_handle,
)


@pytest.fixture
def payload() -> bytes:
    return bytes(((i * 37 + 11) % 256) for i in range(1024 * 1024))


@pytest.fixture
def server(payload: bytes) -> Any:
    srv = ThrottlingServer(payload=payload, throttle_bps=64 * 1024, chunk_size=2048)
    srv.start()
    try:
        yield srv
    finally:
        srv.close()


def test_recycle_midthrottle_preserves_offset_and_uri(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    """The exact watchdog-cycle bug: a 302-derived URI must survive a recycle.

    The transfer starts at a redirect whose target is a range-capable loopback
    host. That makes the effective URI *redirect-derived* -- the thing aria2
    dropped on pause/unpause in the diagnosis -- while still permitting 16
    throttled, segmented channels to measure offset preservation.
    """
    dest = tmp_path / "recycle.bin"
    handle = start_native(server.url("/redirect-ranged"), dest, connections=16)

    # Let the throttled transfer commit a measurable number of bytes.
    assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) >= 32 * 1024), (
        "throttled transfer never made progress before recycle"
    )
    before = int(snap(handle.poll_snapshot(), "downloaded"))
    conns_before = int(snap(handle.poll_snapshot(), "connections") or 0)
    ranges_before = len(server.ranges)
    assert conns_before == 16, f"expected 16 channels before recycle, got {conns_before}"

    # Trigger the in-process recycle while the server is still throttled.
    recycler = getattr(handle, "recycle_connections", None)
    assert recycler is not None, "DownloadHandle.recycle_connections() is required"
    recycler()

    # Offsets are committed, never rolled back (no truncation on recycle).
    after = int(snap(handle.poll_snapshot(), "downloaded"))
    assert after >= before, f"byte offset regressed across recycle: {before} -> {after}"

    # The URI pool / connection budget is restored, not dropped.
    conns_after = int(snap(handle.poll_snapshot(), "connections") or 0)
    assert conns_after == 16, f"connection pool lost after recycle: {conns_after}"

    # Progress must continue after the recycle (aria2's 'No URI available' bug
    # manifests as an immediate stall/error here).
    assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) > after), (
        "transfer stalled after recycle -- URI pool was lost"
    )

    # Unthrottle and let it finish; the final file must be byte-identical.
    server.throttle_bps = 0.0
    wait_handle(handle)

    assert dest.is_file(), "final file missing after recycle"
    assert dest.read_bytes() == payload, "recycle truncated or corrupted the output file"

    # The range requests must have gone to the redirect target on the *other*
    # host name; otherwise this is not exercising the redirect-derived URI.
    assert any(r.host and "localhost" in r.host for r in server.requests_for("/file")), (
        "redirect target was never used -- recycle did not exercise a redirected URI"
    )

    # Uncommitted claims were re-fetched from a committed offset.
    assert len(server.ranges) > ranges_before, "no new range requests after recycle"


def test_recycle_on_redirected_no_ranges_target_is_not_false_complete(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    """A 302 to a host without Accept-Ranges must still deliver all bytes.

    The diagnosis showed aria2 jumping to 100% while leaving only a few KB on
    disk after a watchdog cycle on a redirected transfer.  Recycled Rust
    transfers must retain the redirected URI and write every byte.
    """
    dest = tmp_path / "redirect.bin"
    handle = start_native(server.url("/redirect"), dest, connections=4)

    assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) >= 16 * 1024)
    after = int(snap(handle.poll_snapshot(), "downloaded"))
    handle.recycle_connections()

    # Progress must continue after the recycle: a lost redirect-derived URI
    # would stall here instead of resuming from the committed offset.
    assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) > after), (
        "transfer stalled after recycle -- redirected URI was lost"
    )

    server.throttle_bps = 0.0
    status = wait_handle(handle)

    assert dest.is_file()
    assert dest.read_bytes() == payload
    assert terminal_success(status, handle.poll_snapshot()), (
        f"final status is not success: {status!r}"
    )
    assert server.requests_for("/final"), "redirect target was never fetched"
    total = int(snap(handle.poll_snapshot(), "total") or 0)
    assert total == len(payload), f"reported total {total} != {len(payload)}"


def test_crash_resume_from_part_state_is_byte_identical(
    server: ThrottlingServer, payload: bytes, tmp_path: Path
) -> None:
    """Start, interrupt, resume from .part state, verify identical bytes."""
    dest = tmp_path / "resume.bin"
    handle = start_native(server.url("/file"), dest, connections=8)

    assert wait_for(lambda: int(snap(handle.poll_snapshot(), "downloaded") or 0) >= 32 * 1024)
    committed = int(snap(handle.poll_snapshot(), "downloaded"))

    canceller = getattr(handle, "cancel", None)
    assert canceller is not None, "DownloadHandle.cancel() is required for crash simulation"
    canceller()

    part_files = sorted(tmp_path.rglob("*.part"))
    assert part_files, "no .part state persisted after interruption"
    assert b"".join(p.read_bytes() for p in part_files)[:committed], "part state is empty"

    # Resume from the persisted state with the server no longer throttled.
    server.throttle_bps = 0.0
    resumed = resume_native(server.url("/file"), dest, connections=8)
    wait_handle(resumed)

    assert dest.is_file(), "resume did not produce the final file"
    assert dest.read_bytes() == payload, "resumed file is not byte-identical"
