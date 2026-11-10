"""Unit tests verifying all fixes for H1-H4, M1-M9, L1-L5, and R1."""

from __future__ import annotations

import os
import tarfile
import time
from pathlib import Path
from unittest import mock

import pytest

from paradl.cli import build_parser
from paradl.config import PDLConfig
from paradl.curl_parser import parse_curl_command
from paradl.engines import aria2 as aria2_mod
from paradl.engines.aria2 import Aria2Engine
from paradl.engines.github import GitHubEngine
from paradl.engines.hf import HuggingFaceEngine
from paradl.engines.media import MediaEngine
from paradl.engines.pypi import PyPIEngine
from paradl.history import compute_hour_bucket
from paradl.mirrors import MirrorResolution, parse_metalink_bytes
from paradl.models import DownloadItem, WatchdogMode, format_size, sanitize_filename
from paradl.net import detect_isp, probe_http_url, probe_range_support, reset_isp_detector
from paradl.platform.windows import unquarantine_zone_identifier_text
from paradl.postproc.archive import ArchiveExtractor, ArchiveSecurityError
from paradl.ui import UI


# ---------------------------------------------------------------------------
# H1: cURL parser consumes option values and identifies real target URL
# ---------------------------------------------------------------------------
def test_h1_curl_parser_value_flags():
    cases = [
        (
            "curl -x http://proxy.local:8080 https://example.com/file.zip",
            "https://example.com/file.zip",
        ),
        ("curl --connect-timeout 10 https://example.com/file.zip", "https://example.com/file.zip"),
        ("curl --retry 3 https://example.com/file.zip", "https://example.com/file.zip"),
        ("curl --max-time 30 -L https://example.com/file.zip", "https://example.com/file.zip"),
        ("curl --limit-rate 5M https://example.com/file.zip", "https://example.com/file.zip"),
        ("curl -o target.zip https://example.com/file.zip", "https://example.com/file.zip"),
    ]
    for cmd, expected_url in cases:
        parsed = parse_curl_command(cmd)
        assert parsed.url == expected_url
    # Test proxy capture
    p = parse_curl_command("curl -x http://proxy.local:8080 https://example.com/file.zip")
    assert p.proxy == "http://proxy.local:8080"


# ---------------------------------------------------------------------------
# H2: PyPI explicit pinned version 404 does not fall back to latest
# ---------------------------------------------------------------------------
def test_h2_pypi_pinned_version_404_no_fallback(monkeypatch):
    from paradl.engines import pypi as pypi_mod

    def fake_fetch(url: str, *args, **kwargs):
        if "/nonexistent-pkg/9.9.9/json" in url:
            return None, {}, 404
        if "/nonexistent-pkg/json" in url:
            return (
                {
                    "info": {
                        "name": "nonexistent-pkg",
                        "version": "1.0.0",
                        "summary": "",
                        "requires_dist": [],
                    },
                    "urls": [
                        {
                            "filename": "nonexistent_pkg-1.0.0-py3-none-any.whl",
                            "packagetype": "bdist_wheel",
                            "url": "https://files.invalid/nonexistent_pkg-1.0.0-py3-none-any.whl",
                            "size": 1,
                        }
                    ],
                },
                {},
                200,
            )
        return None, {}, 404

    monkeypatch.setattr(pypi_mod, "fetch_json_with_etag", fake_fetch)
    resolved = PyPIEngine.resolve("pip:nonexistent-pkg==9.9.9", allow_network=True)
    assert resolved is None


# ---------------------------------------------------------------------------
# H3: MediaEngine codec args map for flac and wav
# ---------------------------------------------------------------------------
def test_h3_media_codec_args():
    # Verify codec argument definitions in MediaEngine
    import inspect

    src = inspect.getsource(MediaEngine._convert_coreaudio_or_ffmpeg)
    assert '"flac": ["-vn", "-c:a", "flac"]' in src
    assert '"wav": ["-vn", "-c:a", "pcm_s16le"]' in src


