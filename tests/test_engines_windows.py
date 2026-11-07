"""Tests for engine Windows adaptations (pypi, github, aria2, toolchain)."""

from __future__ import annotations

import platform
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from pdl.engines.github import GitHubEngine
from pdl.engines.pypi import PyPIEngine
from pdl.toolchain import Toolchain


class TestEnginesWindows(unittest.TestCase):
    def test_pypi_wheel_scoring_windows(self) -> None:
        """Verify Windows wheel scoring for win_amd64, win_arm64, win32, and any."""
        # 64-bit Windows
        win_wheel = "numpy-2.1.0-cp312-cp312-win_amd64.whl"
        mac_wheel = "numpy-2.1.0-cp312-cp312-macosx_11_0_arm64.whl"
        any_wheel = "requests-2.32.3-py3-none-any.whl"

        score_win = PyPIEngine.score_wheel_filename(win_wheel, target_py="312", target_plat="win32", is_arm=False)
        score_mac = PyPIEngine.score_wheel_filename(mac_wheel, target_py="312", target_plat="win32", is_arm=False)
        score_any = PyPIEngine.score_wheel_filename(any_wheel, target_py="312", target_plat="win32", is_arm=False)

        self.assertGreater(score_win, 50, "win_amd64 wheel should score high on win32 target_plat")
        self.assertLessEqual(score_mac, 0, "macosx wheel should have score <= 0 on win32 target_plat")
        self.assertGreater(score_any, 0, "pure-python any wheel should be compatible")

    def test_github_asset_scoring_windows(self) -> None:
        """Verify GitHub release asset scoring on Windows."""
        win_zip = "ripgrep-14.1.1-x86_64-pc-windows-msvc.zip"
        win_msi = "ripgrep-14.1.1-x86_64.msi"
        mac_tar = "ripgrep-14.1.1-aarch64-apple-darwin.tar.gz"

        with patch("platform.system", return_value="Windows"), \
             patch("platform.machine", return_value="AMD64"):
            score_zip_dl = GitHubEngine.score_release_asset(win_zip, install_mode=False)
            score_msi_dl = GitHubEngine.score_release_asset(win_msi, install_mode=False)
            score_mac_dl = GitHubEngine.score_release_asset(mac_tar, install_mode=False)

            self.assertGreater(score_zip_dl, score_mac_dl)
            self.assertGreater(score_msi_dl, score_mac_dl)

            # In install_mode, portable .zip should beat .msi installer
            score_zip_inst = GitHubEngine.score_release_asset(win_zip, install_mode=True)
            score_msi_inst = GitHubEngine.score_release_asset(win_msi, install_mode=True)
            self.assertGreater(score_zip_inst, score_msi_inst)

    def test_toolchain_find_bin_windows_candidates(self) -> None:
        """Verify Toolchain checks .exe and candidate Windows directories."""
        tc = Toolchain(check_fallbacks=True)

        with patch("sys.platform", "win32"), \
             patch("shutil.which", return_value=None), \
             patch("os.path.exists", side_effect=lambda p: "scoop" in str(p) and str(p).endswith(".exe")), \
             patch("os.access", return_value=True):
            found = tc._find_bin("aria2c", [str(Path.home() / "scoop/shims/aria2c.exe")])
            self.assertIsNotNone(found)
            self.assertTrue(found.endswith(".exe"))


if __name__ == "__main__":
    unittest.main()
