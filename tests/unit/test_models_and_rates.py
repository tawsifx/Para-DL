"""Unit tests: table-driven rate parsing, aria2 formatting, audio honesty, and domain models."""

from __future__ import annotations

from pathlib import Path

import pytest

from pdl.engines.media import classify_audio_production
from pdl.models import (
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    format_rate_for_aria2,
    format_size,
    format_speed,
    parse_rate,
    sanitize_filename,
)

pytestmark = pytest.mark.unit


@pytest.mark.parametrize(
    ("rate_str", "expected_bps"),
    [
        # Bytes per second
        ("1024", 1024.0),
        ("500K", 500.0 * 1024),
        ("500KB/s", 500.0 * 1024),
        ("4MBps", 4.0 * 1024 * 1024),
        ("4MB/s", 4.0 * 1024 * 1024),
        ("4MiB/s", 4.0 * 1024 * 1024),
        ("1.5GB/s", 1.5 * 1024**3),
        # Bits per second (divided by 8)
        ("8Mbps", 1.0 * 1024 * 1024),
        ("8Mbit/s", 1.0 * 1024 * 1024),
        ("8Mbits/s", 1.0 * 1024 * 1024),
        ("800Kbps", 100.0 * 1024),
        ("800b/s", 100.0),
        # Disabled / zero values
        ("0", 0.0),
        ("off", 0.0),
        ("none", 0.0),
        (None, 0.0),
    ],
)
def test_parse_rate_bytes_vs_bits(rate_str: str | None, expected_bps: float) -> None:
    """Verify distinct parsing for byte rates vs bit rates and off/none/0."""
    assert abs(parse_rate(rate_str) - expected_bps) < 1e-3


@pytest.mark.parametrize(
    "bad_rate",
    [
        "",
        "   ",
        "fast",
        "10parsecs",
        "-5MB/s",
        "MB/s",
        "10MB/h",
    ],
)
def test_parse_rate_rejects_unparseable_values(bad_rate: str) -> None:
    """Unparseable rate expressions must raise a descriptive ValueError."""
    with pytest.raises(ValueError):
        parse_rate(bad_rate)


@pytest.mark.parametrize(
    ("input_str", "expected_aria2"),
    [
        ("8Mbps", str(1024 * 1024)),
        ("4MB/s", str(4 * 1024 * 1024)),
        ("0", None),
        ("off", None),
        (None, None),
    ],
)
def test_format_rate_for_aria2(input_str: str | None, expected_aria2: str | None) -> None:
    """Verify conversion of human rate strings to aria2 byte count strings."""
    assert format_rate_for_aria2(input_str) == expected_aria2


@pytest.mark.parametrize(
    ("pref", "codec", "produced", "expected_keyword"),
    [
        ("lossless", "mp4a.40.2", "passthrough_aac", "passthrough AAC"),
        ("lossless", "opus", "alac", "ALAC-wrapped lossy"),
        ("lossless", "flac", "alac", "true lossless ALAC"),
    ],
)
def test_audio_naming_honesty(pref: str, codec: str, produced: str, expected_keyword: str) -> None:
    """Ensure honest reporting of audio codec wrappers and containers."""
    msg = classify_audio_production(pref, codec, produced)
    assert expected_keyword in msg


@pytest.mark.parametrize(
    ("raw_name", "expected_clean"),
    [
        ("normal_file.tar.gz", "normal_file.tar.gz"),
        ("path/traversal/name.bin", "path_traversal_name.bin"),
        ("windows\\style\\path.exe", "windows_style_path.exe"),
        ('bad:chars*in?"file<name>|.zip', "bad_chars_in__file_name__.zip"),
        ("   ", "download"),
        ("", "download"),
    ],
)
def test_sanitize_filename(raw_name: str, expected_clean: str) -> None:
    """Sanitize filename by stripping directory separators and dangerous characters."""
    assert sanitize_filename(raw_name) == expected_clean


def test_models_dataclass_invariants() -> None:
    """Verify model field defaults and construction invariants."""
    t = TargetInfo(
        raw="https://example.com/file.zip", target_type=TargetType.DIRECT, clean_url="https://example.com/file.zip"
    )
    assert t.content_length is None
    assert t.accept_ranges is None
    assert t.headers == {}
    assert t.cookie_file is None

    res = TransferResult(success=True, file_path=Path("/tmp/test.bin"))
    assert res.exit_code == int(ExitCode.OK)
    assert res.recycle_count == 0

    assert format_size(1024) == "1.0 KB"
    assert format_size(1048576) == "1.0 MB"
    assert format_speed(1048576) == "1.0 MB/s"


def test_ui_secret_redaction_and_filter(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify secret redaction for Bearer tokens, query tokens, and env tokens."""
    import logging

    from pdl.ui import SecretRedactingFilter, _redact_obj, redact_secrets

    assert redact_secrets("Authorization: Bearer secret_token_123") == "Authorization: Bearer ***REDACTED***"
    assert redact_secrets("Connecting with token=super_secret_val") == "Connecting with token=***REDACTED***"

    monkeypatch.setenv("GITHUB_TOKEN", "ghp_myfaketoken12345")
    assert redact_secrets("Using ghp_myfaketoken12345 for fetch") == "Using ***REDACTED*** for fetch"

    data = {"secret_key": "raw_pass", "nested": {"password": "123", "normal": "hello"}}
    redacted = _redact_obj(data)
    assert redacted["secret_key"] == "***REDACTED***"
    assert redacted["nested"]["password"] == "***REDACTED***"
    assert redacted["nested"]["normal"] == "hello"

    flt = SecretRedactingFilter()
    rec = logging.LogRecord("test", logging.INFO, "test.py", 10, "Bearer secret_abc_xyz", (), None)
    assert flt.filter(rec) is True
    assert rec.msg == "Bearer ***REDACTED***"