# ---------------------------------------------------------------------------
# H4: aria2 final file resolution does not pick unrelated pre-existing files
# ---------------------------------------------------------------------------
def test_h4_aria2_final_file_resolution(tmp_path):
    UI.quiet = True
    actual = tmp_path / "payload.bin"
    actual.write_bytes(b"P" * 1024)

    unrelated = tmp_path / "unrelated.bin"
    unrelated.write_bytes(b"X" * 1024)
    future = time.time() + 10 * 365 * 86400
    os.utime(unrelated, (future, future))

    class FakeDaemon:
        def __init__(self, **kwargs):
            self._proc = None

        def start(self):
            pass

        def add_uri(self, uris, options):
            return "gid1"

        def add_metalink(self, raw, options):
            return ["gid1"]

        def get_global_stat(self):
            return {"downloadSpeed": "0"}

        def tell_status(self, gid):
            return {
                "status": "complete",
                "completedLength": str(len(actual.read_bytes())),
                "totalLength": str(len(actual.read_bytes())),
                "files": [{"path": str(actual)}],
            }

        def get_servers(self, gid):
            return []

        def force_pause(self, gid):
            pass

        def unpause(self, gid):
            pass

        def change_option(self, gid, options):
            pass

        def stop(self, *, graceful: bool = True):
            pass

    item = DownloadItem(
        url="http://127.0.0.1:9/payload.bin",
        out_dir=tmp_path,
        filename="expected-name.bin",
    )
    resolution = MirrorResolution(
        active_uris=[item.url],
        expected_length=len(actual.read_bytes()),
        accept_ranges=True,
        filename=None,
    )
    with (
        mock.patch.object(aria2_mod, "Aria2RpcDaemon", FakeDaemon),
        mock.patch.object(aria2_mod.MirrorManager, "resolve_and_validate", return_value=resolution),
    ):
        res = Aria2Engine.download_batch(
            [item],
            tmp_path,
            connections=4,
            anti_throttle=None,
            accept_ranges_hint=True,
            poll_interval=0.01,
            min_cooldown_sec=0.1,
            watchdog_mode=WatchdogMode.OFF,
            caffeinate=False,
            net_watch=False,
            write_xattr=False,
        )

    assert res.success
    assert Path(res.file_path).name == "payload.bin"
    assert "unrelated.bin" not in [p.name for p in res.files]


# ---------------------------------------------------------------------------
# M1: CLI flags in cURL mode
# ---------------------------------------------------------------------------
def test_m1_cli_flags_parsing():
    parser = build_parser(PDLConfig())
    args = parser.parse_args(
        ["https://example.com/f.zip", "--watchdog", "off", "--no-caffeinate", "--no-net-watch"]
    )
    assert args.watchdog == "off"
    assert args.caffeinate is False
    assert args.net_watch is False


# ---------------------------------------------------------------------------
# M2: Standard Metalink DOCTYPE accepted; ENTITY rejected
# ---------------------------------------------------------------------------
def test_m2_metalink_doctype_handling():
    valid_xml = b"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE metalink SYSTEM "http://www.metalinker.org/DTDs/metalink.dtd">
<metalink xmlns="http://www.metalinker.org/">
  <files><file name="ubuntu.iso"><size>1234</size>
  <verification><hash type="sha-256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash></verification>
  <resources><url>https://example.com/ubuntu.iso</url></resources>
  </file></files>
</metalink>"""
    parsed = parse_metalink_bytes(valid_xml)
    assert parsed is not None
    assert parsed.filename == "ubuntu.iso"
    assert parsed.expected_length == 1234

    # ENTITY declaration must be rejected
    malicious_xml = b"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE metalink [ <!ENTITY xxe "evil"> ]>
<metalink xmlns="http://www.metalinker.org/">
  <files><file name="ubuntu.iso"><size>1234</size></file></files>
</metalink>"""
    assert parse_metalink_bytes(malicious_xml) is None


# ---------------------------------------------------------------------------
# M3: HF /tree/<revision>/<subpath> URL parsing
# ---------------------------------------------------------------------------
def test_m3_hf_tree_url_parsing():
    url = "https://huggingface.co/meta-llama/Llama-3.1-8B/tree/main/onnx"
    parsed = HuggingFaceEngine.parse_hf_target(url)
    assert parsed["repo_id"] == "meta-llama/Llama-3.1-8B"
    assert parsed["subpath"] == "onnx"
    assert parsed["revision"] == "main"
    assert parsed["repo_type"] == "model"


