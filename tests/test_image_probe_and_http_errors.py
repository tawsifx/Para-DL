"""Tests for image domain classification, HTTP probe error handling, and long path safety."""

from __future__ import annotations

import argparse
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from pdl.app import PDLApp
from pdl.config import PDLConfig
from pdl.mirrors import find_or_fetch_metalink
from pdl.models import ExitCode, TargetType, is_image_domain
from pdl.net import HttpProbeResult, probe_target


class TestImageProbeAndHttpErrors(unittest.TestCase):
    def test_is_image_domain(self) -> None:
        """Verify image domain recognition for AI generation and image CDN services."""
        self.assertTrue(is_image_domain("https://image.pollinations.ai/prompt/sunset"))
        self.assertTrue(is_image_domain("https://pollinations.ai/prompt/cat"))
        self.assertTrue(is_image_domain("https://images.unsplash.com/photo-12345"))
        self.assertTrue(is_image_domain("https://i.imgur.com/abcde"))
        self.assertTrue(is_image_domain("https://i.redd.it/xyz123"))
        self.assertFalse(is_image_domain("https://youtube.com/watch?v=12345"))
        self.assertFalse(is_image_domain("https://github.com/tawsifx/pdl"))

    def test_image_domain_classified_as_direct(self) -> None:
        """Verify image services are classified as DIRECT instead of STREAM."""
        url = "https://image.pollinations.ai/prompt/vintage%20painting?width=800&height=450"
        mock_p = HttpProbeResult(
            ok=True,
            status_code=200,
            content_type="image/jpeg",
            filename="vintage-painting.jpg",
            final_url=url,
        )
        with patch("pdl.net.probe_http_url", return_value=mock_p):
            t = probe_target(url)
            self.assertEqual(t.target_type, TargetType.DIRECT)
            self.assertEqual(t.filename_hint, "vintage-painting.jpg")
            self.assertEqual(t.mime_type, "image/jpeg")

    def test_probe_402_surfaces_error_without_falling_to_stream(self) -> None:
        """Verify HTTP 402 returns direct error information and does not fall back to STREAM."""
        url = "https://image.pollinations.ai/prompt/expensive%20prompt"
        mock_p = HttpProbeResult(
            ok=False,
            status_code=402,
            final_url=url,
        )
        with patch("pdl.net.probe_http_url", return_value=mock_p):
            t = probe_target(url)
            self.assertEqual(t.target_type, TargetType.DIRECT)
            self.assertEqual(t.status_code, 402)
            self.assertIsNotNone(t.probe_error)
            self.assertIn("402", t.probe_error)
            self.assertIn("Payment Required", t.probe_error)

    def test_generic_url_probe_404_surfaces_error(self) -> None:
        """Verify generic URLs returning 404 do not fall back to streaming media engines."""
        url = "https://example.com/api/missing_resource"
        mock_p = HttpProbeResult(
            ok=False,
            status_code=404,
            final_url=url,
        )
        with patch("pdl.net.probe_http_url", return_value=mock_p):
            t = probe_target(url)
            self.assertEqual(t.status_code, 404)
            self.assertIsNotNone(t.probe_error)
            self.assertIn("404", t.probe_error)
            self.assertIn("Not Found", t.probe_error)

    def test_app_halts_early_on_probe_error(self) -> None:
        """Verify PDLApp cleanly fails without invoking yt-dlp on probe errors."""
        url = "https://image.pollinations.ai/prompt/test"
        mock_p = HttpProbeResult(
            ok=False,
            status_code=402,
            final_url=url,
        )
        with patch("pdl.net.probe_http_url", return_value=mock_p), \
             patch("pdl.engines.media.MediaEngine.execute") as mock_media:
            app = PDLApp(PDLConfig())
            args = argparse.Namespace(
                targets=[url],
                output_dir="/tmp",
                connections=16,
                audio=True,
            )
            exit_code = app.execute(args)
            self.assertEqual(exit_code, int(ExitCode.FAILURE))
            self.assertFalse(mock_media.called)

    def test_long_filename_metalink_safety(self) -> None:
        """Verify filenames over 255 characters do not trigger OSError: File name too long."""
        long_url = "https://example.com/" + ("very_long_path_segment_" * 20)
        # Should return None without raising OSError
        res = find_or_fetch_metalink(long_url, filename="long_" * 50, out_dir=Path("/tmp"))
        self.assertIsNone(res)


if __name__ == "__main__":
    unittest.main()
