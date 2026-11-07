"""Integration tests: pdl doctor system diagnostics, yt-dlp age, and JSON event formatting."""

from __future__ import annotations

import subprocess
from datetime import datetime, timedelta
from typing import Any

import pytest

from pdl.doctor import get_ytdlp_version_and_age, run_doctor
from pdl.models import ExitCode
from pdl.ui import UI

pytestmark = pytest.mark.integration


def test_doctor_detects_ytdlp_staleness(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify that yt-dlp older than 30 days triggers an outdated warning."""
    stale_date = datetime.now() - timedelta(days=45)
    stale_ver = stale_date.strftime("%Y.%m.%d")

    from pdl.toolchain import TOOLCHAIN

    monkeypatch.setattr(type(TOOLCHAIN), "ytdlp", property(lambda self: "/mock/bin/yt-dlp"))

    def mock_run(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        if "--version" in cmd:
            return subprocess.CompletedProcess(cmd, 0, stdout=stale_ver, stderr="")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(subprocess, "run", mock_run)

    ver, age, outdated = get_ytdlp_version_and_age()
    assert ver == stale_ver
    assert age is not None and age >= 45
    assert outdated is True

    # Fresh version (5 days old)
    fresh_date = datetime.now() - timedelta(days=5)
    fresh_ver = fresh_date.strftime("%Y.%m.%d")

    def mock_run_fresh(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        if "--version" in cmd:
            return subprocess.CompletedProcess(cmd, 0, stdout=fresh_ver, stderr="")
        return subprocess.CompletedProcess(cmd, 0, stdout="", stderr="")

    monkeypatch.setattr(subprocess, "run", mock_run_fresh)

    ver_f, age_f, outdated_f = get_ytdlp_version_and_age()
    assert ver_f == fresh_ver
    assert age_f is not None and age_f <= 6
    assert outdated_f is False


def test_doctor_json_mode(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify 'pdl doctor --json' emits doctor event with complete diagnostics dictionary."""
    events: list[dict[str, Any]] = []

    monkeypatch.setattr(UI, "json_mode", True)
    monkeypatch.setattr(UI, "emit_event", lambda ev, **kw: events.append({"event": ev, **kw}))

    rc = run_doctor()
    assert rc == int(ExitCode.OK)
    assert len(events) >= 1
    doc_ev = next(e for e in events if e["event"] == "doctor")
    assert "ytdlp" in doc_ev
    assert "auth" in doc_ev
    assert "macos_utilities" in doc_ev
    assert "core_tools" in doc_ev
    assert "fixes" in doc_ev


def test_probe_http3_support(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify HTTP/3 curl prober with presence and absence of curl binary."""
    from pdl.netprobe import probe_http3_support
    from pdl.toolchain import TOOLCHAIN

    # 1. Missing curl
    monkeypatch.setattr(type(TOOLCHAIN), "curl", property(lambda self: None))
    ok, msg = probe_http3_support("https://example.com")
    assert ok is False
    assert "not found" in msg

    # 2. curl returns HTTP/3
    monkeypatch.setattr(type(TOOLCHAIN), "curl", property(lambda self: "/usr/bin/curl"))

    def mock_run(cmd: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(cmd, 0, stdout="HTTP/3 200 OK\r\n", stderr="")

    monkeypatch.setattr(subprocess, "run", mock_run)
    ok, msg = probe_http3_support("https://example.com")
    assert ok is True
    assert "HTTP/3" in msg
