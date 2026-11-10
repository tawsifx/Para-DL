"""Centralized state path management honoring PARADL_HOME."""

from __future__ import annotations

import os
import sys
from pathlib import Path


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


def get_paradl_home() -> Path:
    """Return the pdl state directory, honoring PARADL_HOME env var (default ~/.config/paradl or %LOCALAPPDATA%/paradl)."""
    env_home = os.environ.get("PARADL_HOME")
    if env_home and env_home.strip():
        return Path(env_home.strip()).expanduser().resolve()

    if is_windows():
        local_appdata = os.environ.get("LOCALAPPDATA") or os.environ.get("APPDATA")
        if local_appdata and local_appdata.strip():
            return (Path(local_appdata.strip()) / "paradl").resolve()

    return (Path.home() / ".config" / "paradl").resolve()


def get_config_file() -> Path:
    """Return path to config.json inside PARADL_HOME."""
    return get_paradl_home() / "config.json"


def get_legacy_history_file() -> Path:
    """Return path to legacy history.json inside PARADL_HOME."""
    return get_paradl_home() / "history.json"


def get_history_db_file() -> Path:
    """Return path to SQLite history.db inside PARADL_HOME."""
    return get_paradl_home() / "history.db"


def get_mirrors_file() -> Path:
    """Return path to mirrors.json inside PARADL_HOME."""
    return get_paradl_home() / "mirrors.json"


def get_logs_dir() -> Path:
    """Return path to logs directory inside PARADL_HOME."""
    return get_paradl_home() / "logs"


def get_throttle_dir() -> Path:
    """Return path to throttle diagnostic reports directory inside PARADL_HOME."""
    return get_paradl_home() / "throttle"


def get_targets_file() -> Path:
    """Return path to custom targets.toml inside PARADL_HOME."""
    return get_paradl_home() / "targets.toml"
