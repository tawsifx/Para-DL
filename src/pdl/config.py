"""Configuration management with schema versioning, validation, and atomic writes."""

from __future__ import annotations

import json
import logging
import os
import shutil
import tempfile
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from pdl.models import WatchdogMode, parse_rate
from pdl.paths import get_config_file, get_pdl_home

logger = logging.getLogger(__name__)

CONFIG_SCHEMA_VERSION = 2

VALID_AUDIO_CODECS = {"lossless", "native", "mp3", "flac", "opus", "wav", "aac"}
VALID_VIDEO_QUALITIES = {"best", "4k", "1440p", "1080p", "720p", "480p"}
VALID_WATCHDOG_MODES = {m.value for m in WatchdogMode}


@dataclass
class PDLConfig:
    schema_version: int = CONFIG_SCHEMA_VERSION
    connections: int = 16
    output_dir: str = "."
    audio_codec: str = "aac"
    video_quality: str = "best"
    auto_extract: bool = False
    auto_adb_push: bool = False
    bin_dir: str = str(Path.home() / ".local/bin")
    adb_music_dir: str = "/storage/emulated/0/Music"
    adb_download_dir: str = "/storage/emulated/0/Download"
    max_download_limit: str | None = None
    history_limit: int = 50
    anti_throttle: str | None = "4MBps"
    anti_throttle_grace: int = 5
    watchdog: str = "v2"
    ytdlp_player_client: str | None = None
    caffeinate: bool = True
    net_watch: bool = True
    stall_timeout: float = 30.0
    require_ac: bool = False
    pause_on_low_power: bool = False
    isp: str = "Duronto Internet"

    def validate(self) -> None:
        """Validate all configuration fields and normalize values. Raises ValueError on bad input."""
        if not isinstance(self.schema_version, int) or self.schema_version < 1:
            raise ValueError(f"schema_version must be a positive integer, got {self.schema_version!r}")

        if isinstance(self.connections, bool) or not isinstance(self.connections, int):
            raise ValueError(f"connections must be an integer (1-16), got {self.connections!r}")
        if self.connections < 1:
            raise ValueError(f"connections must be >= 1, got {self.connections}")
        self.connections = min(self.connections, 16)

        if not isinstance(self.output_dir, str) or not self.output_dir.strip():
            raise ValueError("output_dir must be a non-empty string")

        if not isinstance(self.audio_codec, str) or self.audio_codec.lower() not in VALID_AUDIO_CODECS:
            raise ValueError(f"audio_codec must be one of {sorted(VALID_AUDIO_CODECS)}, got {self.audio_codec!r}")
        self.audio_codec = self.audio_codec.lower()

        if not isinstance(self.video_quality, str) or self.video_quality.lower() not in VALID_VIDEO_QUALITIES:
            raise ValueError(
                f"video_quality must be one of {sorted(VALID_VIDEO_QUALITIES)}, got {self.video_quality!r}"
            )
        self.video_quality = self.video_quality.lower()

        if not isinstance(self.auto_extract, bool):
            raise ValueError(f"auto_extract must be a boolean, got {self.auto_extract!r}")
        if not isinstance(self.auto_adb_push, bool):
            raise ValueError(f"auto_adb_push must be a boolean, got {self.auto_adb_push!r}")

        if not isinstance(self.bin_dir, str) or not self.bin_dir.strip():
            raise ValueError("bin_dir must be a non-empty path string")
        if not isinstance(self.adb_music_dir, str) or not self.adb_music_dir.strip():
            raise ValueError("adb_music_dir must be a non-empty path string")
        if not isinstance(self.adb_download_dir, str) or not self.adb_download_dir.strip():
            raise ValueError("adb_download_dir must be a non-empty path string")

        if self.max_download_limit is not None:
            if not isinstance(self.max_download_limit, str):
                raise ValueError(f"max_download_limit must be a rate string or null, got {self.max_download_limit!r}")
            if self.max_download_limit.strip().lower() in ("none", "null", "off", "0", ""):
                self.max_download_limit = None
            else:
                parse_rate(self.max_download_limit)

        if isinstance(self.history_limit, bool) or not isinstance(self.history_limit, int) or self.history_limit < 1:
            raise ValueError(f"history_limit must be a positive integer, got {self.history_limit!r}")

        if self.anti_throttle is not None:
            if not isinstance(self.anti_throttle, str):
                raise ValueError(f"anti_throttle must be a rate string or null, got {self.anti_throttle!r}")
            if self.anti_throttle.strip().lower() in ("none", "null", "off", "0", "false", ""):
                self.anti_throttle = None
            else:
                parse_rate(self.anti_throttle)

        if isinstance(self.anti_throttle_grace, bool) or not isinstance(self.anti_throttle_grace, int):
            raise ValueError(f"anti_throttle_grace must be an integer, got {self.anti_throttle_grace!r}")
        if self.anti_throttle_grace < 0:
            raise ValueError(f"anti_throttle_grace cannot be negative, got {self.anti_throttle_grace}")
        # Reconcile spec and code: clamp minimum grace period to 3s without artificial 5s ceiling
        self.anti_throttle_grace = max(self.anti_throttle_grace, 3)

        if not isinstance(self.watchdog, str) or self.watchdog.lower() not in VALID_WATCHDOG_MODES:
            raise ValueError(f"watchdog must be one of {sorted(VALID_WATCHDOG_MODES)}, got {self.watchdog!r}")
        self.watchdog = self.watchdog.lower()

        if self.ytdlp_player_client is not None:
            if not isinstance(self.ytdlp_player_client, str):
                raise ValueError("ytdlp_player_client must be a string or null")
            if self.ytdlp_player_client.strip().lower() in ("none", "null", "default", ""):
                self.ytdlp_player_client = None

        for bool_field in ("caffeinate", "net_watch", "require_ac", "pause_on_low_power"):
            val = getattr(self, bool_field)
            if not isinstance(val, bool):
                raise ValueError(f"{bool_field} must be a boolean, got {val!r}")

        if isinstance(self.stall_timeout, bool) or not isinstance(self.stall_timeout, (int, float)):
            raise ValueError(f"stall_timeout must be a number >= 0, got {self.stall_timeout!r}")
        if self.stall_timeout < 0:
            raise ValueError(f"stall_timeout cannot be negative, got {self.stall_timeout}")
        self.stall_timeout = float(self.stall_timeout)

        if not isinstance(self.isp, str) or not self.isp.strip():
            raise ValueError("isp must be a non-empty string")
        self.isp = self.isp.strip()

    @classmethod
    def _backup_corrupt_file(cls, cfg_file: Path, exc: Exception) -> None:
        bad_file = cfg_file.with_suffix(cfg_file.suffix + ".bad")
        logger.warning("Corrupt config file %s (%s); backing up to %s", cfg_file, exc, bad_file)
        try:
            shutil.copy2(cfg_file, bad_file)
        except OSError as copy_err:
            logger.debug("Failed to back up corrupt config file: %s", copy_err)

    @classmethod
    def load(cls) -> PDLConfig:
        cfg_file = get_config_file()
        if not cfg_file.exists():
            return cls()

        try:
            raw_text = cfg_file.read_text(encoding="utf-8")
            data = json.loads(raw_text)
            if not isinstance(data, dict):
                raise ValueError("Root of config.json must be a JSON object")
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            cls._backup_corrupt_file(cfg_file, exc)
            return cls()

        known_kwargs: dict[str, Any] = {}
        for k, v in data.items():
            if hasattr(cls, k):
                known_kwargs[k] = v
        known_kwargs["schema_version"] = CONFIG_SCHEMA_VERSION

        try:
            cfg = cls(**known_kwargs)
            cfg.validate()
            return cfg
        except (TypeError, ValueError) as exc:
            cls._backup_corrupt_file(cfg_file, exc)
            return cls()

    def save(self) -> None:
        """Atomically save configuration to PDL_HOME/config.json via temp file + os.replace."""
        self.schema_version = CONFIG_SCHEMA_VERSION
        self.validate()
        pdl_home = get_pdl_home()
        pdl_home.mkdir(parents=True, exist_ok=True)
        cfg_file = get_config_file()

        payload = json.dumps(asdict(self), indent=2) + "\n"
        fd, tmp_path = tempfile.mkstemp(prefix=".config_", suffix=".tmp", dir=str(pdl_home))
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(payload)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp_path, cfg_file)
        except Exception:
            if os.path.exists(tmp_path):
                try:
                    os.remove(tmp_path)
                except OSError as rm_err:
                    logger.debug("Failed to clean temp config file: %s", rm_err)
            raise

    def set(self, key: str, value: Any) -> tuple[bool, str]:
        """Set a configuration key with type coercion and validation.

        Returns (success, message).
        """
        if not hasattr(self, key) or key == "schema_version":
            return False, f"Unknown configuration key: {key!r}"

        orig_val = getattr(self, key)
        try:
            if key in ("auto_extract", "auto_adb_push", "caffeinate", "net_watch", "require_ac", "pause_on_low_power"):
                if isinstance(value, bool):
                    coerced: Any = value
                elif str(value).strip().lower() in ("true", "1", "yes", "on"):
                    coerced = True
                elif str(value).strip().lower() in ("false", "0", "no", "off"):
                    coerced = False
                else:
                    return False, f"Invalid boolean value for {key}: {value!r} (expected true/false)"
            elif key in ("connections", "history_limit", "anti_throttle_grace"):
                try:
                    coerced = int(str(value).strip())
                except ValueError:
                    return False, f"Invalid integer value for {key}: {value!r}"
            elif key == "stall_timeout":
                try:
                    coerced = float(str(value).strip())
                except ValueError:
                    return False, f"Invalid numeric value for {key}: {value!r}"
            elif key in ("max_download_limit", "anti_throttle", "ytdlp_player_client"):
                coerced = None if value is None else str(value).strip()
            else:
                coerced = str(value).strip()

            setattr(self, key, coerced)
            self.validate()
            self.save()
            return True, f"Updated {key} -> {getattr(self, key)}"
        except ValueError as exc:
            setattr(self, key, orig_val)
            return False, str(exc)
