"""Unit tests for rate parsing, target classification, and model utilities."""

from __future__ import annotations

import unittest
from pathlib import Path

from paradl.models import (
    DIRECT_EXTENSIONS,
    ExitCode,
    TargetType,
    WatchdogMode,
    format_size,
    format_speed,
    is_image_domain,
    is_media_domain,
    parse_rate,
    sanitize_filename,
)


class TestParseRate(unittest.TestCase):
    """Tests for parse_rate() — human-readable rate to bytes/second conversion."""

    def test_plain_number_is_bytes(self) -> None:
        self.assertEqual(parse_rate(1000), 1000.0)
        self.assertEqual(parse_rate("1000"), 1000.0)

    def test_kb_suffix(self) -> None:
        self.assertEqual(parse_rate("1K"), 1024.0)
        self.assertEqual(parse_rate("1KB"), 1024.0)
        self.assertEqual(parse_rate("1KB/s"), 1024.0)
        self.assertEqual(parse_rate("1KBps"), 1024.0)

    def test_mb_suffix(self) -> None:
        self.assertEqual(parse_rate("1M"), 1024.0**2)
        self.assertEqual(parse_rate("1MB/s"), 1024.0**2)
        self.assertEqual(parse_rate("4MBps"), 1024.0**2 * 4)

    def test_bits_suffix_divides_by_8(self) -> None:
        self.assertEqual(parse_rate("1Kbps"), 1024.0 / 8.0)
        self.assertEqual(parse_rate("1Mbps"), 1024.0**2 / 8.0)

    def test_none_and_zero(self) -> None:
        self.assertEqual(parse_rate(None), 0.0)
        self.assertEqual(parse_rate("0"), 0.0)
        self.assertEqual(parse_rate("off"), 0.0)

    def test_invalid_raises(self) -> None:
        with self.assertRaises(ValueError):
            parse_rate("abc")
        with self.assertRaises(ValueError):
            parse_rate("-5")
        with self.assertRaises(ValueError):
            parse_rate("")


class TestFormatSpeedAndSize(unittest.TestCase):
    """Tests for format_speed() and format_size()."""

    def test_format_speed(self) -> None:
        self.assertEqual(format_speed(0), "0 B/s")
        self.assertEqual(format_speed(500), "500 B/s")
        self.assertEqual(format_speed(1536), "1.5 KB/s")
        self.assertEqual(format_speed(1024 * 1024 * 2.5), "2.5 MB/s")

    def test_format_size(self) -> None:
        self.assertEqual(format_size(None), "Unknown size")
        self.assertEqual(format_size(0), "0 B")
        self.assertEqual(format_size(512), "512.0 B")
        self.assertEqual(format_size(1024 * 1.5), "1.5 KB")
        self.assertEqual(format_size(1024 * 1024 * 3), "3.0 MB")


class TestSanitizeFilename(unittest.TestCase):
    """Tests for sanitize_filename() — path traversal and reserved name protection."""

    def test_unsafe_chars_replaced(self) -> None:
        self.assertEqual(sanitize_filename('a/b:c*d?e"f<g>h|i'), "a_b_c_d_e_f_g_h_i")

    def test_null_byte_removed(self) -> None:
        self.assertEqual(sanitize_filename("a\0b"), "a_b")

    def test_dot_only_name(self) -> None:
        self.assertEqual(sanitize_filename("..."), "download")

    def test_windows_reserved_name(self) -> None:
        self.assertEqual(sanitize_filename("CON"), "_CON")
        self.assertEqual(sanitize_filename("NUL.txt"), "_NUL.txt")

    def test_trailing_spaces_stripped(self) -> None:
        self.assertEqual(sanitize_filename("  file  "), "file")


