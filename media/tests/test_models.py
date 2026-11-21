"""Retargeted model-helper tests: sanitize_filename, domains, and audio literals.

These are byte-exact copies of the relevant oracle cases from
``tests/unit/test_models.py`` and ``tests/unit/test_bug_fixes.py`` plus the 14
golden ``classify_audio_production`` cases from
``rust/crates/paradl-oracle/fixtures/media_protocol.json``.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from paradl_media.models import (
    IMAGE_DOMAINS,
    LOSSLESS_SOURCE_CODECS,
    MEDIA_DOMAINS,
    classify_audio_production,
    is_image_domain,
    is_media_domain,
    parse_rate,
    sanitize_filename,
)

FIXTURES = Path(__file__).parent / "fixtures"
GOLDEN = json.loads((FIXTURES / "media_protocol_golden.json").read_text())


class TestSanitizeFilename:
    """Byte-exact sanitize_filename behavior (200-char cap, Windows reserved)."""

    def test_unsafe_chars_replaced(self) -> None:
        assert sanitize_filename('a/b:c*d?e"f<g>h|i') == "a_b_c_d_e_f_g_h_i"

    def test_null_byte_replaced(self) -> None:
        assert sanitize_filename("a\0b") == "a_b"

    def test_dot_only_name(self) -> None:
        assert sanitize_filename("...") == "download"
        assert sanitize_filename("..") == "download"
        assert sanitize_filename("....") == "download"
        assert sanitize_filename("   ...   ") == "download"

    def test_windows_reserved_name(self) -> None:
        assert sanitize_filename("CON") == "_CON"
        assert sanitize_filename("NUL.txt") == "_NUL.txt"
        assert sanitize_filename("CON.txt") == "_CON.txt"

    def test_trailing_spaces_stripped(self) -> None:
        assert sanitize_filename("  file  ") == "file"
        assert sanitize_filename("valid_name.zip") == "valid_name.zip"

    def test_200_char_cap(self) -> None:
        result = sanitize_filename("a" * 250)
        assert len(result) == GOLDEN["byte_exact"]["sanitize_200_cap_len"] == 200


class TestDomains:
    """Retargeted from ``tests/unit/test_models.py``."""

    def test_media_domains(self) -> None:
        assert is_media_domain("https://www.youtube.com/watch?v=x")
        assert is_media_domain("https://youtu.be/x")
        assert is_media_domain("https://soundcloud.com/artist/track")
        assert is_media_domain("https://vimeo.com/12345")
        assert is_media_domain("https://twitch.tv/channel")
        assert not is_media_domain("https://example.com/file.zip")
        assert not is_media_domain("https://github.com/owner/repo")

    def test_image_domains(self) -> None:
        assert is_image_domain("https://image.pollinations.ai/prompt/sunset")
        assert is_image_domain("https://i.imgur.com/abcde")
        assert not is_image_domain("https://example.com/photo.png")

    def test_domain_tables_match_oracle(self) -> None:
        assert MEDIA_DOMAINS[0] == "youtube.com"
        assert "x.com" in MEDIA_DOMAINS
        assert IMAGE_DOMAINS[0] == "image.pollinations.ai"


class TestAudioFormatProduction:
    """All 14 golden audio-format strings (media.py:48-72)."""

    def test_lossless_codec_set(self) -> None:
        assert set(GOLDEN["audio_format_production"]["lossless_source_codecs"]) == LOSSLESS_SOURCE_CODECS

    @pytest.mark.parametrize("case", GOLDEN["audio_format_production"]["cases"], ids=lambda c: c["result"])
    def test_exact_literal(self, case: dict) -> None:
        got = classify_audio_production(
            case["requested_codec"], case["source_acodec"], case["conversion_mode"]
        )
        assert got == case["result"]


@pytest.mark.parametrize(
    ("raw", "expected_bps"),
    [
        ("4MBps", 4 * 1024 * 1024),
        ("4Mbps", 4 * 1024 * 1024 / 8),
        (None, 0.0),
        ("off", 0.0),
        (1048576, 1048576.0),
    ],
)
def test_parse_rate(raw: object, expected_bps: float) -> None:
    assert parse_rate(raw) == expected_bps
