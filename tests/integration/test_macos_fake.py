"""Integration tests: macOS platform boundaries, extended attributes, Gatekeeper, and notifications."""

from __future__ import annotations

import io
import json
from pathlib import Path
from typing import Any

import pytest
from tests.support.macos_fake import get_fake_runner

from pdl.engines.github import BinaryInstaller, GitHubEngine
from pdl.hooks import HookRunner
from pdl.models import TransferResult
from pdl.platform import macos

pytestmark = pytest.mark.integration


def test_xattr_where_froms_roundtrip(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify setting and reading com.apple.metadata:kMDItemWhereFroms binary plist attribute."""
    test_file = tmp_path / "downloaded_file.iso"
    test_file.write_bytes(b"content")

    get_fake_runner()
    monkeypatch.setattr(macos, "is_effective_macos", lambda: True)

    url = "https://example.com/huge-file.iso"
    success = macos.set_where_froms(test_file, [url])
    assert success is True

    read_urls = macos.get_where_froms(test_file)
    assert read_urls == [url]


def test_install_verification_outcomes(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify codesign, spctl, ad-hoc sign (--sign), and quarantine removal behavior."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()

    asset_file = tmp_path / "sample_tool"
    asset_file.write_text("#!/bin/sh\necho 'v1.0.0'\n")
    asset_file.chmod(0o755)

    codesign_called: list[str] = []
    spctl_called: list[str] = []
    adhoc_signed: list[str] = []
    quarantine_removed: list[str] = []

    monkeypatch.setattr(macos, "codesign_verify", lambda p: (codesign_called.append(str(p)), (True, "valid"))[1])
    monkeypatch.setattr(macos, "spctl_assess", lambda p: (spctl_called.append(str(p)), (True, "accepted"))[1])
    monkeypatch.setattr(macos, "codesign_adhoc_sign", lambda p: (adhoc_signed.append(str(p)), (True, "signed"))[1])
    monkeypatch.setattr(macos, "remove_quarantine", lambda p: (quarantine_removed.append(str(p)), True)[1])

    def make_mock_dl(verified: bool):
        def _mock(*, out_dir: Path, **kw: Any):
            asset_path = out_dir / "sample_tool"
            asset_path.write_bytes(b"\xcf\xfa\xed\xfe" + b"\x00" * 60)
            asset_path.chmod(0o755)
            return (
                TransferResult(success=True, file_path=asset_path, checksum_verified=verified),
                {"name": "sample_tool", "browser_download_url": "https://gh.com/sample_tool"},
                [],
            )

        return _mock

    # 1. Verified by pdl checksum + --sign False
    monkeypatch.setattr(GitHubEngine, "download_release_asset", make_mock_dl(True))
    monkeypatch.setattr(GitHubEngine, "extract_asset_digest", lambda a: "sha256:1234")
    from pdl.postproc.checksum import ChecksumEngine

    monkeypatch.setattr(ChecksumEngine, "verify", lambda p, dig: (True, "sha256", "1234"))

    res1 = BinaryInstaller.install("owner/sample_tool", bin_dir, sign=False)
    assert res1.success is True
    assert len(codesign_called) == 1
    assert len(spctl_called) == 1
    assert len(adhoc_signed) == 0
    assert len(quarantine_removed) == 1

    # 2. Verified by pdl checksum + --sign True -> ad-hoc sign runs
    codesign_called.clear()
    spctl_called.clear()
    adhoc_signed.clear()
    quarantine_removed.clear()

    res2 = BinaryInstaller.install("owner/sample_tool", bin_dir, sign=True)
    assert res2.success is True
    assert len(adhoc_signed) == 1
    assert len(quarantine_removed) == 1

    # 3. Unverified (no checksum, --yes used) -> quarantine NOT removed
    codesign_called.clear()
    spctl_called.clear()
    adhoc_signed.clear()
    quarantine_removed.clear()

    monkeypatch.setattr(GitHubEngine, "extract_asset_digest", lambda a: None)
    monkeypatch.setattr(GitHubEngine, "find_release_companion_checksum", lambda a, b: None)
    monkeypatch.setattr(GitHubEngine, "download_release_asset", make_mock_dl(False))

    res3 = BinaryInstaller.install("owner/sample_tool", bin_dir, assume_yes=True, sign=False)
    assert res3.success is True
    assert len(quarantine_removed) == 0


def test_hook_runner_isolation_and_timeout(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify that hooks never block or crash pdl, and time out cleanly."""
    pdl_home = tmp_path / "pdl_home"
    monkeypatch.setenv("PDL_HOME", str(pdl_home))

    complete_hooks = pdl_home / "hooks" / "on_complete"
    complete_hooks.mkdir(parents=True)

    # 1. Normal working hook
    out_file = tmp_path / "hook_received.json"
    good_hook = complete_hooks / "01_good_hook.sh"
    good_hook.write_text(f"#!/bin/sh\ncat > '{out_file}'\nexit 0\n")
    good_hook.chmod(0o755)

    payload = {"target": "https://example.com/file.zip", "success": True, "bytes_downloaded": 1024}
    HookRunner.run_result_actions(payload, is_success=True, notify=False)

    assert out_file.exists()
    received_data = json.loads(out_file.read_text())
    assert received_data["target"] == "https://example.com/file.zip"

    # 2. Crashing hook exits 1 cleanly without raising
    bad_hook = complete_hooks / "02_crash_hook.sh"
    bad_hook.write_text("#!/bin/sh\nexit 1\n")
    bad_hook.chmod(0o755)
    HookRunner.run_result_actions(payload, is_success=True, notify=False)

    # 3. Fast timeout hook check
    hang_hook = complete_hooks / "03_hang_hook.sh"
    hang_hook.write_text("#!/bin/sh\nsleep 5\nexit 0\n")
    hang_hook.chmod(0o755)
    HookRunner.run_result_actions(payload, is_success=True, notify=False, timeout=0.05)


def test_hook_runner_notifications(isolated_pdl_home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify built-in --notify actions for success, failure, watchdog alert, and queue completion."""
    runner = get_fake_runner()
    monkeypatch.setattr(macos, "is_effective_macos", lambda: True)

    payload_fail = {"target": "file.bin", "error": "Disk full"}
    HookRunner.run_result_actions(payload_fail, is_success=False, notify=True)
    assert any("pdl - Download Failed" in n["script"] for n in runner.notifications)

    payload_ok = {"target": "file.bin", "bytes_downloaded": 2048}
    HookRunner.run_result_actions(payload_ok, is_success=True, notify=True)
    assert any("pdl - Download Complete" in n["script"] for n in runner.notifications)

    HookRunner.notify_watchdog_giveup("example.com", "recycled 3 times", enabled=True)
    assert any("Watchdog Alert" in n["script"] for n in runner.notifications)

    HookRunner.notify_queue_summary(5, 4, 1, enabled=True)
    assert any("Queue Complete" in n["script"] for n in runner.notifications)


def test_notification_command_construction(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify native notification properly escapes titles and messages in AppleScript."""
    runner = get_fake_runner()
    monkeypatch.setattr(macos, "is_effective_macos", lambda: True)

    title = 'Test "Title" with quotes'
    msg = 'Message with "double quotes" and \\backslash'
    sub = 'Subtitle "sub"'

    ok = macos.display_notification(title, msg, subtitle=sub)
    assert ok is True
    assert len(runner.notifications) == 1
    script = runner.notifications[0]["script"]
    assert '\\"double quotes\\"' in script
    assert '\\"Title\\"' in script
    assert '\\"sub\\"' in script


def test_macos_watcher_scutil_route_and_nwi_fallbacks() -> None:
    """Verify scutil interactive commands and the fallback chain to route -n monitor and scutil --nwi."""

    class FakeScutilProc:
        def __init__(self):
            self.stdin = io.StringIO()
            self.stdout = io.StringIO("changed key [0] = State:/Network/Global/IPv4\n")
            self.stderr = io.StringIO()
            self.returncode = 0

        def poll(self):
            return None

        def terminate(self):
            pass

        def kill(self):
            pass

        def wait(self, timeout=None):
            return 0

    def _scutil_popen(cmd: list[str], **kwargs: Any) -> Any:
        return FakeScutilProc()

    watcher = macos.MacOSNetworkWatcher(enabled=True, popen_factory=_scutil_popen)
    backend = watcher.start()
    assert backend == "scutil"
    notes = watcher.poll_notifications()
    assert any("State:/Network/Global/IPv4" in n for n in notes)
    watcher.stop()

    # Route monitor fallback
    class FakeRouteProc:
        def __init__(self):
            self.stdin = None
            self.stdout = io.StringIO("got message --> RTM_NEWADDR: address added: if en0\n")
            self.stderr = io.StringIO()
            self.returncode = 0

        def poll(self):
            return None

        def terminate(self):
            pass

        def kill(self):
            pass

        def wait(self, timeout=None):
            return 0

    def _route_fallback_popen(cmd: list[str], **kwargs: Any) -> Any:
        if "scutil" in cmd[0]:
            raise FileNotFoundError("scutil missing")
        return FakeRouteProc()

    watcher_route = macos.MacOSNetworkWatcher(enabled=True, popen_factory=_route_fallback_popen)
    assert watcher_route.start() == "route_monitor"
    import time

    time.sleep(0.05)
    notes_route = watcher_route.poll_notifications()
    assert any("RTM_NEWADDR" in n for n in notes_route)
    watcher_route.stop()

    # NWI fallback
    def _fail_popen(cmd: list[str], **kwargs: Any) -> Any:
        raise FileNotFoundError("missing")

    sample_nwi_1 = (
        "IPv4 network interface information\n   en0 : flags : 0x5\n   address : 192.168.0.10\nNetwork interfaces: en0\n"
    )
    sample_nwi_2 = (
        "IPv4 network interface information\n   en0 : flags : 0x5\n   address : 10.0.0.42\nNetwork interfaces: en0\n"
    )
    nwi_counter = [0]

    def _fake_nwi() -> str:
        nwi_counter[0] += 1
        return sample_nwi_1 if nwi_counter[0] == 1 else sample_nwi_2

    clock_val = [100.0]
    watcher_nwi = macos.MacOSNetworkWatcher(
        enabled=True,
        nwi_poll_interval_sec=5.0,
        popen_factory=_fail_popen,
        nwi_runner=_fake_nwi,
        mono_clock=lambda: clock_val[0],
    )
    assert watcher_nwi.start() == "scutil_nwi_poll"
    assert watcher_nwi.poll_notifications() == []
    clock_val[0] += 6.0
    nwi_notes = watcher_nwi.poll_notifications()
    assert len(nwi_notes) == 1
    assert "scutil_nwi" in nwi_notes[0]
    watcher_nwi.stop()
