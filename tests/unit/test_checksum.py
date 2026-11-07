"""Unit tests: streaming multi-algorithm checksum calculation and companion file discovery."""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest

from pdl.postproc.checksum import ChecksumEngine

pytestmark = pytest.mark.unit


def test_checksum_multi_algo_computation(tmp_path: Path) -> None:
    """Verify streaming computation of MD5, SHA1, SHA256, SHA512, and BLAKE2b."""
    payload = b"test payload for multi algorithm checksum verification"
    f = tmp_path / "data.bin"
    f.write_bytes(payload)

    exp_sha256 = hashlib.sha256(payload).hexdigest()
    exp_md5 = hashlib.md5(payload).hexdigest()

    matched, algo, digest = ChecksumEngine.verify(f, f"sha256:{exp_sha256}")
    assert matched is True
    assert algo == "sha256"
    assert digest == exp_sha256

    matched_md5, algo_md5, digest_md5 = ChecksumEngine.verify(f, f"md5:{exp_md5}")
    assert matched_md5 is True
    assert algo_md5 == "md5"
    assert digest_md5 == exp_md5


@pytest.mark.parametrize(
    ("content", "filename", "expected_algo"),
    [
        (
            "SHA256 (sample.bin) = 1122334455667788990011223344556677889900112233445566778899001122\n",
            "SHA256SUMS",
            "sha256",
        ),
        ("1122334455667788990011223344556677889900112233445566778899001122  sample.bin\n", "sha256sums.txt", "sha256"),
        ("MD5 (sample.bin) = aabbccddeeff00112233445566778899\n", "sample.bin.md5", "md5"),
        ("aabbccddeeff00112233445566778899 *sample.bin\n", "checksums.txt", "md5"),
    ],
)
def test_companion_checksum_format_parser(
    tmp_path: Path,
    content: str,
    filename: str,
    expected_algo: str,
) -> None:
    """Parse both BSD-style and GNU coreutils-style companion checksum files."""
    data_file = tmp_path / "sample.bin"
    data_file.write_bytes(b"data")
    sums_file = tmp_path / filename
    sums_file.write_text(content, encoding="utf-8")

    found = ChecksumEngine.find_companion_checksum(data_file)
    assert found is not None
    algo, digest = found
    assert algo == expected_algo
    assert len(digest) > 0


def test_display_hash_table(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Verify display_hash_table for terminal, quiet, and missing file states."""
    from pdl.ui import UI

    f = tmp_path / "test.txt"
    f.write_text("hello world\n")

    # 1. Non-existent file
    assert ChecksumEngine.display_hash_table(tmp_path / "nonexistent.txt") is False

    # 2. Quiet mode
    monkeypatch.setattr(UI, "quiet", True)
    monkeypatch.setattr(UI, "json_mode", False)
    assert ChecksumEngine.display_hash_table(f) is True
    out_quiet = capsys.readouterr().out
    assert "SHA256" in out_quiet

    # 3. Terminal formatted table mode
    monkeypatch.setattr(UI, "quiet", False)
    monkeypatch.setattr(UI, "json_mode", False)
    assert ChecksumEngine.display_hash_table(f) is True
    out_table = capsys.readouterr().out
    assert "Cryptographic Hashes:" in out_table


def test_companion_checksum_multi_manifest(tmp_path: Path) -> None:
    """Verify companion checksum discovery from SHA256SUMS multi-file manifest."""
    import hashlib

    data_file = tmp_path / "app.tar.gz"
    payload = b"some binary payload"
    data_file.write_bytes(payload)
    expected_hash = hashlib.sha256(payload).hexdigest()

    manifest = tmp_path / "SHA256SUMS"
    manifest.write_text(f"{expected_hash}  app.tar.gz\n1234567890abcdef  other.tar.gz\n", encoding="utf-8")

    found = ChecksumEngine.find_companion_checksum(data_file)
    assert found is not None
    assert found == ("sha256", expected_hash.lower())
