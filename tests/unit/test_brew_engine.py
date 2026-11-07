"""Unit tests for Homebrew bottle engine resolution, tag priority, and target parsing."""

from __future__ import annotations

from pathlib import Path
from unittest.mock import patch

from pdl.engines.brew import BrewEngine
from pdl.models import TargetType
from pdl.net import probe_target


def test_is_brew_detection() -> None:
    assert BrewEngine.is_brew("brew:wget")
    assert BrewEngine.is_brew("brew/wget")
    assert BrewEngine.is_brew("homebrew:jq")
    assert BrewEngine.is_brew("homebrew/ripgrep")
    assert BrewEngine.is_brew("https://formulae.brew.sh/formula/tree")
    assert BrewEngine.is_brew("https://formulae.brew.sh/api/formula/tree.json")
    assert not BrewEngine.is_brew("https://github.com/astral-sh/uv")
    assert not BrewEngine.is_brew("pip:requests")


def test_parse_brew_target() -> None:
    assert BrewEngine.parse_brew_target("brew:wget") == "wget"
    assert BrewEngine.parse_brew_target("homebrew:jq") == "jq"
    assert BrewEngine.parse_brew_target("https://formulae.brew.sh/formula/ffmpeg") == "ffmpeg"
    assert BrewEngine.parse_brew_target("https://formulae.brew.sh/api/formula/ripgrep.json") == "ripgrep"


def test_get_fallback_bottle_tags() -> None:
    tags = BrewEngine.get_fallback_bottle_tags()
    assert isinstance(tags, list)
    assert len(tags) > 0
    assert "all" in tags


def test_resolve_brew_mocked() -> None:
    fake_formula_json = {
        "name": "sample-tool",
        "desc": "A sample command line tool",
        "versions": {"stable": "2.0.0"},
        "bottle": {
            "stable": {
                "files": {
                    "arm64_golden_gate": {
                        "cellar": ":any",
                        "url": "https://ghcr.io/v2/homebrew/core/sample-tool/blobs/sha256:11112222",
                        "sha256": "11112222",
                    },
                    "all": {
                        "cellar": ":any",
                        "url": "https://ghcr.io/v2/homebrew/core/sample-tool/blobs/sha256:33334444",
                        "sha256": "33334444",
                    },
                }
            }
        },
    }

    with (
        patch("pdl.engines.brew.fetch_json_with_etag", return_value=(fake_formula_json, {}, 200)),
        patch(
            "pdl.engines.brew.BrewEngine.get_brew_cache_path", return_value=Path("/tmp/cache/sample-tool.bottle.tar.gz")
        ),
        patch(
            "pdl.engines.brew.BrewEngine.resolve_ghcr_direct_url",
            return_value=("https://pkg-containers.githubusercontent.com/sample-tool.tar.gz", {}, 45000),
        ),
    ):
        res = BrewEngine.resolve("brew:sample-tool")
        assert res is not None
        assert res["formula"] == "sample-tool"
        assert res["version"] == "2.0.0"
        assert res["sha256"] == "11112222"
        assert res["filesize"] == 45000
        assert res["url"] == "https://pkg-containers.githubusercontent.com/sample-tool.tar.gz"


def test_probe_target_brew_integration() -> None:
    fake_json = {
        "name": "mytool",
        "versions": {"stable": "1.5.0"},
        "bottle": {
            "stable": {
                "files": {
                    "all": {
                        "url": "https://ghcr.io/v2/homebrew/core/mytool/blobs/sha256:aabbcc",
                        "sha256": "aabbcc",
                    }
                }
            }
        },
    }
    with (
        patch("pdl.engines.brew.fetch_json_with_etag", return_value=(fake_json, {}, 200)),
        patch("pdl.engines.brew.BrewEngine.get_brew_cache_path", return_value=None),
        patch(
            "pdl.engines.brew.BrewEngine.resolve_ghcr_direct_url",
            return_value=("https://pkg-containers.githubusercontent.com/mytool.tar.gz", {}, 12345),
        ),
    ):
        info = probe_target("brew:mytool")
        assert info.target_type == TargetType.BREW
        assert info.clean_url == "https://pkg-containers.githubusercontent.com/mytool.tar.gz"
        assert info.expected_digest == "sha256:aabbcc"
        assert info.content_length == 12345
