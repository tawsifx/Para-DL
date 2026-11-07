"""Integration tests: mirrors.json resolution, size validation, failure drop, and metalink repair."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.engines.aria2 import Aria2Engine
from pdl.mirrors import (
    MirrorManager,
    parse_metalink_bytes,
)
from pdl.models import TargetInfo, TargetType
from pdl.ui import UI

pytestmark = pytest.mark.integration


def _make_meta4_xml(
    filename: str,
    file_data: bytes,
    urls: list[str],
    piece_length: int = 65536,
) -> bytes:
    full_sha256 = hashlib.sha256(file_data).hexdigest()
    pieces_xml: list[str] = []
    for offset in range(0, len(file_data), piece_length):
        chunk = file_data[offset : offset + piece_length]
        piece_hash = hashlib.sha256(chunk).hexdigest()
        pieces_xml.append(f"      <hash>{piece_hash}</hash>")
    pieces_block = "\n".join(pieces_xml)
    urls_block = "\n".join(f'    <url priority="{idx + 1}">{u}</url>' for idx, u in enumerate(urls))
    xml_text = f"""<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="{filename}">
    <size>{len(file_data)}</size>
    <hash type="sha-256">{full_sha256}</hash>
    <pieces length="{piece_length}" type="sha-256">
{pieces_block}
    </pieces>
{urls_block}
  </file>
</metalink>
"""
    return xml_text.encode("utf-8")


def test_mirrors_json_pattern_matching_and_failure_drop(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify mirrors.json glob/prefix/regex rules and failure dropping after repeated errors."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))

    mirrors_cfg = {
        "mirrors": {
            "https://primary.example.com/releases/*": [
                "https://mirror1.example.com/releases/{filename}",
                "https://mirror2.example.com/releases/{filename}",
            ],
            "https://cdn.example.org/files/": [
                "https://backup.example.org/files/",
            ],
        }
    }
    (pdl_home / "mirrors.json").write_text(json.dumps(mirrors_cfg), encoding="utf-8")
    MirrorManager.reset_failures()

    urls = MirrorManager.match_configured_mirrors("https://primary.example.com/releases/app-1.0.tar.gz")
    assert urls == [
        "https://mirror1.example.com/releases/app-1.0.tar.gz",
        "https://mirror2.example.com/releases/app-1.0.tar.gz",
    ]

    prefix_urls = MirrorManager.match_configured_mirrors("https://cdn.example.org/files/sub/archive.bin")
    assert prefix_urls == ["https://backup.example.org/files/sub/archive.bin"]

    m1 = "https://mirror1.example.com/releases/app-1.0.tar.gz"
    assert not MirrorManager.record_failure(m1, max_failures=2)
    assert MirrorManager.record_failure(m1, max_failures=2)
    assert MirrorManager.is_dropped(m1, max_failures=2)


def test_simulator_mirror_size_mismatch_rejection(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Simulator test: reject a mirror reporting a mismatched Content-Length and complete via valid mirror."""
    MirrorManager.reset_failures()

    with (
        ThrottleTestServer(SimulatorProfile("none", file_size=256 * 1024)) as primary_srv,
        ThrottleTestServer(SimulatorProfile("none", file_size=512 * 1024)) as bad_size_srv,
        ThrottleTestServer(SimulatorProfile("none", file_size=256 * 1024)) as good_mirror_srv,
    ):
        events: list[dict[str, Any]] = []
        orig_emit = UI.emit_event

        def capture_emit(event_type: str, **payload: Any) -> None:
            events.append({"event": event_type, **payload})
            orig_emit(event_type, **payload)

        monkeypatch.setattr(UI, "emit_event", capture_emit)

        dest_dir = tmp_path / "out_mismatch"
        dest_dir.mkdir(parents=True, exist_ok=True)

        t_info = TargetInfo(
            raw=primary_srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=primary_srv.file_url,
            filename_hint="mismatch_test.bin",
            content_length=primary_srv.profile.file_size,
            accept_ranges=True,
            etag_sha256=primary_srv.sha256_hex,
            expected_digest=f"sha256:{primary_srv.sha256_hex}",
            mirrors=[bad_size_srv.file_url, good_mirror_srv.file_url],
        )

        res = Aria2Engine.download(
            t_info,
            dest_dir,
            connections=4,
            caffeinate=False,
            net_watch=False,
            mirrors=[bad_size_srv.file_url, good_mirror_srv.file_url],
        )

        assert res.success, f"Download failed: {res.error}"
        assert res.file_path is not None and res.file_path.exists()
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == primary_srv.sha256_hex
        assert res.checksum_verified is True

        rejected = [e for e in events if e.get("event") == "mirror_rejected" and e.get("reason") == "size_mismatch"]
        assert len(rejected) == 1
        assert rejected[0]["mirror_url"] == bad_size_srv.file_url


def test_simulator_metalink_companion_chunk_repair(tmp_path: Path) -> None:
    """Metalink companion repair: server corrupts 1st chunk once; aria2 repairs via chunk hashes."""
    with (
        ThrottleTestServer(SimulatorProfile("none", file_size=256 * 1024, corrupt_once=True)) as corrupt_srv,
        ThrottleTestServer(SimulatorProfile("none", file_size=256 * 1024)) as clean_mirror_srv,
    ):
        raw_payload = corrupt_srv.httpd.payload
        expected_sha = corrupt_srv.sha256_hex

        meta4_bytes = _make_meta4_xml(
            "metalink_repaired.bin",
            raw_payload,
            [corrupt_srv.file_url, clean_mirror_srv.file_url],
            piece_length=65536,
        )
        meta4_file = tmp_path / "metalink_repaired.bin.meta4"
        meta4_file.write_bytes(meta4_bytes)

        parsed_meta = parse_metalink_bytes(meta4_bytes)
        assert parsed_meta is not None
        assert parsed_meta.filename == "metalink_repaired.bin"
        assert parsed_meta.expected_length == len(raw_payload)

        dest_dir = tmp_path / "out_metalink"
        dest_dir.mkdir(parents=True, exist_ok=True)

        t_info = TargetInfo(
            raw=corrupt_srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=corrupt_srv.file_url,
            filename_hint="metalink_repaired.bin",
            content_length=len(raw_payload),
            accept_ranges=True,
            etag_sha256=expected_sha,
            expected_digest=f"sha256:{expected_sha}",
            metalink_path=meta4_file,
            mirrors=[clean_mirror_srv.file_url],
        )

        res = Aria2Engine.download(
            t_info,
            dest_dir,
            connections=4,
            caffeinate=False,
            net_watch=False,
            mirrors=[clean_mirror_srv.file_url],
        )

        assert res.success, f"Metalink repair download failed: {res.error}"
        assert res.file_path is not None and res.file_path.exists()
        assert hashlib.sha256(res.file_path.read_bytes()).hexdigest() == expected_sha
