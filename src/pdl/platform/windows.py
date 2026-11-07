"""Windows system integration (SetThreadExecutionState, SYSTEM_POWER_STATUS, netsh WLAN, Zone.Identifier ADS, WinCred).

All Windows calls go through this module with a safe no-op fallback on non-Windows platforms
or when mocked in tests. Zero third-party dependencies (pure standard library: ctypes, subprocess, os).
"""

from __future__ import annotations

import configparser
import ctypes
import io
import logging
import os
import re
import shutil
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

logger = logging.getLogger(__name__)

# Kernel32 Execution State Flags (prevent idle sleep during downloads)
ES_SYSTEM_REQUIRED = 0x00000001
ES_DISPLAY_REQUIRED = 0x00000002
ES_AWAYMODE_REQUIRED = 0x00000040
ES_CONTINUOUS = 0x80000000

_SSID_RE = re.compile(r"^\s*SSID\s*:\s*(.+)$", re.IGNORECASE | re.MULTILINE)
_NET_PROFILE_RE = re.compile(r"Name\s*:\s*(.+)$", re.IGNORECASE | re.MULTILINE)

# Test hook: when True, external system calls use no-op fallbacks
_NOOP_FALLBACK_OVERRIDE: bool = False


def set_noop_fallback(enabled: bool) -> None:
    """Enable or disable forced no-op fallback for tests."""
    global _NOOP_FALLBACK_OVERRIDE
    _NOOP_FALLBACK_OVERRIDE = bool(enabled)


def is_windows() -> bool:
    """Return True when running on Windows (win32)."""
    return sys.platform == "win32"


def is_effective_windows() -> bool:
    """Return True when on Windows and not in forced no-op test mode."""
    return is_windows() and not _NOOP_FALLBACK_OVERRIDE


def get_kernel32() -> Any:
    """Return ctypes.windll.kernel32 if available on Windows, else None."""
    if not is_effective_windows():
        return None
    try:
        return ctypes.windll.kernel32  # type: ignore[attr-defined]
    except (AttributeError, OSError):
        return None


def get_advapi32() -> Any:
    """Return ctypes.windll.advapi32 if available on Windows, else None."""
    if not is_effective_windows():
        return None
    try:
        return ctypes.windll.advapi32  # type: ignore[attr-defined]
    except (AttributeError, OSError):
        return None


# ---------------------------------------------------------------------------
# Idle-Sleep Assertion (SetThreadExecutionState)
# ---------------------------------------------------------------------------


class WindowsSleepAssertion:
    """Context manager holding a Windows SetThreadExecutionState sleep assertion."""

    def __init__(self, *, enabled: bool = True, pid: int | None = None) -> None:
        self.enabled = enabled
        self.pid = pid
        self._acquired = False

    def __enter__(self) -> WindowsSleepAssertion:
        if not self.enabled:
            return self

        kernel32 = get_kernel32()
        if kernel32:
            try:
                # Tell Windows the system is in active use to prevent idle sleep
                flags = ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                prev = kernel32.SetThreadExecutionState(flags)
                if prev != 0:
                    self._acquired = True
            except Exception as exc:
                logger.debug("Failed to set Windows thread execution state: %s", exc)
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        if self._acquired:
            kernel32 = get_kernel32()
            if kernel32:
                try:
                    # Reset back to continuous default
                    kernel32.SetThreadExecutionState(ES_CONTINUOUS)
                except Exception as exc:
                    logger.debug("Failed to reset Windows thread execution state: %s", exc)
            self._acquired = False


# Backward compatibility alias
CaffeinateAssertion = WindowsSleepAssertion


# ---------------------------------------------------------------------------
# Power Policy (SYSTEM_POWER_STATUS)
# ---------------------------------------------------------------------------


class SYSTEM_POWER_STATUS(ctypes.Structure):
    _fields_ = [
        ("ACLineStatus", ctypes.c_byte),
        ("BatteryFlag", ctypes.c_byte),
        ("BatteryLifePercent", ctypes.c_byte),
        ("SystemStatusFlag", ctypes.c_byte),
        ("BatteryLifeTime", ctypes.c_ulong),
        ("BatteryFullLifeTime", ctypes.c_ulong),
    ]


