"""Unit tests: table-driven target classification, media domain recognition, RFC 6266, and cloud resolution."""

from __future__ import annotations

import pytest

from pdl.engines.cloud import CloudResolver
from pdl.models import TargetType, is_media_domain
from pdl.net import parse_content_disposition_filename, probe_target

pytestmark = pytest.mark.unit


@pytest.mark.parametrize(
    ("url", "expected"),
    [
        ("https://youtube.com/watch?v=dQw4w9WgXcQ", True),
        ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", True),
        ("https://music.youtube.com/watch?v=dQw4w9WgXcQ", True),
        ("https://youtu.be/dQw4w9WgXcQ", True),
        ("https://x.com/user/status/123", True),
        ("https://mobile.x.com/user/status/123", True),
        ("https://notyoutube.com/file.bin", False),
        ("https://fakeyoutu.be/video", False),
        ("https://linux.com/kernel.tar.xz", False),
        ("https://myx.com/archive.zip", False),
        ("https://vimeo.com.evil.example/video", False),
    ],
)
def test_media_domain_hostname_suffix_matching(url: str, expected: bool) -> None:
    """Verify hostname suffix matching correctly recognizes media domains."""
    assert is_media_domain(url) is expected


@pytest.mark.parametrize(
    ("raw_target", "expected_type", "expected_url_prefix"),
    [
        # IPSW shortcuts and direct firmware URLs
        ("ipsw:iPhone14,5", TargetType.IPSW, "https://api.ipsw.me/v4/ipsw/download/iPhone14,5"),
        ("ipsw:iPhone 16 Pro", TargetType.IPSW, "https://api.ipsw.me/v4/ipsw/download/iPhone17,1"),
        (
            "https://updates.cdn-apple.com/2024/ios/iPhone14,5_18.1_22B83_Restore.ipsw",
            TargetType.IPSW,
            "https://updates.cdn-apple.com/",
        ),
        # Magnet and torrent files
        ("magnet:?xt=urn:btih:1234567890abcdef", TargetType.TORRENT, "magnet:?xt="),
        ("https://example.com/ubuntu.iso.torrent", TargetType.TORRENT, "https://example.com/"),
        # Cloud sharing links (Google Drive, Dropbox, Pastebin)
        (
            "https://drive.google.com/file/d/1AbCdEfGhIjKlMnOpQrStUvWxYz/view?usp=sharing",
            TargetType.DIRECT,
            "https://drive.usercontent.google.com/download?id=1AbCdEfGhIjKlMnOpQrStUvWxYz",
        ),
        (
            "https://www.dropbox.com/s/abc123xyz/archive.zip?dl=0",
            TargetType.DIRECT,
            "https://www.dropbox.com/s/abc123xyz/archive.zip?dl=1",
        ),
        (
            "https://pastebin.com/AbCdEf12",
            TargetType.DIRECT,
            "https://pastebin.com/raw/AbCdEf12",
        ),
        # Hugging Face targets
        ("hf:TheBloke/Llama-2-7B-GGUF", TargetType.HF, "hf:TheBloke/Llama-2-7B-GGUF"),
        ("hf:datasets/fka/awesome-chatgpt-prompts", TargetType.HF, "hf:datasets/"),
        (
            "https://huggingface.co/TheBloke/Llama-2-7B-GGUF/resolve/main/model.Q4_K_M.gguf",
            TargetType.DIRECT,
            "https://huggingface.co/TheBloke/Llama-2-7B-GGUF/resolve/main/model.Q4_K_M.gguf",
        ),
        # GitHub targets
        ("gh:BurntSushi/ripgrep", TargetType.GITHUB, "https://github.com/BurntSushi/ripgrep"),
        ("github:astral-sh/uv", TargetType.GITHUB, "https://github.com/astral-sh/uv"),
        (
            "https://github.com/BurntSushi/ripgrep/releases/tag/14.1.0",
            TargetType.GITHUB,
            "https://github.com/BurntSushi/ripgrep/releases/tag/14.1.0",
        ),
        (
            "https://github.com/BurntSushi/ripgrep/releases/download/14.1.0/ripgrep-14.1.0-aarch64-apple-darwin.tar.gz",
            TargetType.DIRECT,
            "https://github.com/BurntSushi/ripgrep/releases/download/",
        ),
        (
            "https://github.com/torvalds/linux/blob/master/Makefile",
            TargetType.DIRECT,
            "https://raw.githubusercontent.com/torvalds/linux/master/Makefile",
        ),
        (
            "https://github.com/torvalds/linux/tree/master/kernel",
            TargetType.GITHUB,
            "https://github.com/torvalds/linux/tree/master/kernel",
        ),
        # Generic direct extensions vs media vs search
        ("https://releases.ubuntu.com/24.04/ubuntu.iso", TargetType.DIRECT, "https://releases.ubuntu.com/"),
        ("https://linux.com/downloads/kernel-6.11.tar.xz", TargetType.DIRECT, "https://linux.com/"),
        ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", TargetType.STREAM, "https://www.youtube.com/"),
        ("https://soundcloud.com/artist/track-name", TargetType.STREAM, "https://soundcloud.com/"),
        ("Daft Punk Veridis Quo", TargetType.SEARCH, "ytsearch1:Daft Punk Veridis Quo"),
    ],
)
def test_target_classification_grammar_table(
    raw_target: str,
    expected_type: TargetType,
    expected_url_prefix: str,
) -> None:
    """Table-driven test verifying grammar classification across all supported target formats."""
    info = probe_target(raw_target, allow_network_probe=False)
    assert info.target_type == expected_type
    assert info.clean_url.startswith(expected_url_prefix)


def test_hf_single_file_sends_authorization_header(monkeypatch: pytest.MonkeyPatch) -> None:
    """HF single-file downloads attach Authorization header from HF_TOKEN or HUGGING_FACE_HUB_TOKEN."""
    monkeypatch.setenv("HF_TOKEN", "hf_secret_unit_test_token_12345")
    url = "https://huggingface.co/TheBloke/Llama-2-7B-GGUF/resolve/main/llama-2-7b.Q4_K_M.gguf"
    info = probe_target(url, allow_network_probe=False)
    assert info.target_type == TargetType.DIRECT
    assert info.headers.get("Authorization") == "Bearer hf_secret_unit_test_token_12345"


@pytest.mark.parametrize(
    ("header_str", "expected_name"),
    [
        ("attachment; filename=\"fallback.bin\"; filename*=UTF-8''my%20file%20%E2%9C%93.tar.gz", "my file ✓.tar.gz"),
        ('inline; filename="simple.zip"', "simple.zip"),
        ("attachment; filename*=iso-8859-1''legacy%20doc.pdf", "legacy doc.pdf"),
    ],
)
def test_rfc6266_content_disposition_filename_star(header_str: str, expected_name: str) -> None:
    """Parse RFC 6266 / RFC 5987 encoded Content-Disposition header values."""
    assert parse_content_disposition_filename(header_str) == expected_name


def test_cloud_resolver_unchanged_for_normal_url() -> None:
    """Verify that regular non-cloud URLs pass through CloudResolver untouched."""
    url, hint = CloudResolver.resolve_url("https://example.com/data.parquet")
    assert url == "https://example.com/data.parquet"
    assert hint is None
