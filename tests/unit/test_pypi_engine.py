"""Unit tests for PyPI / pip engine resolution, wheel scoring, and target parsing."""

from __future__ import annotations

from unittest.mock import patch

from pdl.engines.pypi import PyPIEngine
from pdl.models import TargetType
from pdl.net import probe_target


def test_is_pypi_detection() -> None:
    assert PyPIEngine.is_pypi("pip:requests")
    assert PyPIEngine.is_pypi("pip/requests")
    assert PyPIEngine.is_pypi("pypi:numpy")
    assert PyPIEngine.is_pypi("pypi/numpy==1.26.0")
    assert PyPIEngine.is_pypi("https://pypi.org/project/urllib3/")
    assert PyPIEngine.is_pypi("https://pypi.org/project/urllib3/2.2.0/")
    assert not PyPIEngine.is_pypi("https://github.com/astral-sh/uv")
    assert not PyPIEngine.is_pypi("brew:wget")


def test_parse_pypi_target() -> None:
    assert PyPIEngine.parse_pypi_target("pip:requests") == ("requests", None)
    assert PyPIEngine.parse_pypi_target("pip:requests==2.32.3") == ("requests", "2.32.3")
    assert PyPIEngine.parse_pypi_target("pypi:numpy@1.26.4") == ("numpy", "1.26.4")
    assert PyPIEngine.parse_pypi_target("https://pypi.org/project/flask/3.0.0/") == ("flask", "3.0.0")
    assert PyPIEngine.parse_pypi_target("https://pypi.org/project/click") == ("click", None)


def test_score_wheel() -> None:
    # Exact CPython + macOS ARM64 match should score highest
    sc_exact = PyPIEngine.score_wheel(
        "cffi-1.17.1-cp312-cp312-macosx_11_0_arm64.whl",
        "bdist_wheel",
        py_version=(3, 12),
        target_platform="darwin",
        target_machine="arm64",
    )
    assert sc_exact >= 100

    # Universal pure python wheel
    sc_pure = PyPIEngine.score_wheel(
        "requests-2.32.3-py3-none-any.whl",
        "bdist_wheel",
        py_version=(3, 12),
        target_platform="darwin",
        target_machine="arm64",
    )
    assert 60 <= sc_pure < sc_exact

    # Stable ABI (abi3)
    sc_abi3 = PyPIEngine.score_wheel(
        "cryptography-43.0.1-cp39-abi3-macosx_10_9_universal2.whl",
        "bdist_wheel",
        py_version=(3, 12),
        target_platform="darwin",
        target_machine="arm64",
    )
    assert sc_abi3 > sc_pure

    # Incompatible architecture
    sc_incompat_arch = PyPIEngine.score_wheel(
        "cffi-1.17.1-cp312-cp312-win_amd64.whl",
        "bdist_wheel",
        py_version=(3, 12),
        target_platform="darwin",
        target_machine="arm64",
    )
    assert sc_incompat_arch == -1

    # Source distribution
    sc_sdist = PyPIEngine.score_wheel(
        "requests-2.32.3.tar.gz",
        "sdist",
        py_version=(3, 12),
        target_platform="darwin",
        target_machine="arm64",
    )
    assert sc_sdist == 10


def test_resolve_pypi_mocked() -> None:
    fake_pypi_json = {
        "info": {"name": "sample-pkg", "version": "1.0.0", "summary": "Sample test package"},
        "urls": [
            {
                "filename": "sample_pkg-1.0.0.tar.gz",
                "packagetype": "sdist",
                "url": "https://files.pythonhosted.org/sample_pkg-1.0.0.tar.gz",
                "size": 12000,
                "digests": {"sha256": "aaaa1111"},
            },
            {
                "filename": "sample_pkg-1.0.0-py3-none-any.whl",
                "packagetype": "bdist_wheel",
                "url": "https://files.pythonhosted.org/sample_pkg-1.0.0-py3-none-any.whl",
                "size": 8500,
                "digests": {"sha256": "bbbb2222"},
            },
        ],
    }

    with patch("pdl.engines.pypi.fetch_json_with_etag", return_value=(fake_pypi_json, {}, 200)):
        res = PyPIEngine.resolve("pip:sample-pkg")
        assert res is not None
        assert res["package"] == "sample-pkg"
        assert res["version"] == "1.0.0"
        assert res["filename"] == "sample_pkg-1.0.0-py3-none-any.whl"
        assert res["sha256"] == "bbbb2222"
        assert res["filesize"] == 8500


def test_probe_target_pypi_integration() -> None:
    fake_json = {
        "info": {"name": "test-mod", "version": "0.5.0"},
        "urls": [
            {
                "filename": "test_mod-0.5.0-py3-none-any.whl",
                "packagetype": "bdist_wheel",
                "url": "https://files.pythonhosted.org/test_mod-0.5.0-py3-none-any.whl",
                "size": 5000,
                "digests": {"sha256": "cccc3333"},
            }
        ],
    }
    with patch("pdl.engines.pypi.fetch_json_with_etag", return_value=(fake_json, {}, 200)):
        info = probe_target("pip:test-mod")
        assert info.target_type == TargetType.PYPI
        assert info.clean_url == "https://files.pythonhosted.org/test_mod-0.5.0-py3-none-any.whl"
        assert info.expected_digest == "sha256:cccc3333"
        assert info.filename_hint == "test_mod-0.5.0-py3-none-any.whl"
        assert info.content_length == 5000
