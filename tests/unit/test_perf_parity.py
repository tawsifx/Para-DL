"""Parity guards for the perf/max-efficiency refactor.

These assert that optimizations preserve observable behaviour exactly. The
concurrency test in this file is a true RED-first regression test for the
daemon-pool race; the others are golden-output guards that must stay green
both before and after each optimization.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from paradl.core.models import sanitize_filename
from paradl.storage.mirrors import MirrorManager

# --- Task 1 guard: entry point must still resolve to the cli PACKAGE ---------


def test_cli_entry_point_resolves_to_package_not_stale_monolith():
    import paradl.cli as cli
    from paradl.cli.main import main
    from paradl.cli.parser import build_parser

    assert cli.main is main
    assert cli.build_parser is build_parser
    assert Path(cli.__file__).parent.name == "cli"
    assert Path(cli.__file__).name == "__init__.py"


# --- Task 2 guard: sanitize_filename exact-output oracle --------------------


def _reference_sanitize(name: str) -> str:
    """Independent, deliberately simple oracle for the intended semantics."""
    for ch in ("/", "\\", ":", "*", "?", '"', "<", ">", "|", "\0"):
        name = name.replace(ch, "_")
    cleaned = name.strip()[:200]
    stripped = cleaned.strip(" .")
    if not stripped or all(c == "." for c in stripped):
        return "download"
    stem = stripped.split(".", 1)[0].upper()
    reserved = {"CON", "PRN", "AUX", "NUL"}
    reserved |= {f"COM{i}" for i in range(1, 10)}
    reserved |= {f"LPT{i}" for i in range(1, 10)}
    if stem in reserved:
        return f"_{cleaned}"
    return cleaned or "download"


def test_sanitize_filename_matches_reference_on_tricky_corpus():
    corpus = [
        "plain.txt",
        "  spaced  name.zip  ",
        "../etc/passwd",
        "..\\windows\\system32",
        "a/b:c*d?e\"f<g>h|i\0j",
        "...",
        " . ",
        "",
        "CON",
        "con.txt",
        "COM1",
        "COM9.tar.gz",
        "LPT10",
        "aux",
        "NUL.",
        ".hidden",
        "file.name.with.dots",
        "x" * 250,
        "café-文件.bin",
    ]
    for raw in corpus:
        assert sanitize_filename(raw) == _reference_sanitize(raw), raw


# --- Task 3 guards: mirror matching output + cache invalidation -------------


def _write_mirrors(path: Path, mirror_for_host: str) -> None:
    cfg = {
        "mirrors": {
            "https://host.example.com/*": [
                "https://m1.example.com/",
                "https://m2.example.com/{filename}",
                "https://m1.example.com/",  # duplicate -> must be dropped
            ],
            "https://other.example.org/*": [f"https://{mirror_for_host}/"],
        }
    }
    path.write_text(json.dumps(cfg), encoding="utf-8")


def test_match_configured_mirrors_preserves_order_and_dedups(tmp_path: Path):
    path = tmp_path / "mirrors.json"
    _write_mirrors(path, "m3.example.org")
    url = "https://host.example.com/path/file.zip"
    assert MirrorManager.match_configured_mirrors(url, mirrors_file=path) == [
        "https://m1.example.com/path/file.zip",
        "https://m2.example.com/file.zip",
    ]


def test_mirror_config_cache_is_invalidated_when_file_changes(tmp_path: Path):
    path = tmp_path / "mirrors.json"
    _write_mirrors(path, "m3.example.org")
    url = "https://other.example.org/a/b.zip"
    assert MirrorManager.match_configured_mirrors(url, mirrors_file=path) == [
        "https://m3.example.org/a/b.zip"
    ]

    _write_mirrors(path, "newmirror.example.net")
    st = path.stat()
    os.utime(path, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000))
    assert MirrorManager.match_configured_mirrors(url, mirrors_file=path) == [
        "https://newmirror.example.net/a/b.zip"
    ]


# --- Task 4: RED-first daemon pool race regression --------------------------


def test_concurrent_acquires_get_distinct_daemons_and_release_reuses(monkeypatch):
    from unittest.mock import MagicMock

    from paradl.engines import aria2

    created: list[MagicMock] = []

    class FakeDaemon:
        def __init__(self, **kwargs):
            self._proc = MagicMock()
            self._proc.poll.return_value = None
            self.started = False
            created.append(self)

        def start(self):
            self.started = True

        def stop(self, graceful=True):
            self._proc.poll.return_value = 1

    monkeypatch.setattr(aria2, "Aria2RpcDaemon", FakeDaemon)
    pool = aria2.Aria2DaemonPool()

    kwargs = {
        "aria2c_bin": "aria2c",
        "max_concurrent_downloads": 16,
        "ip_family": "prefer-ipv4",
        "limit_rate": None,
        "check_integrity": False,
        "http3": False,
    }
    first = pool.acquire(**kwargs)
    second = pool.acquire(**kwargs)
    assert first is not second, "an in-use daemon must not be handed to a second caller"
    assert len(created) == 2

    pool.release(second)
    third = pool.acquire(**kwargs)
    assert third is second, "a released idle daemon must be reused"
    pool.release(first)
    pool.release(third)
    pool.shutdown_all()
