"""JS/EJS selection and version probes moved from ``toolchain.py``."""

from __future__ import annotations

import importlib.metadata

from paradl_media import toolchain as tc


def test_parse_version_tuple() -> None:
    assert tc.parse_version_tuple("v22.11.0") == (22, 11, 0)
    assert tc.parse_version_tuple("9.11.2") == (9, 11, 2)
    assert tc.parse_version_tuple("garbage") == (0,)
    assert tc.parse_version_tuple("v2") == (2,)


def test_select_js_runtime_preference(monkeypatch) -> None:
    tool = tc.Toolchain(check_fallbacks=False)
    # Isolate from PATH-installed runtimes on the host.
    tool._cache.update({"deno": None, "node": None, "quickjs": None, "bun": None})
    assert tool.select_js_runtime() is None

    monkeypatch.setitem(tool._cache, "bun", "/usr/bin/bun")
    assert tool.select_js_runtime() == ("bun", "/usr/bin/bun")

    monkeypatch.setitem(tool._cache, "quickjs", "/usr/bin/qjs")
    assert tool.select_js_runtime() == ("quickjs", "/usr/bin/qjs")

    monkeypatch.setitem(tool._cache, "node", "/usr/bin/node")
    assert tool.select_js_runtime() == ("node", "/usr/bin/node")

    monkeypatch.setitem(tool._cache, "deno", "/usr/bin/deno")
    assert tool.select_js_runtime() == ("deno", "/usr/bin/deno")


def test_js_runtime_human_label(monkeypatch) -> None:
    tool = tc.Toolchain(check_fallbacks=False)
    tool._cache.update({"deno": None, "node": None, "quickjs": None, "bun": None})
    monkeypatch.setitem(tool._cache, "deno", "/usr/bin/deno")
    assert tool.js_runtime == "deno (/usr/bin/deno)"
    tool._cache["deno"] = None
    monkeypatch.setitem(tool._cache, "bun", "/usr/bin/bun")
    assert tool.js_runtime == "bun [deprecated] (/usr/bin/bun)"


def test_ytdlp_ejs_version_probe(monkeypatch) -> None:
    """The ``importlib.metadata.version('yt-dlp-ejs')`` probe is moved verbatim."""
    calls: list[str] = []

    def fake_version(name: str) -> str:
        calls.append(name)
        if name == "yt-dlp-ejs":
            return "0.8.0"
        raise importlib.metadata.PackageNotFoundError(name)

    monkeypatch.setattr(tc.importlib.metadata, "version", fake_version)
    tool = tc.Toolchain(check_fallbacks=False)
    assert tool.yt_dlp_ejs_version == "0.8.0"
    assert calls[:1] == ["yt-dlp-ejs"]
    # Cached on subsequent access.
    assert tool.yt_dlp_ejs_version == "0.8.0"
    assert calls == ["yt-dlp-ejs"]


def test_ytdlp_js_opts_and_args(monkeypatch) -> None:
    tool = tc.Toolchain(check_fallbacks=False)
    monkeypatch.setitem(tool._cache, "deno", "/usr/bin/deno")

    assert tool.get_ytdlp_js_args() == ["--js-runtimes", "deno:/usr/bin/deno"]
    assert tool.get_ytdlp_js_args(ejs_remote=True) == [
        "--js-runtimes",
        "deno:/usr/bin/deno",
        "--remote-components",
        "ejs:github",
    ]
    assert tool.get_ytdlp_js_opts() == {"js_runtimes": {"deno": {"path": "/usr/bin/deno"}}}
    assert tool.get_ytdlp_js_opts(ejs_remote=True) == {
        "js_runtimes": {"deno": {"path": "/usr/bin/deno"}},
        "remote_components": ["ejs:github"],
    }


def test_binary_version_probe(monkeypatch) -> None:
    assert tc.binary_version(None) == "unknown"

    class Proc:
        stdout = b"ffmpeg version 7.0.2 Copyright (c) 2000-2024 the FFmpeg developers\n"

    monkeypatch.setattr(tc.subprocess, "run", lambda *a, **k: Proc())
    assert tc.binary_version("/usr/bin/ffmpeg") == "7.0.2"


def test_binary_version_missing_is_weak(monkeypatch) -> None:
    def raise_os(*a, **k):
        raise OSError("not found")

    monkeypatch.setattr(tc.subprocess, "run", raise_os)
    assert tc.binary_version("/no/such/ffmpeg") == "unknown"
    assert tc.is_macos() is (tc.sys.platform == "darwin")
