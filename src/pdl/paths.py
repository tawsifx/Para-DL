"""Centralized state path management honoring PDL_HOME."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


def get_pdl_home() -> Path:
    """Return the PDL state directory, honoring PDL_HOME env var (default ~/.config/pdl or %LOCALAPPDATA%/pdl)."""
    env_home = os.environ.get("PDL_HOME")
    if env_home and env_home.strip():
        return Path(env_home.strip()).expanduser().resolve()

    if is_windows():
        local_appdata = os.environ.get("LOCALAPPDATA") or os.environ.get("APPDATA")
        if local_appdata and local_appdata.strip():
            return (Path(local_appdata.strip()) / "pdl").resolve()

    return (Path.home() / ".config" / "pdl").resolve()


def get_config_file() -> Path:
    """Return path to config.json inside PDL_HOME."""
    return get_pdl_home() / "config.json"


def get_legacy_history_file() -> Path:
    """Return path to legacy history.json inside PDL_HOME."""
    return get_pdl_home() / "history.json"


def get_history_db_file() -> Path:
    """Return path to SQLite history.db inside PDL_HOME."""
    return get_pdl_home() / "history.db"


def get_mirrors_file() -> Path:
    """Return path to mirrors.json inside PDL_HOME."""
    return get_pdl_home() / "mirrors.json"


def get_logs_dir() -> Path:
    """Return path to logs directory inside PDL_HOME."""
    return get_pdl_home() / "logs"


def get_throttle_dir() -> Path:
    """Return path to throttle diagnostic reports directory inside PDL_HOME."""
    return get_pdl_home() / "throttle"


def get_targets_file() -> Path:
    """Return path to custom targets.toml inside PDL_HOME."""
    return get_pdl_home() / "targets.toml"
