"""Tests for PyPI transitive dependency prefetching and offline installation enforcement."""

from __future__ import annotations

import unittest
import zipfile
from pathlib import Path
from tempfile import TemporaryDirectory

from paradl.engines.pypi import PyPIEngine


class TestPyPITransitive(unittest.TestCase):
    def test_parse_pypi_target_complex_specifiers(self) -> None:
        """Verify parse_pypi_target handles complex PEP 508 specifiers."""
        pkg, ver = PyPIEngine.parse_pypi_target("pip:llvmlite<0.51,>=0.50.0dev0")
        self.assertEqual(pkg, "llvmlite")
        self.assertIsNone(ver)

        pkg, ver = PyPIEngine.parse_pypi_target("pip:mpmath>=1.1.0,<1.4")
        self.assertEqual(pkg, "mpmath")
        self.assertIsNone(ver)

        pkg, ver = PyPIEngine.parse_pypi_target("pip:requests==2.32.3")
        self.assertEqual(pkg, "requests")
        self.assertEqual(ver, "2.32.3")

    def test_extract_wheel_dependencies_filters_markers(self) -> None:
        """Verify extract_wheel_dependencies extracts active requirements and drops inactive/extras."""
        with TemporaryDirectory() as tmp_dir:
            wheel_path = Path(tmp_dir) / "test_pkg-1.0.0-py3-none-any.whl"
            metadata_content = (
                "Metadata-Version: 2.1\n"
                "Name: test-pkg\n"
                "Version: 1.0.0\n"
                "Requires-Dist: required-core>=1.0\n"
                "Requires-Dist: linux-only>=2.0; platform_system == 'NonExistentOS'\n"
                "Requires-Dist: optional-feature; extra == 'feature'\n"
                "Requires-Dist: active-dep\n"
            )

            # Build mock wheel zip
            with zipfile.ZipFile(wheel_path, "w") as zf:
                zf.writestr("test_pkg-1.0.0.dist-info/METADATA", metadata_content)

            deps = PyPIEngine.extract_wheel_dependencies(wheel_path)
            self.assertIn("required-core>=1.0", deps)
            self.assertIn("active-dep", deps)
            self.assertNotIn("linux-only>=2.0; platform_system == 'NonExistentOS'", deps)
            self.assertNotIn("optional-feature", deps)


if __name__ == "__main__":
    unittest.main()