# ---------------------------------------------------------------------------
# M4: HF unmatched pattern fails rather than downloading all files
# ---------------------------------------------------------------------------
def test_m4_hf_unmatched_pattern_fails(tmp_path):
    siblings = [
        {"rfilename": "model.bin", "size": 1000},
        {"rfilename": "config.json", "size": 100},
    ]
    with mock.patch.object(HuggingFaceEngine, "fetch_hf_info", return_value={"siblings": siblings}):
        res = HuggingFaceEngine.download(
            {"repo_id": "test/model", "repo_type": "model", "revision": "main", "subpath": None},
            tmp_path,
            pattern="*.nonexistent",
        )
        assert res.success is False
        assert "No files matched" in (res.error or "")


# ---------------------------------------------------------------------------
# M5: sanitize_filename directory traversal guards
# ---------------------------------------------------------------------------
def test_m5_sanitize_filename_traversal():
    assert sanitize_filename("..") == "download"
    assert sanitize_filename(".") == "download"
    assert sanitize_filename("....") == "download"
    assert sanitize_filename("   ...   ") == "download"
    assert sanitize_filename("CON.txt") == "_CON.txt"
    assert sanitize_filename("valid_name.zip") == "valid_name.zip"


# ---------------------------------------------------------------------------
# M6: compute_hour_bucket contract
# ---------------------------------------------------------------------------
def test_m6_compute_hour_bucket():
    for h in range(24):
        assert compute_hour_bucket(h) == h // 4


# ---------------------------------------------------------------------------
# M7: Batch downloads checksum status
# ---------------------------------------------------------------------------
def test_m7_batch_checksum_aggregation(tmp_path):
    UI.quiet = True
    f1 = tmp_path / "f1.bin"
    f1.write_bytes(b"file1")
    f2 = tmp_path / "f2.bin"
    f2.write_bytes(b"file2")

    it1 = DownloadItem(
        url="http://x/f1.bin",
        out_dir=tmp_path,
        filename="f1.bin",
        expected_digest="sha256:4b4869...dummy",
    )
    it2 = DownloadItem(
        url="http://x/f2.bin", out_dir=tmp_path, filename="f2.bin"
    )  # No expected digest

    # When it1 is verified but it2 has no digest hint, the batch is not claimed fully verified
    with (
        mock.patch("paradl.engines.aria2.ChecksumEngine.verify", return_value=(True, "sha256", "abc")),
        mock.patch.object(
            aria2_mod.MirrorManager,
            "resolve_and_validate",
            return_value=MirrorResolution(
                active_uris=["http://x/f1.bin"],
                expected_length=5,
                accept_ranges=True,
                filename=None,
            ),
        ),
        mock.patch.object(aria2_mod, "Aria2RpcDaemon") as mock_d_cls,
    ):
        mock_d = mock_d_cls.return_value
        mock_d.add_uri.return_value = "gid1"
        mock_d.get_global_stat.return_value = {"downloadSpeed": "0"}
        mock_d.tell_status.return_value = {
            "status": "complete",
            "completedLength": "5",
            "totalLength": "5",
            "files": [],
        }
        mock_d.get_servers.return_value = []

        res = Aria2Engine.download_batch(
            [it1, it2],
            tmp_path,
            connections=2,
            watchdog_mode=WatchdogMode.OFF,
            caffeinate=False,
            net_watch=False,
            write_xattr=False,
        )
        assert res.success
        # Aggregate checksum_verified is False because not all items were verified
        assert res.checksum_verified is False


# ---------------------------------------------------------------------------
# M8: 7z archive traversal validation
# ---------------------------------------------------------------------------
def test_m8_archive_member_validation(tmp_path):
    dest = tmp_path / "dest"
    dest.mkdir()
    with pytest.raises(ArchiveSecurityError):
        ArchiveExtractor._validate_relative_member_path("../../evil.txt", dest)
    with pytest.raises(ArchiveSecurityError):
        ArchiveExtractor._validate_relative_member_path("/absolute/path", dest)


# ---------------------------------------------------------------------------
# L1: GitHub URL with query string
# ---------------------------------------------------------------------------
def test_l1_github_url_query_string():
    url = "https://github.com/astral-sh/uv?tab=readme-ov-file"
    parsed = GitHubEngine.parse_github_target(url)
    assert parsed["type"] == "repo"
    assert parsed["owner"] == "astral-sh"
    assert parsed["repo"] == "uv"

    url_rel = "https://github.com/astral-sh/uv/releases?after=0.1.0"
    parsed_rel = GitHubEngine.parse_github_target(url_rel)
    assert parsed_rel["type"] == "release"
    assert parsed_rel["owner"] == "astral-sh"
    assert parsed_rel["repo"] == "uv"


