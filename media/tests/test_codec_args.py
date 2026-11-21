"""Retargeted ``test_h3_media_codec_args`` from ``tests/unit/test_bug_fixes.py``.

The original inspected ``MediaEngine._convert_coreaudio_or_ffmpeg`` source for
the flac/wav codec argument literals. Here they live in the worker's
:data:`CODEC_ARGS_MAP`, which the conversion method consumes.
"""

from __future__ import annotations

import inspect

from paradl_media.media import CODEC_ARGS_MAP, MediaEngine


def test_h3_media_codec_args() -> None:
    """flac and wav ffmpeg codec argument lists remain byte-exact."""
    assert CODEC_ARGS_MAP["flac"] == ["-vn", "-c:a", "flac"]
    assert CODEC_ARGS_MAP["wav"] == ["-vn", "-c:a", "pcm_s16le"]
    # The conversion method consumes the exact map (same guarantee as the
    # original source-inspection oracle, without the brittle string match).
    src = inspect.getsource(MediaEngine._convert_coreaudio_or_ffmpeg)
    assert "CODEC_ARGS_MAP" in src


def test_codec_args_full_map() -> None:
    """All codec argument lists match the legacy worker exactly."""
    assert CODEC_ARGS_MAP == {
        "mp3": ["-vn", "-c:a", "libmp3lame", "-b:a", "256k"],
        "flac": ["-vn", "-c:a", "flac"],
        "opus": ["-vn", "-c:a", "libopus", "-b:a", "192k"],
        "wav": ["-vn", "-c:a", "pcm_s16le"],
        "aac": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
        "lossless": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
    }
