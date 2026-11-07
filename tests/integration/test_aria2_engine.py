"""Integration tests: Aria2RpcDaemon JSON-RPC client, options translation, and socket recycling."""

from __future__ import annotations

from pathlib import Path

import pytest
from tests.support.fakes import FakeAria2Server

from pdl.engines.aria2 import Aria2Engine
from pdl.models import ExitCode, TargetInfo, TargetType
from pdl.platform.macos import PowerStatus

pytestmark = pytest.mark.integration


def test_fake_aria2_rpc_daemon_call_sequence() -> None:
    """Verify JSON-RPC call serialization, token authorization, and method responses."""
    fake_srv = FakeAria2Server(secret="secret_tok_123")
    res_ver = fake_srv.call("aria2.getVersion", "token:secret_tok_123")
    assert "version" in res_ver
    assert res_ver["version"] == "1.37.0"

    # Unauthorized token raises PermissionError
    with pytest.raises(PermissionError):
        fake_srv.call("aria2.getVersion", "token:wrong_token")

    # Add URI
    gid = fake_srv.call(
        "aria2.addUri",
        "token:secret_tok_123",
        ["https://example.com/file.iso"],
        {"dir": "/tmp", "split": "8"},
    )
    assert gid is not None
    assert fake_srv.options[gid]["split"] == "8"

    # Status & Global Stat
    status = fake_srv.call("aria2.tellStatus", "token:secret_tok_123", gid, [])
    assert status["status"] == "active"

    gstat = fake_srv.call("aria2.getGlobalStat", "token:secret_tok_123")
    assert int(gstat["numActive"]) == 1

    # Pause and unpause
    fake_srv.call("aria2.forcePause", "token:secret_tok_123", gid)
    assert fake_srv.downloads[gid]["status"] == "paused"

    fake_srv.call("aria2.unpause", "token:secret_tok_123", gid)
    assert fake_srv.downloads[gid]["status"] == "active"


def test_aria2_engine_missing_binary(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """When aria2c binary is missing, return MISSING_DEPENDENCY exit code with clear error."""
    from pdl.toolchain import TOOLCHAIN

    monkeypatch.setattr(type(TOOLCHAIN), "aria2c", property(lambda self: None))

    t_info = TargetInfo(
        raw="https://example.com/test.bin", target_type=TargetType.DIRECT, clean_url="https://example.com/test.bin"
    )
    res = Aria2Engine.download(t_info, tmp_path)
    assert res.success is False
    assert res.exit_code == int(ExitCode.MISSING_DEPENDENCY)
    assert "aria2c binary not found" in (res.error or "")


def test_aria2_engine_insufficient_disk_guard(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Insufficient disk space triggers start-up guard failure without launching transfer."""
    from pdl.platform import macos

    monkeypatch.setattr(macos, "get_free_disk_bytes", lambda p: 1024)

    t_info = TargetInfo(
        raw="https://example.com/huge.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/huge.bin",
        content_length=100 * 1024 * 1024,  # 100 MB
        accept_ranges=True,
    )
    res = Aria2Engine.download(t_info, tmp_path)
    assert res.success is False
    assert res.exit_code == int(ExitCode.FAILURE)
    assert "Insufficient disk space" in (res.error or "")


def test_aria2_engine_power_policy_guard(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    """Require AC power guard blocks download when running on battery."""
    from pdl.platform import macos

    monkeypatch.setattr(
        macos,
        "get_power_status",
        lambda: PowerStatus(on_ac_power=False, battery_percent=45, low_power_mode=False, raw_source="Battery Power"),
    )

    t_info = TargetInfo(
        raw="https://example.com/test.bin",
        target_type=TargetType.DIRECT,
        clean_url="https://example.com/test.bin",
    )
    res = Aria2Engine.download(t_info, tmp_path, require_ac=True)
    assert res.success is False
    assert res.exit_code == int(ExitCode.FAILURE)
    assert "AC power is required" in (res.error or "")


def test_aria2_temp_cleanup_and_helpers(tmp_path: Path) -> None:
    """Verify temp file cleanup, active connection counting, and rate string parsing."""
    from pdl.engines.aria2 import (
        _ACTIVE_TEMP_FILES,
        _count_active_connections,
        cleanup_children_and_temps,
        register_temp_path,
        unregister_temp_path,
    )

    t_file = tmp_path / "temp_secret.bin"
    t_file.write_text("secret")
    aria_ctrl = tmp_path / "temp_secret.bin.aria2"
    aria_ctrl.write_text("ctrl_bytes")

    register_temp_path(t_file)
    assert t_file in _ACTIVE_TEMP_FILES
    unregister_temp_path(t_file)
    assert t_file not in _ACTIVE_TEMP_FILES

    register_temp_path(t_file)
    cleanup_children_and_temps(remove_partial_raw=True)
    assert not t_file.exists()
    assert not aria_ctrl.exists()

    # Connection counting
    servers = [{"servers": [{"downloadSpeed": "102400"}, {"downloadSpeed": "204800"}]}]
    count, speeds = _count_active_connections(servers)
    assert count == 2
    assert speeds == [102400.0, 204800.0]

    assert Aria2Engine.parse_speed_str("500KB/s") == 500.0 * 1024
