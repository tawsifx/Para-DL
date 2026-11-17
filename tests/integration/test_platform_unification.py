"""Tests for core modules platform unification (paths, config, events, auth, hooks)."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from paradl import auth, paths


class TestPlatformUnification(unittest.TestCase):
    def test_paths_windows_localappdata(self) -> None:
        """Verify get_paradl_home() uses LOCALAPPDATA on Windows when PARADL_HOME is unset."""
        with tempfile.TemporaryDirectory() as tmp:
            fake_local_appdata = str(Path(tmp).resolve())
            with patch.dict(os.environ, {"LOCALAPPDATA": fake_local_appdata}, clear=False):
                if "PARADL_HOME" in os.environ:
                    del os.environ["PARADL_HOME"]
                with patch("paradl.core.paths.is_windows", return_value=True):
                    home = paths.get_paradl_home()
                    self.assertEqual(home, Path(fake_local_appdata) / "paradl")

    def test_auth_status_windows_source_name(self) -> None:
        """Verify auth status reports 'credential_manager' on Windows when configured."""
        with (
            patch.dict(
                os.environ,
                {
                    "HF_TOKEN": "",
                    "HUGGING_FACE_HUB_TOKEN": "",
                    "GITHUB_TOKEN": "",
                    "GH_TOKEN": "",
                },
            ),
            patch("paradl.auth.manager.is_windows", return_value=True),
            patch(
                "paradl.auth.manager.has_keychain_token",
                side_effect=lambda srv: srv == "hf",
            ),
        ):
            status = auth.get_auth_status()
            self.assertTrue(status["hf"]["configured"])
            self.assertEqual(status["hf"]["source"], "credential_manager")

    def test_hooks_notify_calls_platform_notification(self) -> None:
        """Verify hooks dispatch to platform display_notification."""
        from paradl.resilience import hooks as resilience_hooks

        with patch(
            "paradl.resilience.hooks.display_notification", return_value=True
        ) as mock_notify:
            resilience_hooks.notify_download_complete(
                "https://example.com/file.zip", Path("/tmp/file.zip"), 1024
            )
            self.assertTrue(mock_notify.called)


if __name__ == "__main__":
    unittest.main()
