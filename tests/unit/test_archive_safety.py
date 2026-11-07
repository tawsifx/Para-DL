"""Unit tests: table-driven malicious archive extraction security tests."""

from __future__ import annotations

import io
import stat
import tarfile
import zipfile
from pathlib import Path

import pytest
from tests.support.fakes import archive_factory

from pdl.postproc.archive import ArchiveExtractor, ArchiveSecurityError

pytestmark = pytest.mark.unit


@pytest.mark.parametrize(
    ("bad_member", "error_pattern"),
    [
        ("../../outside_pwned.txt", "traversal"),
        ("/tmp/pdl_abs_escape_test.txt", "Absolute path"),
        ("../parent_dir/escape.bin", "traversal"),
    ],
)
def test_malicious_tar_paths_blocked(tmp_path: Path, bad_member: str, error_pattern: str) -> None:
    """Tar archives containing path traversals or absolute paths must be rejected."""
    archive_path = tmp_path / "evil_tar.tar.gz"
    dest_dir = tmp_path / "out"

    with tarfile.open(archive_path, "w:gz") as tf:
        info = tarfile.TarInfo(name=bad_member)
        payload = b"malicious content"
        info.size = len(payload)
        tf.addfile(info, io.BytesIO(payload))

    with pytest.raises(ArchiveSecurityError, match=error_pattern):
        ArchiveExtractor.extract(archive_path, dest_dir)


def test_malicious_tar_escaping_symlink_blocked(tmp_path: Path) -> None:
    """Tar archives with symlinks pointing outside the destination directory must fail."""
    archive_path = tmp_path / "evil_symlink.tar.gz"
    dest_dir = tmp_path / "out"

    with tarfile.open(archive_path, "w:gz") as tf:
        sym = tarfile.TarInfo(name="sub/escape_link")
        sym.type = tarfile.SYMTYPE
        sym.linkname = "../../../etc/passwd"
        tf.addfile(sym)

    with pytest.raises(ArchiveSecurityError, match="escapes destination"):
        ArchiveExtractor.extract(archive_path, dest_dir)


@pytest.mark.parametrize(
    ("bad_member", "error_pattern"),
    [
        ("../escaped_zip.txt", "traversal"),
        ("../../root_file.txt", "traversal"),
    ],
)
def test_malicious_zip_path_traversal_blocked(tmp_path: Path, bad_member: str, error_pattern: str) -> None:
    """Zip archives containing directory traversal member paths must be rejected."""
    archive_path = tmp_path / "evil.zip"
    dest_dir = tmp_path / "out"

    with zipfile.ZipFile(archive_path, "w") as zf:
        zf.writestr(bad_member, "pwned")

    with pytest.raises(ArchiveSecurityError, match=error_pattern):
        ArchiveExtractor.extract(archive_path, dest_dir)


def test_malicious_zip_escaping_symlink_blocked(tmp_path: Path) -> None:
    """Zip archives containing symlinks escaping destination must be rejected."""
    archive_path = tmp_path / "evil_zip_sym.zip"
    dest_dir = tmp_path / "out"

    with zipfile.ZipFile(archive_path, "w") as zf:
        zinfo = zipfile.ZipInfo("bad_link")
        zinfo.create_system = 3
        zinfo.external_attr = (stat.S_IFLNK | 0o777) << 16
        zf.writestr(zinfo, "../../outside_secret")

    with pytest.raises(ArchiveSecurityError, match="escapes destination"):
        ArchiveExtractor.extract(archive_path, dest_dir)


def test_safe_tar_and_zip_extraction_succeeds(tmp_path: Path) -> None:
    """Legitimate archives with nested directories extract cleanly."""
    tar_path = tmp_path / "good.tar.gz"
    tar_dest = tmp_path / "tar_out"
    archive_factory(tar_path, {"nested/hello.txt": "safe tar payload"}, kind="tar.gz")

    ok, out, files = ArchiveExtractor.extract(tar_path, tar_dest)
    assert ok is True
    assert out == tar_dest.resolve()
    assert len(files) == 1
    assert (tar_dest / "nested" / "hello.txt").read_bytes() == b"safe tar payload"

    zip_path = tmp_path / "good.zip"
    zip_dest = tmp_path / "zip_out"
    archive_factory(zip_path, {"dir/readme.md": "# Safe Zip"}, kind="zip")

    ok_z, _out_z, files_z = ArchiveExtractor.extract(zip_path, zip_dest)
    assert ok_z is True
    assert len(files_z) == 1
    assert (zip_dest / "dir" / "readme.md").read_text(encoding="utf-8") == "# Safe Zip"


def test_archive_is_archive_and_missing_file(tmp_path: Path) -> None:
    """Verify is_archive format detection and missing archive extraction error handling."""
    assert ArchiveExtractor.is_archive(Path("file.zip")) is True
    assert ArchiveExtractor.is_archive(Path("file.tar.gz")) is True
    assert ArchiveExtractor.is_archive(Path("file.tgz")) is True
    assert ArchiveExtractor.is_archive(Path("file.bin")) is False
    assert ArchiveExtractor.is_archive(Path("file.iso")) is False

    ok, dest, files = ArchiveExtractor.extract(tmp_path / "nonexistent.zip", tmp_path / "out")
    assert ok is False
    assert dest is None
    assert files == []


def test_archive_extractor_special_fifo_and_empty_member(tmp_path: Path) -> None:
    """Verify special FIFO/device nodes and empty member names raise ArchiveSecurityError."""
    dest = tmp_path / "out"
    with pytest.raises(ArchiveSecurityError, match="Empty archive member"):
        ArchiveExtractor._validate_relative_member_path("", dest)

    tar_path = tmp_path / "fifo.tar"
    with tarfile.open(tar_path, "w") as tf:
        info = tarfile.TarInfo("my_fifo")
        info.type = tarfile.FIFOTYPE
        tf.addfile(info)

    with pytest.raises(ArchiveSecurityError, match="Special device/FIFO member forbidden"):
        ArchiveExtractor.extract(tar_path, dest)