@dataclass
class PowerStatus:
    on_ac_power: bool = True
    battery_percent: int | None = 100
    low_power_mode: bool = False
    raw_source: str = "AC Power"


def get_power_status() -> PowerStatus:
    """Return current Windows power status (AC vs Battery and Battery Saver mode).

    Falls back to AC Power when not on Windows or when kernel32 is unavailable.
    """
    kernel32 = get_kernel32()
    if not kernel32:
        return PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False, raw_source="AC Power")

    status = SYSTEM_POWER_STATUS()
    try:
        ok = kernel32.GetSystemPowerStatus(ctypes.byref(status))
        if not ok:
            return PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False, raw_source="AC Power")

        # ACLineStatus: 0 = Offline (Battery), 1 = Online (AC), 255 = Unknown
        on_ac = status.ACLineStatus != 0
        raw_source = "AC Power" if status.ACLineStatus == 1 else ("Battery Power" if status.ACLineStatus == 0 else "Unknown Power")

        # BatteryLifePercent: 0..100, or 255 for unknown
        pct = int(status.BatteryLifePercent) if status.BatteryLifePercent <= 100 else None

        # SystemStatusFlag: 1 = Battery Saver is ON (Windows 10/11 Low Power Mode)
        low_power = status.SystemStatusFlag == 1

        return PowerStatus(
            on_ac_power=on_ac,
            battery_percent=pct,
            low_power_mode=low_power,
            raw_source=raw_source,
        )
    except Exception as exc:
        logger.debug("GetSystemPowerStatus call failed: %s", exc)
        return PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False, raw_source="AC Power")


# ---------------------------------------------------------------------------
# Disk Free Space Check
# ---------------------------------------------------------------------------


def get_free_disk_bytes(target_path: Path) -> int:
    """Return available free bytes on the filesystem containing target_path."""
    probe_dir = Path(target_path).expanduser().resolve()
    while not probe_dir.exists() and probe_dir.parent != probe_dir:
        probe_dir = probe_dir.parent
    try:
        usage = shutil.disk_usage(probe_dir)
        return int(usage.free)
    except OSError as exc:
        logger.debug("disk_usage failed for %s: %s", probe_dir, exc)
        return 1 << 50


# ---------------------------------------------------------------------------
# Network Identity & SSID Detection
# ---------------------------------------------------------------------------


def parse_netsh_wlan_ssid(output: str) -> str | None:
    """Parse active SSID from `netsh wlan show interfaces` output."""
    if not output:
        return None
    m = _SSID_RE.search(output)
    if m:
        ssid = m.group(1).strip()
        if ssid:
            return ssid
    return None


def get_network_id() -> str:
    """Return current network identifier (Wi-Fi SSID, connection profile, or 'default')."""
    if not is_effective_windows():
        return "default"

    # 1. Try netsh wlan show interfaces (instant, no powershell startup delay)
    try:
        proc = subprocess.run(
            ["netsh", "wlan", "show", "interfaces"],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
        )
        if proc.returncode == 0 and proc.stdout:
            ssid = parse_netsh_wlan_ssid(proc.stdout)
            if ssid:
                return ssid
    except (OSError, subprocess.SubprocessError):
        pass

    # 2. Try PowerShell Get-NetConnectionProfile
    try:
        proc = subprocess.run(
            [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-NetConnectionProfile | Select-Object -ExpandProperty Name)[0]",
            ],
            capture_output=True,
            text=True,
            timeout=3.0,
            check=False,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            return proc.stdout.strip().splitlines()[0].strip()
    except (OSError, subprocess.SubprocessError):
        pass

    return "default"


# ---------------------------------------------------------------------------
# Network Watcher for Windows
# ---------------------------------------------------------------------------


