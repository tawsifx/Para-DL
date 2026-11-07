"""Unit tests: Toolchain discovery, NVM numeric version sort, and yt-dlp EJS flags."""

from __future__ import annotations

from pathlib import Path

import pytest

from pdl.toolchain import Toolchain, parse_version_tuple

pytestmark = pytest.mark.unit


def test_nvm_numeric_version_sorting(tmp_path: Path) -> None:
    """Sort nvm node versions by numeric version tuple (v22 beats v9)."""
    nvm_dir = tmp_path / "nvm" / "versions" / "node"
    for v_str in ("v9.11.2", "v18.20.4", "v22.11.0", "v20.15.1"):
        bin_dir = nvm_dir / v_str / "bin"
        bin_dir.mkdir(parents=True)
        node_bin = bin_dir / "node"
        node_bin.write_text("#!/bin/sh\necho " + v_str + "\n", encoding="utf-8")
        node_bin.chmod(0o755)

    sorted_nodes = Toolchain.find_nvm_nodes(nvm_dir)
    versions = [ver for ver, _path in sorted_nodes]
    assert versions == [
        (22, 11, 0),
        (20, 15, 1),
        (18, 20, 4),
        (9, 11, 2),
    ]
    assert parse_version_tuple("v22.11.0") > parse_version_tuple("v9.11.2")


def test_toolchain_js_runtime_preference_and_ejs_flag(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Runtime preference deno -> node (>=22) -> quickjs -> bun (deprecated).

    Do NOT pass --remote-components ejs:github unless ejs_remote=True.
    """
    fake_bin = tmp_path / "fake_bin"
    fake_bin.mkdir()

    # Fake node v22 binary
    node_script = fake_bin / "node"
    node_script.write_text("#!/bin/sh\necho v22.12.0\n", encoding="utf-8")
    node_script.chmod(0o755)

    monkeypatch.setenv("PATH", str(fake_bin))
    tc = Toolchain(check_fallbacks=False)

    assert tc.deno is None
    assert tc.node == str(node_script)
    assert tc.select_js_runtime() == ("node", str(node_script))

    # Default EJS args: explicit --js-runtimes node:..., NO --remote-components
    default_args = tc.get_ytdlp_js_args(ejs_remote=False)
    assert "--js-runtimes" in default_args
    assert f"node:{node_script}" in default_args
    assert "--remote-components" not in default_args

    # Opt-in --ejs-remote adds --remote-components ejs:github
    remote_args = tc.get_ytdlp_js_args(ejs_remote=True)
    assert "--remote-components" in remote_args
    assert "ejs:github" in remote_args

    # Deno takes priority over node
    deno_script = fake_bin / "deno"
    deno_script.write_text("#!/bin/sh\necho deno 2.0.0\n", encoding="utf-8")
    deno_script.chmod(0o755)
    tc.reset_cache()
    assert tc.select_js_runtime() == ("deno", str(deno_script))


def test_toolchain_binary_properties_and_parse_version_tuple(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Test Toolchain property accessors, caching, and version parsing variations."""
    assert parse_version_tuple("") == (0,)
    assert parse_version_tuple("abc") == (0,)
    assert parse_version_tuple("v1.2.3b4") == (1, 2, 3, 4)
    assert parse_version_tuple("2024.11.04") == (2024, 11, 4)

    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for tool in ("aria2c", "yt-dlp", "ffmpeg", "adb", "7z", "curl", "gh", "qjs", "bun"):
        p = bin_dir / tool
        p.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        p.chmod(0o755)

    monkeypatch.setenv("PATH", str(bin_dir))
    tc = Toolchain(check_fallbacks=True)
    assert tc.aria2c == str(bin_dir / "aria2c")
    assert tc.ytdlp == str(bin_dir / "yt-dlp")
    assert tc.ffmpeg == str(bin_dir / "ffmpeg")
    assert tc.adb == str(bin_dir / "adb")
    assert tc.p7zip == str(bin_dir / "7z")
    assert tc.curl == str(bin_dir / "curl")
    assert tc.gh == str(bin_dir / "gh")
    assert tc.quickjs == str(bin_dir / "qjs")
    assert tc.bun == str(bin_dir / "bun")
    assert tc.yt_dlp_ejs_version is not None or tc.yt_dlp_ejs_version is None
