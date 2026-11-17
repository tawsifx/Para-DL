"""Contract tests for the Python adapter around the native Rust rapids.

TDD for Task 2: a fake native module is injected via ``monkeypatch`` so these
contracts run before Subagent 1's PyO3 extension has been compiled.  They pin
down the four behaviours the adapter must guarantee:

* native result -> ``TransferResult`` with identical ``ExitCode`` values;
* a Watchdog recycle decision calls ``handle.recycle_connections()`` exactly once;
* a SHA-256 mismatch yields ``ExitCode.VERIFICATION_FAILED``;
* the ``paradl._rust_engine`` shim imports cleanly with no native module present.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any
from unittest.mock import MagicMock

import pytest

from paradl.core.models import ExitCode, TargetInfo, TargetType, TransferResult


class _FakeHandle:
    """Minimal stand-in for the native ``DownloadHandle``."""

    def __init__(self, snapshot: dict[str, Any], result: dict[str, Any]) -> None:
        self._snapshot = snapshot
        self.recycle_connections = MagicMock(name="recycle_connections")
        self.pause = MagicMock(name="pause")
        self.resume = MagicMock(name="resume")
        self.cancel = MagicMock(name="cancel")
        self.wait = MagicMock(name="wait", return_value=result)

    def poll_snapshot(self) -> dict[str, Any]:
        return dict(self._snapshot)


class _FakeNative:
    """Minimal stand-in for the ``paradl._rust_engine`` module."""

    def __init__(self, handle: _FakeHandle) -> None:
        self._handle = handle
        self.start_download = MagicMock(name="start_download", return_value=handle)
        self.resume_download = MagicMock(name="resume_download", return_value=handle)
        self.probe_url = MagicMock(
            name="probe_url", return_value={"accept_ranges": True, "total": 1000}
        )

    def is_available(self) -> bool:
        return True

    def version(self) -> str:
        return "0.1.0-fake"


# ---------------------------------------------------------------------------
# (a) ExitCode mapping
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    ("native", "expected"),
    [
        ({"success": True, "exit_code": int(ExitCode.OK)}, ExitCode.OK),
        (
            {"success": False, "exit_code": int(ExitCode.FAILURE), "error": "boom"},
            ExitCode.FAILURE,
        ),
        (
            {"success": False, "exit_code": int(ExitCode.VERIFICATION_FAILED)},
            ExitCode.VERIFICATION_FAILED,
        ),
        (
            {"success": False, "exit_code": int(ExitCode.MISSING_DEPENDENCY)},
            ExitCode.MISSING_DEPENDENCY,
        ),
    ],
)
def test_map_native_result_exit_code(native: dict[str, Any], expected: ExitCode) -> None:
    from paradl.engines.rust_engine import _map_native_result

    result = _map_native_result(native, out_path=Path("/tmp/contract.bin"), duration=1.0)

    assert isinstance(result, TransferResult)
    assert result.exit_code == int(expected)


def test_map_native_result_derives_exit_code_when_absent() -> None:
    from paradl.engines.rust_engine import _map_native_result

    ok = _map_native_result({"success": True}, out_path=Path("/tmp/contract.bin"))
    failure = _map_native_result(
        {"success": False, "error": "network down"}, out_path=Path("/tmp/contract.bin")
    )
    assert ok.exit_code == int(ExitCode.OK)
    assert failure.exit_code == int(ExitCode.FAILURE)


# ---------------------------------------------------------------------------
# (b) Watchdog recycle wiring
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "action",
    [
        "resilience_recycle",
        "tier2_pause_unpause",
        "adjust_connections",
    ],
)
def test_watchdog_recycle_invokes_handle_once(action: str) -> None:
    from paradl.engines.watchdog import (
        WatchdogActionType,
        WatchdogDecision,
        apply_recycle_decision,
    )

    handle = MagicMock()
    decision = WatchdogDecision(action=WatchdogActionType(action), reason="cycle", cycle=1)

    assert apply_recycle_decision(decision, handle) is True
    handle.recycle_connections.assert_called_once_with()


def test_watchdog_no_action_does_not_recycle() -> None:
    from paradl.engines.watchdog import (
        WatchdogActionType,
        WatchdogDecision,
        apply_recycle_decision,
    )

    handle = MagicMock()
    decision = WatchdogDecision(action=WatchdogActionType.NONE)

    assert apply_recycle_decision(decision, handle) is False
    handle.recycle_connections.assert_not_called()


# ---------------------------------------------------------------------------
# (c) Checksum mismatch via the adapter end-to-end
# ---------------------------------------------------------------------------


def test_checksum_mismatch_yields_verification_failed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import paradl.engines.rust_engine as rust_engine

    payload = b"hello world"
    dest = tmp_path / "file.bin"
    dest.write_bytes(payload)
    bad_digest = "sha256:" + "0" * 64

    snapshot = {
        "downloaded": len(payload),
        "total": len(payload),
        "speed_bps": 100.0,
        "connections": 4,
        "state": "complete",
    }
    native_result = {
        "success": True,
        "exit_code": int(ExitCode.OK),
        "file_path": str(dest),
        "downloaded": len(payload),
    }
    handle = _FakeHandle(snapshot, native_result)
    monkeypatch.setattr(rust_engine, "_native", _FakeNative(handle))

    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
        expected_digest=bad_digest,
        content_length=len(payload),
    )
    result = rust_engine.RustEngine().execute(info, tmp_path, connections=4, poll_interval=0.0)

    assert result.success is False
    assert result.exit_code == int(ExitCode.VERIFICATION_FAILED)
    assert result.checksum_verified is False


# ---------------------------------------------------------------------------
# (d) Shim imports cleanly with no compiled native module
# ---------------------------------------------------------------------------


def test_shim_imports_cleanly_without_native(monkeypatch: pytest.MonkeyPatch) -> None:
    import importlib.util

    import paradl

    shim_path = Path(str(paradl.__file__)).with_name("_rust_engine.py")
    spec = importlib.util.spec_from_file_location("contract_test_rust_shim", shim_path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)

    real_import = importlib.import_module

    def _blocked(name: str, *args: Any, **kwargs: Any) -> Any:
        if name.startswith(("paradl._rust", "_paradl_rs", "_rust_engine", "paradl._mdm")):
            raise ImportError("native module intentionally blocked")
        return real_import(name, *args, **kwargs)

    with monkeypatch.context() as mp:
        mp.setattr(importlib, "import_module", _blocked)
        spec.loader.exec_module(module)

    assert module.is_available() is False
    for name in (
        "RustDownloadEngine",
        "DownloadHandle",
        "probe_url",
        "start_download",
        "resume_download",
    ):
        assert hasattr(module, name), name
    with pytest.raises(module.RustEngineUnavailable):
        module.start_download("https://example.com/file", "/tmp/contract.bin")


# ---------------------------------------------------------------------------
# (e) CQ-2: resilience events must reach the Rust watchdog -> recycle handle
# ---------------------------------------------------------------------------


class _SequenceHandle(_FakeHandle):
    """Handle whose snapshots progress from running to complete across ticks."""

    def __init__(self, snapshots: list[dict[str, Any]], result: dict[str, Any]) -> None:
        self._snapshots = list(snapshots)
        self._calls = 0
        self.recycle_connections = MagicMock(name="recycle_connections")
        self.pause = MagicMock(name="pause")
        self.resume = MagicMock(name="resume")
        self.cancel = MagicMock(name="cancel")
        self.wait = MagicMock(name="wait", return_value=result)

    def poll_snapshot(self) -> dict[str, Any]:
        index = min(len(self._snapshots) - 1, self._calls)
        self._calls += 1
        return dict(self._snapshots[index])


class _FakeEventSource:
    """Event source emitting a queued event once, then idle."""

    def __init__(self, events: list[Any]) -> None:
        self._events = list(events)
        self.reset_progress_timer = MagicMock(name="reset_progress_timer")

    def __enter__(self) -> _FakeEventSource:
        return self

    def __exit__(self, *args: Any) -> bool:
        return False

    def tick(self, downloaded_bytes: int = 0) -> Any:
        return self._events.pop(0) if self._events else None


@pytest.mark.parametrize("event_name", ["net_change", "stall"])
def test_resilience_event_triggers_recycle_in_rust_mode(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, event_name: str
) -> None:
    import paradl.engines.rust_engine as rust_engine
    from paradl.resilience.events import ResilienceEvent, ResilienceEventType

    payload = b"abcdef"
    dest = tmp_path / "file.bin"
    dest.write_bytes(payload)
    running = {
        "downloaded": 1,
        "total": len(payload),
        "speed_bps": 0.0,
        "connections": 4,
        "state": "running",
    }
    done = {
        "downloaded": len(payload),
        "total": len(payload),
        "speed_bps": 100.0,
        "connections": 4,
        "state": "complete",
    }
    handle = _SequenceHandle(
        [running, done],
        {"success": True, "file_path": str(dest), "downloaded": len(payload)},
    )
    monkeypatch.setattr(rust_engine, "_native", _FakeNative(handle))

    event = ResilienceEvent(
        event_type=ResilienceEventType(event_name),
        reason=f"injected {event_name}",
        timestamp=0.0,
    )
    source = _FakeEventSource([event])
    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
        content_length=len(payload),
    )
    result = rust_engine.RustEngine().execute(
        info,
        tmp_path,
        connections=4,
        poll_interval=0.0,
        event_source=source,
    )

    assert result.success is True
    assert result.exit_code == int(ExitCode.OK)
    handle.recycle_connections.assert_called_once_with()


# ---------------------------------------------------------------------------
# (f) CQ-3: native-equivalent flags must reach start_download
# ---------------------------------------------------------------------------


class _StrictNative:
    """Stub mirroring the exact PyO3 signature (no ``**kwargs``).

    Mirrors ``rust/paradl-rs/src/lib.rs`` ``start_download``: PyO3 filters
    keyword arguments by declared parameter name, so wrong key names are
    silently dropped.  A MagicMock with ``(*args, **kwargs)`` cannot detect
    that; this stub records only what the adapter actually passed.
    """

    def __init__(self, handle: Any) -> None:
        self.handle = handle
        self.calls: list[dict[str, Any]] = []

    def start_download(
        self,
        url: str,
        out_dir: str,
        filename: str | None = None,
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
                "out_dir": out_dir,
                "filename": filename,
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
        return self.handle

    def is_available(self) -> bool:
        return True


def test_native_equivalent_flags_reach_start_download(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    import paradl.engines.rust_engine as rust_engine

    payload = b"hello"
    dest = tmp_path / "file.bin"
    dest.write_bytes(payload)
    snapshot = {
        "downloaded": len(payload),
        "total": len(payload),
        "speed_bps": 100.0,
        "connections": 4,
        "state": "complete",
    }
    handle = _FakeHandle(
        snapshot,
        {"success": True, "file_path": str(dest), "downloaded": len(payload)},
    )
    fake_native = _StrictNative(handle)
    monkeypatch.setattr(rust_engine, "_native", fake_native)

    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
        headers={"Authorization": "Bearer token"},
        cookie_file="/tmp/cookies.txt",
        referer="https://example.com/",
        user_agent="paradl-test",
        expected_digest="sha256:" + "0" * 64,
    )
    result = rust_engine.RustEngine().execute(
        info,
        tmp_path,
        connections=4,
        poll_interval=0.0,
        limit_rate="1MB/s",
        ip_family="ipv4",
        proxy="socks5://127.0.0.1:1080",
    )

    assert result.success is False  # deliberately bad digest
    assert len(fake_native.calls) == 1
    call = fake_native.calls[0]
    assert call["ip_family"] == "ipv4"
    assert call["proxy"] == "socks5://127.0.0.1:1080"
    assert call["rate_limit_bps"] is not None
    # The compiled extension declares `checksum` / `cookies`, NOT
    # `expected_digest` / `cookie_file`; wrong keys are silently filtered out.
    assert call["checksum"] == info.expected_digest
    assert call["cookies"] == "/tmp/cookies.txt"
    assert call["user_agent"] == "paradl-test"
    assert call["headers"] == {"Authorization": "Bearer token"}


# ---------------------------------------------------------------------------
# (g) CQ-4: metalink/magnet batches must stay on legacy aria2
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "url",
    [
        "magnet:?xt=urn:btih:abcdef",
        "https://example.com/releases.meta4",
        "https://example.com/releases.metalink",
    ],
)
def test_metalink_and_magnet_batch_stay_on_legacy_aria2(
    monkeypatch: pytest.MonkeyPatch, url: str
) -> None:
    import paradl._rust_engine as shim
    from paradl.core.config import get_active_engine, set_active_engine
    from paradl.core.models import DownloadItem
    from paradl.engines import aria2 as aria2_mod

    monkeypatch.setattr(shim, "is_available", lambda: True)
    previous_engine = get_active_engine()
    set_active_engine("rust")
    monkeypatch.setattr(
        "paradl.core.config.get_active_engine", lambda: "rust"
    )

    legacy = MagicMock(name="legacy_download_batch")
    rust = MagicMock(name="rust_download_batch")
    monkeypatch.setattr(aria2_mod.Aria2Engine, "_legacy_download_batch", legacy)
    monkeypatch.setattr(
        "paradl.engines.rust_engine.RustEngine.download_batch", rust
    )

    item = DownloadItem(url=url, out_dir=Path("/tmp"))
    aria2_mod.Aria2Engine.download_batch([item], Path("/tmp"), connections=2)

    legacy.assert_called_once()
    rust.assert_not_called()
    set_active_engine(previous_engine)


def test_metalink_path_single_target_stays_on_legacy_aria2(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import paradl._rust_engine as shim
    from paradl.core.models import TargetInfo, TargetType
    from paradl.engines import aria2 as aria2_mod

    monkeypatch.setattr(shim, "is_available", lambda: True)
    monkeypatch.setattr(
        "paradl.core.config.get_active_engine", lambda: "rust"
    )
    legacy = MagicMock(name="legacy_download")
    rust = MagicMock(name="rust_download")
    monkeypatch.setattr(aria2_mod.Aria2Engine, "_legacy_download", legacy)
    monkeypatch.setattr("paradl.engines.rust_engine.RustEngine.download", rust)

    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        metalink_path=Path("/tmp/releases.meta4"),
    )
    aria2_mod.Aria2Engine.download(info, Path("/tmp"), connections=2)

    legacy.assert_called_once()
    rust.assert_not_called()


# ---------------------------------------------------------------------------
# (h) Native exceptions must never escape the Engine protocol boundary
# ---------------------------------------------------------------------------


def _native_exception(name: str, message: str) -> Exception:
    """Build a native exception, falling back to a local stand-in if needed."""
    import paradl._rust_engine as native

    cls = getattr(native, name, None)
    if not isinstance(cls, type) or not issubclass(cls, BaseException):

        class _Fallback(RuntimeError):
            pass

        cls = _Fallback
    return cls(message)  # type: ignore[return-value]


class _RaisingHandle(_SequenceHandle):
    """Terminal snapshot, but wait() raises like the compiled extension."""

    def __init__(self, error: Exception) -> None:
        snapshots = [
            {
                "downloaded": 1,
                "total": 2,
                "speed_bps": 0.0,
                "connections": 4,
                "state": "running",
            },
            {
                "downloaded": 2,
                "total": 2,
                "speed_bps": 100.0,
                "connections": 4,
                "state": "complete",
            },
        ]
        super().__init__(snapshots, {})
        self.wait = MagicMock(name="wait", side_effect=error)


def _assert_native_error_mapping(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    error: Exception,
    expected_exit_code: int,
    *,
    expected_checksum_verified: bool | None = None,
) -> TransferResult:
    import paradl.engines.rust_engine as rust_engine

    monkeypatch.setattr(rust_engine, "_native", _FakeNative(_RaisingHandle(error)))
    info = TargetInfo(
        raw="https://example.com/file.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/file.bin",
        filename_hint="file.bin",
        expected_digest="sha256:" + "0" * 64,
    )
    result = rust_engine.RustEngine().execute(
        info, tmp_path, connections=2, poll_interval=0.0
    )
    assert isinstance(result, TransferResult)
    assert result.success is False
    assert result.exit_code == expected_exit_code, result.error
    assert str(error) in (result.error or "")
    if expected_checksum_verified is not None:
        assert result.checksum_verified is expected_checksum_verified
    return result


def test_verification_error_maps_to_verification_failed(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    error = _native_exception("VerificationError", "checksum mismatch after download")
    _assert_native_error_mapping(
        monkeypatch,
        tmp_path,
        error,
        int(ExitCode.VERIFICATION_FAILED),
        expected_checksum_verified=False,
    )


def test_download_error_maps_to_failure(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    error = _native_exception("DownloadError", "download failed: HTTP 500")
    _assert_native_error_mapping(
        monkeypatch, tmp_path, error, int(ExitCode.FAILURE)
    )