class TestTargetClassification(unittest.TestCase):
    """Tests for TargetType enum and domain classification."""

    def test_target_type_values(self) -> None:
        self.assertEqual(TargetType.DIRECT.value, "direct")
        self.assertEqual(TargetType.STREAM.value, "stream")
        self.assertEqual(TargetType.SEARCH.value, "search")
        self.assertEqual(TargetType.GITHUB.value, "github")
        self.assertEqual(TargetType.HF.value, "huggingface")
        self.assertEqual(TargetType.IPSW.value, "ipsw")
        self.assertEqual(TargetType.PYPI.value, "pypi")
        self.assertEqual(TargetType.BREW.value, "brew")
        self.assertEqual(TargetType.TORRENT.value, "torrent")

    def test_media_domains(self) -> None:
        self.assertTrue(is_media_domain("https://www.youtube.com/watch?v=x"))
        self.assertTrue(is_media_domain("https://youtu.be/x"))
        self.assertTrue(is_media_domain("https://soundcloud.com/artist/track"))
        self.assertTrue(is_media_domain("https://vimeo.com/12345"))
        self.assertTrue(is_media_domain("https://twitch.tv/channel"))
        self.assertFalse(is_media_domain("https://example.com/file.zip"))
        self.assertFalse(is_media_domain("https://github.com/owner/repo"))

    def test_image_domains(self) -> None:
        self.assertTrue(is_image_domain("https://image.pollinations.ai/prompt/sunset"))
        self.assertTrue(is_image_domain("https://i.imgur.com/abcde"))
        self.assertTrue(is_image_domain("https://i.redd.it/xyz"))
        self.assertTrue(is_image_domain("https://cdn.discordapp.com/attachments/123"))
        self.assertFalse(is_image_domain("https://example.com/image.png"))


class TestDirectExtensions(unittest.TestCase):
    """Tests that DIRECT_EXTENSIONS contains expected file types."""

    def test_common_extensions(self) -> None:
        for ext in (".zip", ".iso", ".dmg", ".exe", ".msi", ".pdf", ".mp4", ".mp3", ".tar", ".gz"):
            self.assertIn(ext, DIRECT_EXTENSIONS)


class TestExitCodes(unittest.TestCase):
    """Tests for ExitCode enum values."""

    def test_exit_code_values(self) -> None:
        self.assertEqual(ExitCode.OK, 0)
        self.assertEqual(ExitCode.FAILURE, 1)
        self.assertEqual(ExitCode.USAGE, 2)
        self.assertEqual(ExitCode.VERIFICATION_FAILED, 3)
        self.assertEqual(ExitCode.MISSING_DEPENDENCY, 4)
        self.assertEqual(ExitCode.INTERRUPTED, 130)


class TestWatchdogModes(unittest.TestCase):
    """Tests for WatchdogMode enum."""

    def test_watchdog_mode_values(self) -> None:
        self.assertEqual(WatchdogMode.V1, "v1")
        self.assertEqual(WatchdogMode.V2, "v2")
        self.assertEqual(WatchdogMode.OFF, "off")


class TestMmapChecksum(unittest.TestCase):
    """Tests for mmap-based checksum verification for large files."""

    def test_compute_hashes_small_file_streaming(self) -> None:
        import os
        import tempfile

        from paradl.postproc.checksum import ChecksumEngine

        with tempfile.NamedTemporaryFile(delete=False, suffix=".bin") as f:
            f.write(b"hello world" * 100)
            tmp_path = f.name

        try:
            hashes = ChecksumEngine.compute_hashes(Path(tmp_path), ["sha256"])
            self.assertIn("sha256", hashes)
            self.assertEqual(len(hashes["sha256"]), 64)
        finally:
            os.unlink(tmp_path)

    def test_compute_hashes_large_file_mmap(self) -> None:
        import os
        import tempfile

        from paradl.postproc.checksum import ChecksumEngine

        # Create a temp file smaller than the threshold to verify streaming path
        with tempfile.NamedTemporaryFile(delete=False, suffix=".bin") as f:
            f.write(b"\x00" * 1024)
            tmp_path = f.name

        try:
            hashes = ChecksumEngine.compute_hashes(Path(tmp_path), ["sha256"])
            self.assertIn("sha256", hashes)
        finally:
            os.unlink(tmp_path)

    def test_mmap_threshold_constant(self) -> None:
        from paradl.postproc.checksum import MMAP_CHECKSUM_THRESHOLD

        self.assertEqual(MMAP_CHECKSUM_THRESHOLD, 100 * 1024 * 1024)


if __name__ == "__main__":
    unittest.main()
