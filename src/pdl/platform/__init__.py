"""Platform-specific integration modules and cross-platform dispatchers for pdl."""

from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

from pdl.platform import macos, windows


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


def is_macos() -> bool:
    """Return True when running on macOS (darwin)."""
    return sys.platform == "darwin"


def get_current_platform_module() -> Any:
    """Return the active platform module (windows on win32, macos on darwin/others)."""
    if is_windows():
        return windows
    return macos


def get_sleep_assertion(*, enabled: bool = True, pid: int | None = None) -> Any:
    """Return platform-appropriate idle-sleep prevention context manager."""
    if is_windows():
        return windows.WindowsSleepAssertion(enabled=enabled, pid=pid)
    return macos.CaffeinateAssertion(enabled=enabled, pid=pid)


def get_power_status() -> Any:
    """Return platform power status (AC vs Battery, % and Low Power / Battery Saver mode)."""
    return get_current_platform_module().get_power_status()


def get_network_id() -> str:
    """Return current network identifier (SSID or primary connection ID)."""
    return get_current_platform_module().get_network_id()


def get_network_watcher_cls() -> type:
    """Return platform network watcher class."""
    if is_windows():
        return windows.WindowsNetworkWatcher
    return macos.MacOSNetworkWatcher


def set_where_froms(filepath: Path, urls: list[str]) -> bool:
    """Record download origin URL in metadata (WhereFroms on macOS, :Zone.Identifier on Windows)."""
    return get_current_platform_module().set_where_froms(filepath, urls)


def get_where_froms(filepath: Path) -> list[str]:
    """Retrieve download origin URLs from metadata."""
    return get_current_platform_module().get_where_froms(filepath)


def has_quarantine(filepath: Path) -> bool:
    """Check if file has active quarantine or Mark of the Web."""
    return get_current_platform_module().has_quarantine(filepath)


def remove_quarantine(filepath: Path) -> bool:
    """Remove quarantine or Mark of the Web from a verified file."""
    return get_current_platform_module().remove_quarantine(filepath)


def display_notification(
    title: str,
    message: str,
    subtitle: str | None = None,
    sound: bool = True,
) -> bool:
    """Display platform desktop notification."""
    return get_current_platform_module().display_notification(
        title=title, message=message, subtitle=subtitle, sound=sound
    )


def get_keychain_token(account: str, service: str = "pdl") -> str | None:
    """Retrieve authentication token from secure platform store."""
    return get_current_platform_module().get_keychain_token(account=account, service=service)


def set_keychain_token(account: str, token: str, service: str = "pdl") -> bool:
    """Save authentication token in secure platform store."""
    return get_current_platform_module().set_keychain_token(account=account, token=token, service=service)


def delete_keychain_token(account: str, service: str = "pdl") -> bool:
    """Delete authentication token from secure platform store."""
    return get_current_platform_module().delete_keychain_token(account=account, service=service)


def has_keychain_token(account: str, service: str = "pdl") -> bool:
    """Check if authentication token exists in secure platform store."""
    return get_current_platform_module().has_keychain_token(account=account, service=service)


def check_platform_utilities() -> dict[str, dict[str, Any]]:
    """Audit platform utilities."""
    if is_windows():
        return windows.check_windows_utilities()
    return macos.check_macos_utilities()


__all__ = [
    "macos",
    "windows",
    "is_windows",
    "is_macos",
    "get_sleep_assertion",
    "get_power_status",
    "get_network_id",
    "get_network_watcher_cls",
    "set_where_froms",
    "get_where_froms",
    "has_quarantine",
    "remove_quarantine",
    "display_notification",
    "get_keychain_token",
    "set_keychain_token",
    "delete_keychain_token",
    "has_keychain_token",
    "check_platform_utilities",
]
