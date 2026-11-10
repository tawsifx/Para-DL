"""Unit tests for config management, validation, and atomic saves."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from unittest.mock import patch

from paradl.config import CONFIG_SCHEMA_VERSION, PDLConfig


class TestPDLConfigDefaults:
    """Tests for default configuration values."""

    def test_default_connections_is_16(self) -> None:
        cfg = PDLConfig()
        assert cfg.connections == 16

    def test_default_watchdog_is_v2(self) -> None:
        cfg = PDLConfig()
        assert cfg.watchdog == "v2"

    def test_default_anti_throttle_is_4mbps(self) -> None:
        cfg = PDLConfig()
        assert cfg.anti_throttle == "4MBps"

    def test_default_schema_version(self) -> None:
        cfg = PDLConfig()
        assert cfg.schema_version == CONFIG_SCHEMA_VERSION

    def test_default_bin_dir(self) -> None:
        cfg = PDLConfig()
        assert cfg.bin_dir  # non-empty string


class TestPDLConfigValidation:
    """Tests for PDLConfig.validate()."""

    def test_connections_clamped_to_16(self) -> None:
        cfg = PDLConfig(connections=32)
        cfg.validate()
        assert cfg.connections == 16

    def test_connections_zero_raises(self) -> None:
        cfg = PDLConfig(connections=0)
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "connections must be >=" in str(exc)

    def test_connections_bool_rejected(self) -> None:
        cfg = PDLConfig(connections=True)
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError:
            pass

    def test_invalid_audio_codec_raises(self) -> None:
        cfg = PDLConfig(audio_codec="vorbis")
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "audio_codec must be one of" in str(exc)

    def test_invalid_video_quality_raises(self) -> None:
        cfg = PDLConfig(video_quality="8k")
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "video_quality must be one of" in str(exc)

    def test_invalid_watchdog_raises(self) -> None:
        cfg = PDLConfig(watchdog="v3")
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "watchdog must be one of" in str(exc)

    def test_anti_throttle_none_disabled(self) -> None:
        cfg = PDLConfig(anti_throttle="none")
        cfg.validate()
        assert cfg.anti_throttle is None

    def test_anti_throttle_off_disabled(self) -> None:
        cfg = PDLConfig(anti_throttle="off")
        cfg.validate()
        assert cfg.anti_throttle is None

    def test_grace_period_minimum_3(self) -> None:
        cfg = PDLConfig(anti_throttle_grace=1)
        cfg.validate()
        assert cfg.anti_throttle_grace == 3

    def test_stall_timeout_negative_raises(self) -> None:
        cfg = PDLConfig(stall_timeout=-1.0)
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "cannot be negative" in str(exc)

    def test_isp_empty_raises(self) -> None:
        cfg = PDLConfig(isp="")
        try:
            cfg.validate()
            raise AssertionError("Expected ValueError")
        except ValueError as exc:
            assert "isp must be a non-empty string" in str(exc)

    def test_isp_auto_preserved(self) -> None:
        cfg = PDLConfig(isp="auto")
        cfg.validate()
        assert cfg.isp == "auto"

    def test_legacy_duronto_migrated_to_auto(self) -> None:
        cfg = PDLConfig(isp="Duronto Internet")
        cfg.validate()
        assert cfg.isp == "auto"


class TestPDLConfigSet:
    """Tests for PDLConfig.set() with type coercion."""

    def test_set_boolean_true(self) -> None:
        cfg = PDLConfig()
        ok, _msg = cfg.set("caffeinate", "true")
        assert ok is True
        assert cfg.caffeinate is True

    def test_set_boolean_false(self) -> None:
        cfg = PDLConfig()
        ok, _msg = cfg.set("caffeinate", "false")
        assert ok is True
        assert cfg.caffeinate is False

    def test_set_integer(self) -> None:
        cfg = PDLConfig()
        ok, _msg = cfg.set("connections", "8")
        assert ok is True
        assert cfg.connections == 8

    def test_set_float(self) -> None:
        cfg = PDLConfig()
        ok, _msg = cfg.set("stall_timeout", "60.5")
        assert ok is True
        assert cfg.stall_timeout == 60.5

    def test_set_unknown_key_returns_false(self) -> None:
        cfg = PDLConfig()
        ok, msg = cfg.set("nonexistent_key", "value")
        assert ok is False
        assert "Unknown configuration key" in msg

    def test_set_schema_version_blocked(self) -> None:
        cfg = PDLConfig()
        ok, _msg = cfg.set("schema_version", "99")
        assert ok is False


class TestPDLConfigAtomicSave:
    """Tests for PDLConfig.save() atomic write behavior."""

    def test_save_creates_config_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cfg_file = Path(tmp) / "config.json"
            with patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False):
                cfg = PDLConfig()
                cfg.save()
                assert cfg_file.exists()
                data = json.loads(cfg_file.read_text(encoding="utf-8"))
                assert data["schema_version"] == CONFIG_SCHEMA_VERSION
                assert data["connections"] == 16

    def test_save_is_atomic_no_temp_left_behind(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp) / "config.json"
            with patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False):
                cfg = PDLConfig()
                cfg.save()
                # No .tmp files should remain
                tmp_files = list(Path(tmp).glob("*.tmp"))
                assert len(tmp_files) == 0

    def test_load_returns_defaults_when_no_file(self) -> None:
        with (
            tempfile.TemporaryDirectory() as tmp,
            patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False),
        ):
            cfg = PDLConfig.load()
            assert cfg.connections == 16
            assert cfg.watchdog == "v2"

    def test_load_returns_defaults_when_corrupt(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cfg_file = Path(tmp) / "config.json"
            cfg_file.write_text("{invalid json", encoding="utf-8")
            with patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False):
                cfg = PDLConfig.load()
                # Falls back to defaults
                assert cfg.connections == 16
                # Corrupt file should be backed up
                assert (Path(tmp) / "config.json.bad").exists()

    def test_load_migrates_schema_version(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cfg_file = Path(tmp) / "config.json"
            # Write config with old schema version
            cfg_file.write_text(
                json.dumps({"schema_version": 1, "connections": 8, "watchdog": "v1"}),
                encoding="utf-8",
            )
            with patch.dict(os.environ, {"PARADL_HOME": tmp}, clear=False):
                cfg = PDLConfig.load()
                assert cfg.schema_version == CONFIG_SCHEMA_VERSION
                assert cfg.connections == 8
                assert cfg.watchdog == "v1"


class TestPDLConfigGetEffectiveISP:
    """Tests for PDLConfig.get_effective_isp()."""

    def test_explicit_isp_returned(self) -> None:
        cfg = PDLConfig(isp="Comcast Cable")
        assert cfg.get_effective_isp() == "Comcast Cable"

    def test_auto_isp_detects(self) -> None:
        cfg = PDLConfig(isp="auto")
        from paradl.net import reset_isp_detector, set_isp_detector

        set_isp_detector(lambda: "Mock ISP")
        try:
            assert cfg.get_effective_isp() == "Mock ISP"
        finally:
            reset_isp_detector()


if __name__ == "__main__":
    import unittest

    unittest.main()
