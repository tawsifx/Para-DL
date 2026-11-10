"""Tests for core modules platform unification (paths, config, events, auth, hooks, doctor)."""

from __future__ import annotations

import os
import unittest
from pathlib import Path
from unittest.mock import patch

from paradl import auth, doctor, hooks, paths


class TestPlatformUnification(unittest.TestCase):
    def test_paths_windows_localappdata(self) -> None:
        """Verify get_paradl_home() uses LOCALAPPDATA on Windows when PARADL_HOME is unset."""
        fake_local_appdata = "/fake/Users/Alice/AppData/Local"
        with patch.dict(os.environ, {"LOCALAPPDATA": fake_local_appdata}, clear=False):
            if "PARADL_HOME" in os.environ:
                del os.environ["PARADL_HOME"]
            with patch("paradl.paths.is_windows", return_value=True):
                home = paths.get_paradl_home()
                self.assertEqual(home, Path(fake_local_appdata) / "paradl")

    def test_auth_status_windows_source_name(self) -> None:
        """Verify auth status reports 'credential_manager' on Windows when configured."""
        with (
            patch("paradl.auth.is_windows", return_value=True),
            patch("paradl.platform.has_keychain_token", side_effect=lambda srv: srv == "hf"),
        ):
            status = auth.get_auth_status()
            self.assertTrue(status["hf"]["configured"])
            self.assertEqual(status["hf"]["source"], "credential_manager")

    def test_hooks_notify_calls_platform_notification(self) -> None:
        """Verify hooks dispatch to platform display_notification."""
        with patch("paradl.hooks.display_notification", return_value=True) as mock_notify:
            hooks.notify_download_complete(
                "https://example.com/file.zip", Path("/tmp/file.zip"), 1024
            )
            self.assertTrue(mock_notify.called)

    def test_doctor_audits_windows_utilities_on_windows(self) -> None:
        """Verify doctor checks Windows tools on Windows and does not fail on macOS tools."""
        fake_win_utils = {
            "powershell": {
                "available": True,
                "path": "C:\\Windows\\System32\\powershell.exe",
                "fix": None,
                "required": True,
            },
            "curl": {
                "available": True,
                "path": "C:\\Windows\\System32\\curl.exe",
                "fix": None,
                "required": True,
            },
            "tar": {
                "available": True,
                "path": "C:\\Windows\\System32\\tar.exe",
                "fix": None,
                "required": False,
            },
            "winget": {
                "available": True,
                "path": "C:\\Users\\Alice\\AppData\\Local\\Microsoft\\WinGet\\Links\\winget.exe",
                "fix": None,
                "required": False,
            },
        }

        with (
            patch("paradl.doctor.is_windows", return_value=True),
            patch("paradl.doctor.check_platform_utilities", return_value=fake_win_utils),
        ):
            from paradl.ui import UI

            UI.json_mode = True
            try:
                ret = doctor.run_doctor()
                self.assertEqual(ret, 0)
            finally:
                UI.json_mode = False


if __name__ == "__main__":
    unittest.main()