class WindowsNetworkWatcher:
    """Monitors Windows network changes (SSID or primary network ID transition)."""

    def __init__(
        self,
        on_change: Callable[[str], None],
        poll_interval_sec: float = 3.0,
    ) -> None:
        self.on_change = on_change
        self.poll_interval_sec = max(1.0, float(poll_interval_sec))
        self._stop_event = threading.Event()
        self._thread: threading.Thread | None = None
        self._last_net_id: str | None = None

    def start(self) -> None:
        if self._thread and self._thread.is_alive():
            return
        self._stop_event.clear()
        self._last_net_id = get_network_id()
        self._thread = threading.Thread(target=self._run, name="pdl-win-netwatch", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop_event.set()
        if self._thread and self._thread.is_alive():
            self._thread.join(timeout=1.0)
        self._thread = None

    def _run(self) -> None:
        while not self._stop_event.wait(self.poll_interval_sec):
            try:
                current_id = get_network_id()
                if self._last_net_id is not None and current_id != self._last_net_id:
                    detail = f"Network changed from '{self._last_net_id}' to '{current_id}'"
                    self._last_net_id = current_id
                    try:
                        self.on_change(detail)
                    except Exception as exc:
                        logger.debug("on_change callback error in WindowsNetworkWatcher: %s", exc)
                else:
                    self._last_net_id = current_id
            except Exception as exc:
                logger.debug("Error during Windows network check: %s", exc)


# ---------------------------------------------------------------------------
# NTFS Alternate Data Stream (:Zone.Identifier) & Origin Tracking
# ---------------------------------------------------------------------------


def parse_zone_identifier_text(text: str) -> dict[str, str]:
    """Parse INI contents of a Zone.Identifier stream."""
    if not text:
        return {}
    config = configparser.ConfigParser(interpolation=None)
    config.optionxform = str
    try:
        config.read_string(text)
        if config.has_section("ZoneTransfer"):
            return dict(config.items("ZoneTransfer"))
    except Exception:
        # Fallback manual line parsing
        res: dict[str, str] = {}
        for line in text.splitlines():
            line = line.strip()
            if "=" in line and not line.startswith("["):
                k, v = line.split("=", 1)
                res[k.strip()] = v.strip()
        return res
    return {}


def format_zone_identifier_text(host_url: str | None, zone_id: int | None = None) -> str:
    """Format Zone.Identifier INI string with optional ZoneId and HostUrl."""
    lines = ["[ZoneTransfer]"]
    if zone_id is not None:
        lines.append(f"ZoneId={zone_id}")
    if host_url:
        lines.append(f"HostUrl={host_url}")
    return "\r\n".join(lines) + "\r\n"


def unquarantine_zone_identifier_text(text: str) -> str:
    """Remove ZoneId from Zone.Identifier INI while preserving HostUrl."""
    data = parse_zone_identifier_text(text)
    data.pop("zoneid", None)
    data.pop("ZoneId", None)
    if not data:
        return ""
    lines = ["[ZoneTransfer]"]
    for k, v in data.items():
        # Preserve TitleCase for well-known keys
        name = "HostUrl" if k.lower() == "hosturl" else k
        lines.append(f"{name}={v}")
    return "\r\n".join(lines) + "\r\n"


def _ads_path(filepath: Path) -> Path:
    return Path(f"{filepath}:Zone.Identifier")


def set_where_froms(filepath: Path, urls: list[str]) -> bool:
    """Write download origin URL to NTFS Alternate Data Stream `<filepath>:Zone.Identifier`."""
    if not urls or not filepath.exists():
        return False
    primary_url = urls[0]
    ads = _ads_path(filepath)
    try:
        existing_text = ""
        if ads.exists():
            try:
                existing_text = ads.read_text(encoding="utf-8", errors="replace")
            except Exception:
                pass
        parsed = parse_zone_identifier_text(existing_text)
        zone_id = None
        if "zoneid" in parsed:
            try:
                zone_id = int(parsed["zoneid"])
            except ValueError:
                pass
        new_text = format_zone_identifier_text(primary_url, zone_id=zone_id)
        ads.write_text(new_text, encoding="utf-8")
        return True
    except (OSError, PermissionError) as exc:
        logger.debug("Failed to write :Zone.Identifier stream to %s: %s", filepath, exc)
        return False


def get_where_froms(filepath: Path) -> list[str]:
    """Retrieve origin URLs recorded in `<filepath>:Zone.Identifier`."""
    if not filepath.exists():
        return []
    ads = _ads_path(filepath)
    try:
        if ads.exists():
            text = ads.read_text(encoding="utf-8", errors="replace")
            data = parse_zone_identifier_text(text)
            url = data.get("hosturl") or data.get("HostUrl")
            if url:
                return [url]
    except (OSError, PermissionError):
        pass
    return []


def has_quarantine(filepath: Path) -> bool:
    """Return True if the file has an active Mark of the Web (ZoneId >= 3)."""
    if not filepath.exists():
        return False
    ads = _ads_path(filepath)
    try:
        if ads.exists():
            text = ads.read_text(encoding="utf-8", errors="replace")
            data = parse_zone_identifier_text(text)
            z_id_str = data.get("zoneid") or data.get("ZoneId")
            if z_id_str:
                return int(z_id_str) >= 3
    except (OSError, PermissionError, ValueError):
        pass
    return False


def remove_quarantine(filepath: Path) -> bool:
    """Remove Mark of the Web (ZoneId) from `<filepath>:Zone.Identifier` while preserving HostUrl."""
    if not filepath.exists():
        return False
    ads = _ads_path(filepath)
    try:
        if not ads.exists():
            return True
        text = ads.read_text(encoding="utf-8", errors="replace")
        updated = unquarantine_zone_identifier_text(text)
        if updated.strip():
            ads.write_text(updated, encoding="utf-8")
        else:
            # Delete empty stream if only ZoneId was present
            try:
                os.remove(str(ads))
            except OSError:
                # Alternatively PowerShell Unblock-File
                subprocess.run(
                    ["powershell.exe", "-NoProfile", "-NonInteractive", "Unblock-File", "-LiteralPath", str(filepath)],
                    capture_output=True,
                    check=False,
                )
        return True
    except (OSError, PermissionError) as exc:
        logger.debug("remove_quarantine failed on %s: %s", filepath, exc)
        return False


# ---------------------------------------------------------------------------
# Windows Credential Manager (WinCred & DPAPI Fallback)
# ---------------------------------------------------------------------------


def _cred_target(service: str, account: str) -> str:
    return f"{service}/{account}"


def _get_dpapi_store_path() -> Path:
    local_appdata = os.environ.get("LOCALAPPDATA")
    if local_appdata:
        p = Path(local_appdata) / "pdl" / "credentials.json"
    else:
        p = Path.home() / ".config" / "pdl" / "credentials.json"
    p.parent.mkdir(parents=True, exist_ok=True)
    return p


def set_keychain_token(account: str, token: str, service: str = "pdl") -> bool:
    """Store credential in Windows Credential Manager or secure user store."""
    if not account or not token:
        return False

    # 1. Try Windows Credential Manager CLI cmdkey
    target = _cred_target(service, account)
    try:
        proc = subprocess.run(
            ["cmdkey.exe", f"/generic:{target}", f"/user:{account}", f"/pass:{token}"],
            capture_output=True,
            text=True,
            check=False,
            timeout=3.0,
        )
        if proc.returncode == 0:
            return True
    except (OSError, subprocess.SubprocessError):
        pass

    # 2. User-scoped credential store fallback with restrictive permissions
    try:
        p = _get_dpapi_store_path()
        import json

        data = {}
        if p.exists():
            try:
                data = json.loads(p.read_text(encoding="utf-8"))
            except Exception:
                pass
        data[f"{service}:{account}"] = token
        p.write_text(json.dumps(data, indent=2), encoding="utf-8")
        try:
            os.chmod(p, 0o600)
        except OSError:
            pass
        return True
    except Exception as exc:
        logger.debug("Failed to save credentials in user store: %s", exc)
        return False


def get_keychain_token(account: str, service: str = "pdl") -> str | None:
    """Retrieve token from secure user store."""
    # Note: cmdkey.exe does not allow reading passwords for security reasons.
    # Therefore, tokens stored via set_keychain_token are retrieved from the user-scoped store.
    try:
        p = _get_dpapi_store_path()
        if p.exists():
            import json

            data = json.loads(p.read_text(encoding="utf-8"))
            return data.get(f"{service}:{account}")
    except Exception as exc:
        logger.debug("Failed to read credentials from user store: %s", exc)
    return None


def delete_keychain_token(account: str, service: str = "pdl") -> bool:
    """Delete credential from Windows Credential Manager and secure user store."""
    target = _cred_target(service, account)
    deleted = False

    try:
        proc = subprocess.run(
            ["cmdkey.exe", f"/delete:{target}"],
            capture_output=True,
            check=False,
            timeout=3.0,
        )
        if proc.returncode == 0:
            deleted = True
    except (OSError, subprocess.SubprocessError):
        pass

    try:
        p = _get_dpapi_store_path()
        if p.exists():
            import json

            data = json.loads(p.read_text(encoding="utf-8"))
            if f"{service}:{account}" in data:
                del data[f"{service}:{account}"]
                p.write_text(json.dumps(data, indent=2), encoding="utf-8")
                deleted = True
    except Exception as exc:
        logger.debug("Failed to delete credentials from user store: %s", exc)

    return deleted


def has_keychain_token(account: str, service: str = "pdl") -> bool:
    """Return True if credential exists."""
    tok = get_keychain_token(account, service)
    return bool(tok and tok.strip())


# ---------------------------------------------------------------------------
# Windows Toast Notifications
# ---------------------------------------------------------------------------


def display_notification(
    title: str,
    message: str,
    subtitle: str | None = None,
    timeout: float = 3.0,
) -> bool:
    """Display a Windows 10/11 desktop toast notification via PowerShell."""
    if not is_effective_windows():
        return False

    full_body = f"{subtitle} - {message}" if subtitle else message
    safe_title = title.replace('"', '`"').replace("$", "`$")
    safe_body = full_body.replace('"', '`"').replace("$", "`$")

    ps_script = f"""
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
$template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$textNodes = $template.GetElementsByTagName('text')
$textNodes.Item(0).AppendChild($template.CreateTextNode("{safe_title}")) > $null
$textNodes.Item(1).AppendChild($template.CreateTextNode("{safe_body}")) > $null
$notifier = [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('parallel-dl')
$notification = [Windows.UI.Notifications.ToastNotification]::new($template)
$notifier.Show($notification)
"""
    try:
        proc = subprocess.run(
            [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                ps_script,
            ],
            capture_output=True,
            timeout=4.0,
            check=False,
        )
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("PowerShell toast notification failed: %s", exc)
        return False


# ---------------------------------------------------------------------------
# Diagnostics & Windows Environment Utilities Check
# ---------------------------------------------------------------------------


def check_windows_utilities() -> dict[str, dict[str, Any]]:
    """Audit Windows platform utilities (powershell, curl, tar, winget, etc.)."""
    utils = {
        "powershell": {
            "path": shutil.which("powershell") or shutil.which("pwsh"),
            "fix": "Install PowerShell 7 or enable Windows PowerShell in Windows Features",
            "required": True,
        },
        "curl": {
            "path": shutil.which("curl"),
            "fix": "curl is included with Windows 10 build 1809+ / Windows 11. Ensure C:\\Windows\\System32 is on PATH.",
            "required": True,
        },
        "tar": {
            "path": shutil.which("tar"),
            "fix": "tar is included with Windows 10 build 1809+ / Windows 11. Ensure C:\\Windows\\System32 is on PATH.",
            "required": False,
        },
        "winget": {
            "path": shutil.which("winget"),
            "fix": "Install 'App Installer' from the Microsoft Store or GitHub: https://github.com/microsoft/winget-cli",
            "required": False,
        },
    }

    result: dict[str, dict[str, Any]] = {}
    for name, meta in utils.items():
        found = meta["path"] is not None
        result[name] = {
            "available": found,
            "path": meta["path"],
            "fix": meta["fix"] if not found else None,
            "required": meta["required"],
        }
    return result
