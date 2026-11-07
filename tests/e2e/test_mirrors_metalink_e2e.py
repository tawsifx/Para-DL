"""End-to-end tests for mirror size mismatch rejection, failover, and metalink chunk repair."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

import pytest
from tests.support.fakes import metalink_factory
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.engines.aria2 import Aria2Engine
from pdl.mirrors import MirrorManager, parse_metalink_bytes
from pdl.models import TargetInfo, TargetType
from pdl.ui import UI


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.mark.slow
def test_simulator_mirror_size_mismatch_rejection(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Reject a mirror reporting a different Content-Length and complete via valid mirror."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))
    MirrorManager.reset_failures()

    with (
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as primary_srv,
        ThrottleTestServer(profile=SimulatorProfile(file_size=512 * 1024, name="none")) as bad_size_srv,
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as good_mirror_srv,
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
        assert _sha256_file(res.file_path) == primary_srv.sha256_hex
        assert res.checksum_verified is True

        rejected = [e for e in events if e.get("event") == "mirror_rejected" and e.get("reason") == "size_mismatch"]
        assert len(rejected) == 1
        assert rejected[0]["mirror_url"] == bad_size_srv.file_url
        assert rejected[0]["expected_length"] == 256 * 1024
        assert rejected[0]["mirror_length"] == 512 * 1024


@pytest.mark.slow
def test_simulator_mirror_failover_and_adaptive_selection(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Primary fails during GET after probe; aria2/failover completes from mirror with valid SHA-256."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))
    MirrorManager.reset_failures()

    with (
        ThrottleTestServer(
            profile=SimulatorProfile(file_size=256 * 1024, name="none", fail_get_after_probe=True)
        ) as primary_srv,
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="always_503")) as dead_mirror_srv,
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as healthy_mirror_srv,
    ):
        mirrors_cfg = {
            "mirrors": {
                f"{primary_srv.base_url}/*": [
                    dead_mirror_srv.file_url,
                    healthy_mirror_srv.file_url,
                ]
            }
        }
        (pdl_home / "mirrors.json").write_text(json.dumps(mirrors_cfg), encoding="utf-8")

        dest_dir = tmp_path / "out_failover"
        dest_dir.mkdir(parents=True, exist_ok=True)

        t_info = TargetInfo(
            raw=primary_srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=primary_srv.file_url,
            filename_hint="failover_test.bin",
            content_length=primary_srv.profile.file_size,
            accept_ranges=True,
            etag_sha256=healthy_mirror_srv.sha256_hex,
            expected_digest=f"sha256:{healthy_mirror_srv.sha256_hex}",
        )

        res = Aria2Engine.download(
            t_info,
            dest_dir,
            connections=4,
            caffeinate=False,
            net_watch=False,
        )

        assert res.success, f"Mirror failover download failed: {res.error}"
        assert res.file_path is not None and res.file_path.exists()
        assert _sha256_file(res.file_path) == healthy_mirror_srv.sha256_hex
        assert res.checksum_verified is True
        assert MirrorManager.is_dropped(dead_mirror_srv.file_url)
        assert healthy_mirror_srv.request_count >= 1


@pytest.mark.slow
def test_simulator_metalink_companion_chunk_repair(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """.meta4 companion passed to aria2 with --check-integrity=true repairs corrupted chunk."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))
    MirrorManager.reset_failures()

    with (
        ThrottleTestServer(
            profile=SimulatorProfile(file_size=256 * 1024, name="none", corrupt_once=True)
        ) as corrupting_srv,
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as clean_mirror_srv,
    ):
        dest_dir = tmp_path / "out_meta4"
        dest_dir.mkdir(parents=True, exist_ok=True)

        meta4_bytes = metalink_factory(
            filename="repaired_payload.bin",
            file_data=corrupting_srv.payload,
            urls=[corrupting_srv.file_url, clean_mirror_srv.file_url],
            piece_length=65536,
        )
        meta4_path = dest_dir / "repaired_payload.bin.meta4"
        meta4_path.write_bytes(meta4_bytes)

        companion = parse_metalink_bytes(meta4_bytes)
        assert companion is not None
        assert companion.expected_digest == f"sha256:{corrupting_srv.sha256_hex}"
        assert len(companion.urls) == 2

        t_info = TargetInfo(
            raw=corrupting_srv.file_url,
            target_type=TargetType.DIRECT,
            clean_url=corrupting_srv.file_url,
            filename_hint="repaired_payload.bin",
            content_length=corrupting_srv.profile.file_size,
            accept_ranges=True,
        )

        res = Aria2Engine.download(
            t_info,
            dest_dir,
            connections=4,
            caffeinate=False,
            net_watch=False,
        )

        assert res.success, f"Metalink repair download failed: {res.error}"
        assert res.file_path is not None and res.file_path.exists()
        assert _sha256_file(res.file_path) == corrupting_srv.sha256_hex
        assert res.checksum_verified is True
