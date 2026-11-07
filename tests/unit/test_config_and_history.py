"""Unit tests: table-driven PDLConfig validation, atomic writes, and corrupt backup rotation."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from pdl.config import CONFIG_SCHEMA_VERSION, PDLConfig
from pdl.paths import get_config_file

pytestmark = pytest.mark.unit


def test_config_defaults_and_atomic_save(isolated_pdl_home: Path) -> None:
    """Verify default config values, version stamp, and persistence across loads."""
    cfg = PDLConfig.load()
    assert cfg.schema_version == CONFIG_SCHEMA_VERSION
    assert cfg.connections == 16
    assert cfg.watchdog == "v2"
    assert cfg.ytdlp_player_client is None

    ok, _ = cfg.set("connections", "12")
    assert ok is True
    assert cfg.connections == 12
    assert get_config_file().exists()

    reloaded = PDLConfig.load()
    assert reloaded.connections == 12
    assert reloaded.schema_version == CONFIG_SCHEMA_VERSION


@pytest.mark.parametrize(
    ("key", "val", "expect_ok", "error_substring"),
    [
        ("connections", "0", False, "connections"),
        ("connections", "-5", False, "connections"),
        ("audio_codec", "ogg_vorbis_invalid", False, "audio_codec"),
        ("anti_throttle", "invalid_rate_text", False, "Unparseable"),
        ("watchdog", "v99", False, "watchdog"),
        ("stall_timeout", "-10", False, "stall_timeout"),
    ],
)
def test_config_validation_table(
    isolated_pdl_home: Path,
    key: str,
    val: str,
    expect_ok: bool,
    error_substring: str,
) -> None:
    """Table-driven verification of configuration key and value validation constraints."""
    cfg = PDLConfig.load()
    ok, msg = cfg.set(key, val)
    assert ok is expect_ok
    if not expect_ok:
        assert error_substring in msg


def test_config_connections_clamping(isolated_pdl_home: Path) -> None:
    """Connections above 16 are clamped to 16."""
    cfg = PDLConfig.load()
    ok, _ = cfg.set("connections", "24")
    assert ok is True
    assert cfg.connections == 16


def test_config_grace_period_clamping(isolated_pdl_home: Path) -> None:
    """Grace period clamps minimum to 3s without artificial low ceiling."""
    cfg = PDLConfig.load()
    ok, _ = cfg.set("anti_throttle_grace", "15")
    assert ok is True
    assert cfg.anti_throttle_grace == 15

    ok, _ = cfg.set("anti_throttle_grace", "1")
    assert ok is True
    assert cfg.anti_throttle_grace == 3


def test_corrupt_config_creates_bad_backup(isolated_pdl_home: Path) -> None:
    """Corrupt config.json is backed up to config.json.bad and reset to default."""
    cfg_file = get_config_file()
    cfg_file.write_text("{corrupt json content!!!", encoding="utf-8")

    cfg = PDLConfig.load()
    assert cfg.connections == 16
    bad_backup = cfg_file.with_suffix(".json.bad")
    assert bad_backup.exists()
    assert bad_backup.read_text(encoding="utf-8") == "{corrupt json content!!!"


@pytest.mark.parametrize(
    ("field", "bad_val", "match_str"),
    [
        ("bin_dir", "", "non-empty"),
        ("history_limit", -5, "positive integer"),
        ("anti_throttle_grace", -1, "cannot be negative"),
        ("watchdog", "super_fast", "must be one of"),
        ("caffeinate", "not_a_bool", "must be a boolean"),
        ("stall_timeout", -10.0, "cannot be negative"),
        ("max_download_limit", 12345, "rate string or null"),
    ],
)
def test_config_field_validation_errors(field: str, bad_val: Any, match_str: str) -> None:
    """Config validation raises informative ValueError on invalid inputs."""
    with pytest.raises(ValueError, match=match_str):
        cfg = PDLConfig(**{field: bad_val})
        cfg.validate()


def test_config_field_normalization() -> None:
    """Config model normalizes 'off', 'none', 'default' string representations."""
    cfg = PDLConfig(
        max_download_limit="off",
        anti_throttle="none",
        ytdlp_player_client="default",
        anti_throttle_grace=1,
    )
    cfg.validate()
    assert cfg.max_download_limit is None
    assert cfg.anti_throttle is None
    assert cfg.ytdlp_player_client is None
    assert cfg.anti_throttle_grace == 3  # clamped to minimum 3s


def test_paths_default_resolution(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify paths resolution when PDL_HOME is explicitly unset."""
    import pdl.paths as paths

    monkeypatch.delenv("PDL_HOME", raising=False)
    home = paths.get_pdl_home()
    assert home == (Path.home() / ".config" / "pdl").resolve()
    assert paths.get_config_file() == home / "config.json"
    assert paths.get_history_db_file() == home / "history.db"
    assert paths.get_legacy_history_file() == home / "history.json"
    assert paths.get_mirrors_file() == home / "mirrors.json"
    assert paths.get_logs_dir() == home / "logs"