# ---------------------------------------------------------------------------
# L2: Windows ZoneId case-insensitive removal
# ---------------------------------------------------------------------------
def test_l2_zone_identifier_case_insensitivity():
    for key in ["ZoneId", "ZONEID", "zoneid", "ZoneID"]:
        text = f"[ZoneTransfer]\r\n{key}=3\r\nHostUrl=https://example.com/test\r\n"
        cleaned = unquarantine_zone_identifier_text(text)
        assert key.lower() not in cleaned.lower()
        assert "HostUrl=https://example.com/test" in cleaned


# ---------------------------------------------------------------------------
# L3: format_size(0) displays "0 B"
# ---------------------------------------------------------------------------
def test_l3_format_size():
    assert format_size(0) == "0 B"
    assert format_size(0.0) == "0 B"
    assert format_size(None) == "Unknown size"
    assert format_size(1024) == "1.0 KB"


# ---------------------------------------------------------------------------
# L4: ISP detection transient failure not cached for 5 minutes
# ---------------------------------------------------------------------------
def test_l4_isp_detection_no_unknown_caching():
    reset_isp_detector()
    with mock.patch("httpx.Client") as mock_c_cls:
        mock_c = mock_c_cls.return_value.__enter__.return_value
        mock_c.get.side_effect = ConnectionError("Transient network failure")
        isp = detect_isp(force_refresh=True)
        assert isp == "Unknown ISP"
        # Ensure _CACHED_ISP was not populated with "Unknown ISP"
        import paradl.net as net_mod

        assert net_mod._CACHED_ISP is None


# ---------------------------------------------------------------------------
# L5: tarfile extractall filter fallback
# ---------------------------------------------------------------------------
def test_l5_tarfile_extractall_fallback(tmp_path):
    tar_path = tmp_path / "test.tar"
    f = tmp_path / "hello.txt"
    f.write_text("hello world")
    with tarfile.open(tar_path, "w") as tf:
        tf.add(f, arcname="hello.txt")

    dest = tmp_path / "extracted"
    dest.mkdir()
    ok, _out_d, files = ArchiveExtractor.extract(tar_path, dest_dir=dest)
    assert ok
    assert len(files) == 1
    assert files[0].name == "hello.txt"


# ---------------------------------------------------------------------------
# R1: Probe requests request Accept-Encoding: identity
# ---------------------------------------------------------------------------
def test_r1_probe_requests_identity_encoding():
    with mock.patch("paradl.net.get_http_client") as mock_client_fn:
        mock_client = mock_client_fn.return_value
        stream_ctx = mock_client.stream.return_value
        mock_resp = stream_ctx.__enter__.return_value
        mock_resp.status_code = 200
        mock_resp.url = "https://example.com/test.zip"
        mock_resp.headers = {"content-length": "100"}

        probe_range_support("https://example.com/test.zip")
        _args, kwargs = mock_client.stream.call_args
        assert kwargs["headers"].get("Accept-Encoding") == "identity"

    with mock.patch("paradl.net.request_with_retry") as mock_req:
        mock_resp = mock.MagicMock()
        mock_resp.status_code = 200
        mock_resp.url = "https://example.com/test.zip"
        mock_resp.headers = {"content-length": "100"}
        mock_req.return_value = mock_resp

        probe_http_url("https://example.com/test.zip")
        _args, kwargs = mock_req.call_args
        assert kwargs["headers"].get("Accept-Encoding") == "identity"


# ---------------------------------------------------------------------------
# CLI Progress Bar Display
# ---------------------------------------------------------------------------
def test_progress_bar_display(capsys):
    from paradl.ui import UI, LiveTransferDisplay

    UI.quiet = False
    UI.json_mode = False

    with LiveTransferDisplay(title="download.bin", total_bytes=1000) as live:
        live.update(downloaded_bytes=500, speed_bps=1000.0, connections=4)

    # In non-terminal / captured output, the progress bar must be printed
    out = capsys.readouterr().out
    assert "download.bin" in out
    assert "█" in out or "░" in out
    assert "%" in out
