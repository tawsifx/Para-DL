"""Centralized state path management honoring PDL_HOME."""

from __future__ import annotations

import os
from pathlib import Path


def get_pdl_home() -> Path:
    """Return the PDL state directory, honoring PDL_HOME env var (default ~/.config/pdl)."""
    env_home = os.environ.get("PDL_HOME")
    if env_home and env_home.strip():
        return Path(env_home.strip()).expanduser().resolve()
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
