#!/usr/bin/env python3
"""Media-lane golden replay: drive the Python ``MediaEngine`` with a recorded
yt-dlp ``-J`` fixture and emit ``paradl-media/1`` protocol frames on stdout.

NO live network and NO real yt-dlp download occur: ``yt_dlp`` is replaced by an
in-process fake that returns the recorded ``mock_info`` and writes the recorded
artifact bytes. Human diagnostics go to stderr; stdout is protocol-only, exactly
as the real worker behaves.

Usage: python media_golden_replay.py <golden.json> <out_dir>

This file is a test fixture owned by rust/crates/paradl-parity; it never
modifies the read-only Python oracle or the media package.
"""

from __future__ import annotations

import copy
import json
import sys
import types
from pathlib import Path
from typing import Any


def _install_fake_ytdlp(mock_info: dict[str, Any], artifact: bytes) -> None:
    """Replace yt_dlp with a deterministic, network-free fake module."""

    class FakeYoutubeDL:  # noqa: D401 - test double
        def __init__(self, opts: dict[str, Any], logger: Any = None) -> None:
            self.opts = opts
            self.logger = logger

        def __enter__(self) -> "FakeYoutubeDL":
            return self

        def __exit__(self, *_args: Any) -> bool:
            return False

        def extract_info(self, _url: str, download: bool = False) -> dict[str, Any]:
            return copy.deepcopy(mock_info)

        def download(self, _urls: list[str]) -> None:
            template = str(self.opts.get("outtmpl") or "")
            if template:
                target = Path(template.replace("%(ext)s", "m4a"))
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(artifact)
            for tick in self.opts.get("_paradl_golden_ticks", []):
                for hook in self.opts.get("progress_hooks", []):
                    hook(tick)

    yt_dlp = types.ModuleType("yt_dlp")
    yt_dlp.YoutubeDL = FakeYoutubeDL  # type: ignore[attr-defined]
    version_module = types.ModuleType("yt_dlp.version")
    version_module.__version__ = "2026.10.26"
    yt_dlp.version = version_module  # type: ignore[attr-defined]
    extractor_module = types.ModuleType("yt_dlp.extractor")
    extractor_module.gen_extractor_classes = lambda: ()  # type: ignore[attr-defined]
    yt_dlp.extractor = extractor_module  # type: ignore[attr-defined]
    sys.modules["yt_dlp"] = yt_dlp
    sys.modules["yt_dlp.version"] = version_module
    sys.modules["yt_dlp.extractor"] = extractor_module


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        sys.stderr.write("usage: media_golden_replay.py <golden.json> <out_dir>\n")
        return 2
    golden_path = Path(argv[1])
    out_dir = Path(argv[2])
    golden = json.loads(golden_path.read_text(encoding="utf-8"))
    artifact = (golden_path.parent / golden["artifact"]["filename"]).read_bytes()
    out_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / golden["artifact"]["filename"]).write_bytes(artifact)

    from paradl_media.media import MediaEngine
    from paradl_media.protocol import progress_frame, result_frame

    _install_fake_ytdlp(golden["mock_info"], artifact)
    request_id = "golden-replay"
    for tick in golden["ticks"]:
        sys.stdout.write(
            json.dumps(
                progress_frame(
                    request_id,
                    downloaded_bytes=tick["downloaded_bytes"],
                    total_bytes=tick["total_bytes"],
                    speed_bps=tick["speed_bps"],
                    eta_seconds=tick["eta_seconds"],
                ),
                separators=(",", ":"),
            )
            + "\n"
        )
    sys.stdout.flush()

    engine = MediaEngine()
    resolved = engine.resolve(golden["target"], golden["options"])
    title, artist, album, year, _thumb = engine._parse_metadata_fields(golden["mock_info"])
    data = {
        "filename": resolved["filename"],
        "http_headers": resolved["http_headers"],
        "audio_format_produced": resolved["audio_format_produced"],
        "paths": resolved["paths"],
        "title": title,
        "artist": artist,
        "album": album,
        "year": year,
    }
    sys.stdout.write(
        json.dumps(
            result_frame(
                request_id,
                versions=golden["versions"],
                caps=golden["caps"],
                data=data,
            ),
            separators=(",", ":"),
        )
        + "\n"
    )
    sys.stdout.flush()
    sys.stderr.write(f"[golden-replay] artifact bytes={len(artifact)}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
