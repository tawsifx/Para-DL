"""macOS system integration (caffeinate, scutil/route network watcher, pmset power policy, network ID).

All macOS calls go through this module with a safe no-op fallback on non-macOS platforms
or when mocked in tests. Never invokes launchctl or installs anything system-wide.
"""

from __future__ import annotations

import contextlib
import logging
import os
import plistlib
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

# Verified at runtime on macOS Apple Silicon:
# - Literal keys for global IPv4/IPv6 and primary en0 AirPort
# - Pattern keys for all interface AirPort and IPv4/IPv6 state dictionaries
SCUTIL_WATCH_COMMANDS: tuple[str, ...] = (
    "n.add State:/Network/Global/IPv4",
    "n.add State:/Network/Global/IPv6",
    "n.add State:/Network/Interface/en0/AirPort",
    "n.add State:/Network/Interface/.*/AirPort pattern",
    "n.add State:/Network/Interface/.*/IPv4 pattern",
    "n.add State:/Network/Interface/.*/IPv6 pattern",
    "n.watch",
)

_BATT_PCT_RE = re.compile(r"(\d+)%")
_LOW_POWER_RE = re.compile(r"lowpowermode\s+(\d+)", re.IGNORECASE)
_PRIMARY_IFACE_RE = re.compile(r"PrimaryInterface\s*:\s*([A-Za-z0-9_]+)")
_NWI_IFACES_RE = re.compile(r"Network interfaces:\s*([A-Za-z0-9_]+)")
_SSID_STR_RE = re.compile(r"SSID_STR\s*:\s*(.+)")
_AIRPORT_NET_RE = re.compile(r"Current (?:Wi-Fi|AirPort) Network:\s*(.+)", re.IGNORECASE)

# Test hook: when True, external background daemons (caffeinate, scutil, route) use no-op fallbacks
# unless explicitly mocked or enabled by a test.
_NOOP_FALLBACK_OVERRIDE: bool = False


def set_noop_fallback(enabled: bool) -> None:
    """Enable or disable forced no-op fallback for tests."""
    global _NOOP_FALLBACK_OVERRIDE  # noqa: PLW0603
    _NOOP_FALLBACK_OVERRIDE = bool(enabled)


def is_macos() -> bool:
    """Return True when running on macOS (Darwin)."""
    return sys.platform == "darwin"


def is_effective_macos() -> bool:
    """Return True when on macOS and not in forced no-op test mode."""
    return is_macos() and not _NOOP_FALLBACK_OVERRIDE


# ---------------------------------------------------------------------------
# Idle-Sleep Assertion (caffeinate -i -w <pdl pid>)
# ---------------------------------------------------------------------------


def start_caffeinate(pid: int | None = None) -> subprocess.Popen[Any] | None:
    """Hold an idle-sleep assertion with `caffeinate -i -w <pdl pid>` while a download runs.

    Returns the Popen handle or None (no-op fallback when disabled or unavailable).
    """
    if not is_effective_macos():
        return None

    caffeinate_bin = shutil.which("caffeinate") or (
        "/usr/bin/caffeinate" if os.path.exists("/usr/bin/caffeinate") else None
    )
    if not caffeinate_bin:
        return None

    target_pid = pid if pid is not None else os.getpid()
    cmd = [caffeinate_bin, "-i", "-w", str(target_pid)]
    try:
        return subprocess.Popen(
            cmd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except OSError as exc:
        logger.debug("caffeinate start failed (no-op fallback): %s", exc)
        return None


def stop_caffeinate(proc: subprocess.Popen[Any] | None) -> None:
    """Terminate an active caffeinate process cleanly."""
    if proc is None:
        return
    try:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=1.5)
            except subprocess.TimeoutExpired:
                proc.kill()
    except OSError as exc:
        logger.debug("Error stopping caffeinate: %s", exc)


class CaffeinateAssertion:
    """Context manager holding a `caffeinate -i -w <pid>` assertion while enabled."""

    def __init__(self, *, enabled: bool = True, pid: int | None = None) -> None:
        self.enabled = enabled
        self.pid = pid
        self.proc: subprocess.Popen[Any] | None = None

    def __enter__(self) -> CaffeinateAssertion:
        if self.enabled:
            self.proc = start_caffeinate(self.pid)
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        stop_caffeinate(self.proc)
        self.proc = None


# ---------------------------------------------------------------------------
# Power Policy (pmset -g batt & Low Power Mode)
# ---------------------------------------------------------------------------


@dataclass
class PowerStatus:
    on_ac_power: bool = True
    battery_percent: int | None = 100
    low_power_mode: bool = False
    raw_source: str = "AC Power"


def parse_pmset_batt(output: str) -> tuple[bool, int | None, str]:
    """Parse `pmset -g batt` output into (on_ac_power, battery_percent, raw_source)."""
    text = output or ""
    on_ac = True
    source = "AC Power"
    if "Battery Power" in text:
        on_ac = False
        source = "Battery Power"
    elif "AC Power" in text:
        on_ac = True
        source = "AC Power"
    elif "UPS Power" in text:
        on_ac = False
        source = "UPS Power"

    m_pct = _BATT_PCT_RE.search(text)
    pct = int(m_pct.group(1)) if m_pct else None
    return on_ac, pct, source


def parse_pmset_general(output: str) -> bool:
    """Parse `pmset -g` output and return True if lowpowermode is enabled (1)."""
    if not output:
        return False
    m = _LOW_POWER_RE.search(output)
    if m:
        return int(m.group(1)) != 0
    return False


def run_pmset(args: list[str], timeout: float = 2.0) -> str:
    """Execute `pmset <args>` on macOS and return stdout, or empty string on fallback."""
    if not is_effective_macos():
        return ""
    pmset_bin = shutil.which("pmset") or (
        "/usr/bin/pmset" if os.path.exists("/usr/bin/pmset") else None
    )
    if not pmset_bin:
        return ""
    try:
        proc = subprocess.run(
            [pmset_bin, *args],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return proc.stdout or ""
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("pmset %s failed: %s", args, exc)
        return ""


def get_power_status() -> PowerStatus:
    """Return current macOS power status (AC vs Battery and Low Power Mode).

    Falls back to AC Power + Low Power Mode off when not on macOS or when pmset is unavailable.
    """
    batt_out = run_pmset(["-g", "batt"])
    gen_out = run_pmset(["-g"])
    if not batt_out and not gen_out:
        return PowerStatus(
            on_ac_power=True, battery_percent=100, low_power_mode=False, raw_source="AC Power"
        )

    on_ac, pct, source = parse_pmset_batt(batt_out)
    lpm = parse_pmset_general(gen_out)
    return PowerStatus(
        on_ac_power=on_ac,
        battery_percent=pct,
        low_power_mode=lpm,
        raw_source=source,
    )


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
        return 1 << 50  # No-op fallback if virtual/unstatable path


# ---------------------------------------------------------------------------
# Network Identity (SSID or Primary Interface)
# ---------------------------------------------------------------------------


def run_scutil_query(commands: list[str], timeout: float = 2.0) -> str:
    """Run non-interactive `scutil` query commands via stdin and return stdout."""
    if not is_effective_macos():
        return ""
    scutil_bin = shutil.which("scutil") or (
        "/usr/sbin/scutil" if os.path.exists("/usr/sbin/scutil") else None
    )
    if not scutil_bin:
        return ""
    script = "\n".join([*commands, "quit"]) + "\n"
    try:
        proc = subprocess.run(
            [scutil_bin],
            input=script,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return proc.stdout or ""
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("scutil query failed: %s", exc)
        return ""


def run_scutil_nwi(timeout: float = 2.0) -> str:
    """Run `scutil --nwi` and return stdout."""
    if not is_effective_macos():
        return ""
    scutil_bin = shutil.which("scutil") or (
        "/usr/sbin/scutil" if os.path.exists("/usr/sbin/scutil") else None
    )
    if not scutil_bin:
        return ""
    try:
        proc = subprocess.run(
            [scutil_bin, "--nwi"],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return proc.stdout or ""
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("scutil --nwi failed: %s", exc)
        return ""


def get_network_id() -> str:
    """Return active network identifier (Wi-Fi SSID if available, else primary interface like 'en0').

    Falls back to 'default' on non-macOS or when offline/mocked.
    """
    if not is_effective_macos():
        return "default"

    # 1. Query Global IPv4 and AirPort state via scutil in one shot
    out = run_scutil_query(
        [
            "show State:/Network/Global/IPv4",
            "show State:/Network/Global/IPv6",
            "show State:/Network/Interface/en0/AirPort",
        ]
    )
    primary_iface = ""
    m_iface = _PRIMARY_IFACE_RE.search(out)
    if m_iface:
        primary_iface = m_iface.group(1).strip()

    if primary_iface and primary_iface != "en0":
        out_iface = run_scutil_query([f"show State:/Network/Interface/{primary_iface}/AirPort"])
        if out_iface:
            out += "\n" + out_iface

    m_ssid = _SSID_STR_RE.search(out)
    if m_ssid:
        ssid = m_ssid.group(1).strip()
        if ssid and ssid not in ("<redacted>", "''", '""'):
            return ssid

    # 2. Try networksetup -getairportnetwork <iface> if on Wi-Fi interface
    iface_to_check = primary_iface or "en0"
    netsetup_bin = shutil.which("networksetup") or (
        "/usr/sbin/networksetup" if os.path.exists("/usr/sbin/networksetup") else None
    )
    if netsetup_bin:
        try:
            proc = subprocess.run(
                [netsetup_bin, "-getairportnetwork", iface_to_check],
                capture_output=True,
                text=True,
                timeout=1.5,
                check=False,
            )
            m_net = _AIRPORT_NET_RE.search(proc.stdout or "")
            if m_net:
                ssid = m_net.group(1).strip()
                if ssid and ssid != "<redacted>":
                    return ssid
        except (OSError, subprocess.SubprocessError):
            pass

    if primary_iface:
        return primary_iface

    # 3. Fallback to scutil --nwi interface list
    nwi_out = run_scutil_nwi()
    m_nwi = _NWI_IFACES_RE.search(nwi_out)
    if m_nwi:
        return m_nwi.group(1).strip()

    return "default"


# ---------------------------------------------------------------------------
# Tiered Network Watcher (scutil -> route -n monitor -> scutil --nwi slow poll)
# ---------------------------------------------------------------------------


def normalize_nwi_signature(nwi_output: str) -> str:
    """Normalize `scutil --nwi` output into a stable string for change detection."""
    lines: list[str] = []
    for raw in (nwi_output or "").splitlines():
        s = raw.strip()
        if any(
            s.startswith(prefix)
            for prefix in (
                "en",
                "utun",
                "bridge",
                "pdp_ip",
                "address",
                "reach",
                "REACH",
                "Network interfaces:",
            )
        ):
            lines.append(s)
    return "\n".join(lines)


class MacOSNetworkWatcher:
    """Watches for macOS network changes with tiered fallback:
    1. Interactive `scutil` (`n.add` on Global IPv4/IPv6 + Wi-Fi AirPort keys, then `n.watch`)
    2. `route -n monitor`
    3. Slow poll of `scutil --nwi`
    4. No-op fallback on non-macOS or when disabled/mocked
    """

    def __init__(
        self,
        *,
        enabled: bool = True,
        nwi_poll_interval_sec: float = 5.0,
        popen_factory: Callable[..., subprocess.Popen[Any]] | None = None,
        nwi_runner: Callable[[], str] | None = None,
        mono_clock: Callable[[], float] = time.monotonic,
    ) -> None:
        self.enabled = enabled
        self.nwi_poll_interval_sec = max(0.2, float(nwi_poll_interval_sec))
        self._popen_factory = popen_factory or subprocess.Popen
        self._nwi_runner = nwi_runner or run_scutil_nwi
        self._mono_clock = mono_clock

        self.backend: str = "noop"
        self._proc: subprocess.Popen[Any] | None = None
        self._reader_thread: threading.Thread | None = None
        self._lock = threading.Lock()
        self._pending_notifications: list[str] = []
        self._stopped = False
        self._last_nwi_poll_mono: float = 0.0
        self._last_nwi_signature: str = ""

    def start(self) -> str:
        """Start the highest-priority available network watcher and return backend name."""
        self._stopped = False
        if not self.enabled:
            self.backend = "noop"
            return self.backend

        # Allow explicit test injection via custom popen_factory or nwi_runner even when _NOOP_FALLBACK_OVERRIDE is True
        has_custom_hooks = (self._popen_factory is not subprocess.Popen) or (
            self._nwi_runner is not run_scutil_nwi
        )
        if not is_effective_macos() and not has_custom_hooks:
            self.backend = "noop"
            return self.backend

        if self._try_start_scutil():
            self.backend = "scutil"
            return self.backend

        if self._try_start_route_monitor():
            self.backend = "route_monitor"
            return self.backend

        if self._try_init_nwi_poll():
            self.backend = "scutil_nwi_poll"
            return self.backend

        self.backend = "noop"
        return self.backend

    def _try_start_scutil(self) -> bool:
        scutil_bin = shutil.which("scutil") or (
            "/usr/sbin/scutil" if os.path.exists("/usr/sbin/scutil") else "scutil"
        )
        try:
            proc = self._popen_factory(
                [scutil_bin],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                bufsize=1,
            )
            if proc.stdin is None or proc.stdout is None:
                self._terminate_proc(proc)
                return False

            init_payload = "\n".join(SCUTIL_WATCH_COMMANDS) + "\n"
            proc.stdin.write(init_payload)
            proc.stdin.flush()

            if proc.poll() is not None:
                return False

            self._proc = proc
            self._reader_thread = threading.Thread(
                target=self._stdout_reader_loop,
                args=(proc, "scutil"),
                daemon=True,
            )
            self._reader_thread.start()
            return True
        except Exception as exc:
            logger.debug("scutil watcher failed to start: %s", exc)
            return False

    def _try_start_route_monitor(self) -> bool:
        route_bin = shutil.which("route") or (
            "/sbin/route" if os.path.exists("/sbin/route") else "route"
        )
        try:
            proc = self._popen_factory(
                [route_bin, "-n", "monitor"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                bufsize=1,
            )
            if proc.stdout is None or proc.poll() is not None:
                self._terminate_proc(proc)
                return False

            self._proc = proc
            self._reader_thread = threading.Thread(
                target=self._stdout_reader_loop,
                args=(proc, "route_monitor"),
                daemon=True,
            )
            self._reader_thread.start()
            return True
        except Exception as exc:
            logger.debug("route -n monitor failed to start: %s", exc)
            return False

    def _try_init_nwi_poll(self) -> bool:
        try:
            initial = self._nwi_runner()
            if not initial:
                return False
            self._last_nwi_signature = normalize_nwi_signature(initial)
            self._last_nwi_poll_mono = self._mono_clock()
            return True
        except Exception as exc:
            logger.debug("scutil --nwi fallback init failed: %s", exc)
            return False

    def _stdout_reader_loop(self, proc: subprocess.Popen[Any], source_name: str) -> None:
        stdout = proc.stdout
        if stdout is None:
            return
        try:
            for raw_line in stdout:
                if self._stopped:
                    break
                line = raw_line.strip()
                if not line:
                    continue
                if source_name == "scutil":
                    # scutil n.watch outputs notification_callback or changed key lines
                    with self._lock:
                        self._pending_notifications.append(f"scutil: {line}")
                elif source_name == "route_monitor" and any(
                    tok in line
                    for tok in (
                        "RTM_NEWADDR",
                        "RTM_DELADDR",
                        "RTM_IFINFO",
                        "RTM_ADD",
                        "RTM_DELETE",
                        "RTM_CHANGE",
                        "got message",
                    )
                ):
                    with self._lock:
                        self._pending_notifications.append(f"route: {line}")
        except Exception as exc:
            logger.debug("%s reader loop ended: %s", source_name, exc)

    def inject_notification(self, detail: str = "simulated_network_change") -> None:
        """Inject a raw network-change notification (used by simulator and unit tests)."""
        if not self.enabled:
            return
        with self._lock:
            self._pending_notifications.append(detail)

    def poll_notifications(self) -> list[str]:
        """Drain and return all raw network change notifications collected since last poll."""
        if not self.enabled or self._stopped:
            return []

        if self.backend == "scutil_nwi_poll":
            now = self._mono_clock()
            if (now - self._last_nwi_poll_mono) >= self.nwi_poll_interval_sec:
                self._last_nwi_poll_mono = now
                current_raw = self._nwi_runner()
                if current_raw:
                    sig = normalize_nwi_signature(current_raw)
                    if self._last_nwi_signature and sig != self._last_nwi_signature:
                        with self._lock:
                            self._pending_notifications.append("scutil_nwi: network state changed")
                    self._last_nwi_signature = sig

        with self._lock:
            if not self._pending_notifications:
                return []
            items = list(self._pending_notifications)
            self._pending_notifications.clear()
            return items

    @staticmethod
    def _terminate_proc(proc: subprocess.Popen[Any] | None) -> None:
        if proc is None:
            return
        try:
            if proc.stdin:
                with contextlib.suppress(Exception):
                    proc.stdin.close()
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=1.0)
                except subprocess.TimeoutExpired:
                    proc.kill()
        except OSError:
            pass

    def stop(self) -> None:
        """Stop any running watcher subprocess."""
        self._stopped = True
        self._terminate_proc(self._proc)
        self._proc = None

    def __enter__(self) -> MacOSNetworkWatcher:
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop()


# ---------------------------------------------------------------------------
# LaunchAgent Plist Validation (plutil -lint + plistlib fallback)
# ---------------------------------------------------------------------------


def validate_plist(plist_path: Path) -> tuple[bool, str]:
    """Validate a generated macOS plist file via `plistlib` and `/usr/bin/plutil -lint` when available."""
    import plistlib

    p = Path(plist_path)
    if not p.is_file():
        return False, f"Plist file not found: {p}"

    try:
        raw_bytes = p.read_bytes()
        data = plistlib.loads(raw_bytes)
        if not isinstance(data, dict) or "Label" not in data or "ProgramArguments" not in data:
            return False, "Plist missing required keys ('Label', 'ProgramArguments')"
    except Exception as exc:
        return False, f"plistlib parse error: {exc}"

    if is_macos():
        plutil_bin = shutil.which("plutil") or (
            "/usr/bin/plutil" if os.path.exists("/usr/bin/plutil") else None
        )
        if plutil_bin:
            try:
                proc = subprocess.run(
                    [plutil_bin, "-lint", str(p)],
                    capture_output=True,
                    text=True,
                    timeout=3.0,
                    check=False,
                )
                out = (proc.stdout or proc.stderr or "").strip()
                if proc.returncode != 0:
                    return False, f"plutil -lint failed: {out}"
                return True, out or "OK"
            except (OSError, subprocess.SubprocessError) as exc:
                logger.debug("plutil -lint execution warning: %s", exc)

    return True, f"{p}: OK (plistlib verified)"


# ---------------------------------------------------------------------------
# macOS Keychain Integration (security -i without argv secret exposure)
# ---------------------------------------------------------------------------


def set_keychain_token(
    account: str,
    token: str,
    service: str = "pdl",
    keychain: str | None = None,
    timeout: float = 5.0,
) -> bool:
    """Store token in macOS login Keychain via 'security -i' on stdin.

    Guarantees the secret is never exposed in process argv.
    """
    if not is_effective_macos():
        return False
    if not token or not account:
        return False

    security_bin = shutil.which("security") or (
        "/usr/bin/security" if os.path.exists("/usr/bin/security") else "security"
    )
    esc_token = token.replace("\\", "\\\\").replace('"', '\\"')
    esc_account = account.replace("\\", "\\\\").replace('"', '\\"')
    esc_service = service.replace("\\", "\\\\").replace('"', '\\"')
    kc_arg = f' "{keychain}"' if keychain else ""
    cmd = (
        f'add-generic-password -a "{esc_account}" -s "{esc_service}" -U -w "{esc_token}"{kc_arg}\n'
    )

    try:
        proc = subprocess.Popen(
            [security_bin, "-i"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        _, err = proc.communicate(input=cmd, timeout=timeout)
        if proc.returncode != 0:
            logger.debug(
                "security -i add-generic-password failed (rc=%s): %s", proc.returncode, err
            )
            return False
        return True
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("security -i failed: %s", exc)
        return False


def get_keychain_token(
    account: str,
    service: str = "pdl",
    keychain: str | None = None,
    timeout: float = 5.0,
) -> str | None:
    """Retrieve token from macOS login Keychain via 'security -i' on stdin."""
    if not is_effective_macos():
        return None
    if not account:
        return None

    security_bin = shutil.which("security") or (
        "/usr/bin/security" if os.path.exists("/usr/bin/security") else "security"
    )
    esc_account = account.replace("\\", "\\\\").replace('"', '\\"')
    esc_service = service.replace("\\", "\\\\").replace('"', '\\"')
    kc_arg = f' "{keychain}"' if keychain else ""
    cmd = f'find-generic-password -a "{esc_account}" -s "{esc_service}" -w{kc_arg}\n'

    try:
        proc = subprocess.Popen(
            [security_bin, "-i"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        out, _ = proc.communicate(input=cmd, timeout=timeout)
        if proc.returncode == 0 and out.strip():
            return out.strip()
        return None
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("security -i find-generic-password failed: %s", exc)
        return None


def delete_keychain_token(
    account: str,
    service: str = "pdl",
    keychain: str | None = None,
    timeout: float = 5.0,
) -> bool:
    """Delete token from macOS login Keychain via 'security -i' on stdin."""
    if not is_effective_macos():
        return False
    if not account:
        return False

    security_bin = shutil.which("security") or (
        "/usr/bin/security" if os.path.exists("/usr/bin/security") else "security"
    )
    esc_account = account.replace("\\", "\\\\").replace('"', '\\"')
    esc_service = service.replace("\\", "\\\\").replace('"', '\\"')
    kc_arg = f' "{keychain}"' if keychain else ""
    cmd = f'delete-generic-password -a "{esc_account}" -s "{esc_service}"{kc_arg}\n'

    try:
        proc = subprocess.Popen(
            [security_bin, "-i"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        _, _ = proc.communicate(input=cmd, timeout=timeout)
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("security -i delete-generic-password failed: %s", exc)
        return False


def has_keychain_token(
    account: str,
    service: str = "pdl",
    keychain: str | None = None,
) -> bool:
    """Return True if a token exists for account in service."""
    return get_keychain_token(account, service, keychain) is not None


# ---------------------------------------------------------------------------
# Finder Metadata (com.apple.metadata:kMDItemWhereFroms via binary plist)
# ---------------------------------------------------------------------------

WHERE_FROMS_ATTR = "com.apple.metadata:kMDItemWhereFroms"


def set_where_froms(filepath: Path, urls: list[str]) -> bool:
    """Write source URLs into com.apple.metadata:kMDItemWhereFroms binary plist attribute."""
    if not is_effective_macos():
        return False
    p = Path(filepath).resolve()
    if not p.exists() or not urls:
        return False

    xattr_bin = shutil.which("xattr") or (
        "/usr/bin/xattr" if os.path.exists("/usr/bin/xattr") else "xattr"
    )
    try:
        plist_bytes = plistlib.dumps(list(urls), fmt=plistlib.FMT_BINARY)
        hex_data = plist_bytes.hex()
        proc = subprocess.run(
            [xattr_bin, "-w", "-x", WHERE_FROMS_ATTR, hex_data, str(p)],
            capture_output=True,
            text=True,
            timeout=3.0,
            check=False,
        )
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("Failed to set kMDItemWhereFroms on %s: %s", p, exc)
        return False


def get_where_froms(filepath: Path) -> list[str]:
    """Read source URLs from com.apple.metadata:kMDItemWhereFroms binary plist attribute."""
    if not is_effective_macos():
        return []
    p = Path(filepath).resolve()
    if not p.exists():
        return []

    xattr_bin = shutil.which("xattr") or (
        "/usr/bin/xattr" if os.path.exists("/usr/bin/xattr") else "xattr"
    )
    try:
        proc = subprocess.run(
            [xattr_bin, "-p", "-x", WHERE_FROMS_ATTR, str(p)],
            capture_output=True,
            text=True,
            timeout=3.0,
            check=False,
        )
        if proc.returncode != 0 or not proc.stdout.strip():
            return []
        raw_hex = "".join(proc.stdout.split())
        raw_bytes = bytes.fromhex(raw_hex)
        data = plistlib.loads(raw_bytes)
        if isinstance(data, list):
            return [str(x) for x in data]
        if isinstance(data, str):
            return [data]
        return []
    except (OSError, subprocess.SubprocessError, ValueError, plistlib.InvalidFileException) as exc:
        logger.debug("Failed to read kMDItemWhereFroms from %s: %s", p, exc)
        return []


# ---------------------------------------------------------------------------
# Codesign & Gatekeeper Verification (codesign, spctl, quarantine)
# ---------------------------------------------------------------------------

QUARANTINE_ATTR = "com.apple.quarantine"


def codesign_verify(filepath: Path, timeout: float = 5.0) -> tuple[bool, str]:
    """Run `codesign --verify --verbose <filepath>` and return (is_valid, details)."""
    if not is_effective_macos():
        return True, "valid (no-op)"
    p = Path(filepath).resolve()
    if not p.exists():
        return False, f"File not found: {p}"

    codesign_bin = shutil.which("codesign") or (
        "/usr/bin/codesign" if os.path.exists("/usr/bin/codesign") else "codesign"
    )
    try:
        proc = subprocess.run(
            [codesign_bin, "--verify", "--verbose", str(p)],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        out = (proc.stdout or proc.stderr or "").strip()
        is_valid = proc.returncode == 0
        detail = out or ("valid" if is_valid else "invalid or unsigned")
        return is_valid, detail
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("codesign --verify failed: %s", exc)
        return False, str(exc)


def spctl_assess(filepath: Path, timeout: float = 5.0) -> tuple[bool, str]:
    """Run `spctl --assess --type execute --verbose <filepath>` and return (is_accepted, details)."""
    if not is_effective_macos():
        return True, "accepted (no-op)"
    p = Path(filepath).resolve()
    if not p.exists():
        return False, f"File not found: {p}"

    spctl_bin = shutil.which("spctl") or (
        "/usr/sbin/spctl" if os.path.exists("/usr/sbin/spctl") else "spctl"
    )
    try:
        proc = subprocess.run(
            [spctl_bin, "--assess", "--type", "execute", "--verbose", str(p)],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        out = (proc.stdout or proc.stderr or "").strip()
        is_accepted = proc.returncode == 0
        detail = out or ("accepted" if is_accepted else "rejected")
        return is_accepted, detail
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("spctl --assess failed: %s", exc)
        return False, str(exc)


def codesign_adhoc_sign(filepath: Path, timeout: float = 5.0) -> tuple[bool, str]:
    """Ad-hoc sign binary with `codesign --force --sign - <filepath>`."""
    if not is_effective_macos():
        return True, "signed (no-op)"
    p = Path(filepath).resolve()
    if not p.exists():
        return False, f"File not found: {p}"

    codesign_bin = shutil.which("codesign") or (
        "/usr/bin/codesign" if os.path.exists("/usr/bin/codesign") else "codesign"
    )
    try:
        proc = subprocess.run(
            [codesign_bin, "--force", "--sign", "-", str(p)],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        out = (proc.stdout or proc.stderr or "").strip()
        is_ok = proc.returncode == 0
        return is_ok, out or ("signed" if is_ok else "signing failed")
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("codesign ad-hoc sign failed: %s", exc)
        return False, str(exc)


def remove_quarantine(filepath: Path) -> bool:
    """Remove com.apple.quarantine attribute if present."""
    if not is_effective_macos():
        return True
    p = Path(filepath).resolve()
    if not p.exists():
        return False

    xattr_bin = shutil.which("xattr") or (
        "/usr/bin/xattr" if os.path.exists("/usr/bin/xattr") else "xattr"
    )
    try:
        subprocess.run(
            [xattr_bin, "-d", QUARANTINE_ATTR, str(p)],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
        )
        return True
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("remove_quarantine failed on %s: %s", p, exc)
        return False


def has_quarantine(filepath: Path) -> bool:
    """Check if com.apple.quarantine attribute is present."""
    if not is_effective_macos():
        return False
    p = Path(filepath).resolve()
    if not p.exists():
        return False

    xattr_bin = shutil.which("xattr") or (
        "/usr/bin/xattr" if os.path.exists("/usr/bin/xattr") else "xattr"
    )
    try:
        proc = subprocess.run(
            [xattr_bin, "-p", QUARANTINE_ATTR, str(p)],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
        )
        return proc.returncode == 0 and bool(proc.stdout.strip())
    except (OSError, subprocess.SubprocessError):
        return False


# ---------------------------------------------------------------------------
# Native macOS Notifications (osascript display notification)
# ---------------------------------------------------------------------------


def display_notification(
    title: str,
    message: str,
    subtitle: str | None = None,
    timeout: float = 3.0,
) -> bool:
    """Display a native macOS user notification via osascript."""
    if not is_effective_macos():
        return False

    osascript_bin = shutil.which("osascript") or (
        "/usr/bin/osascript" if os.path.exists("/usr/bin/osascript") else "osascript"
    )

    def _esc_as(text: str) -> str:
        return text.replace("\\", "\\\\").replace('"', '\\"')

    esc_msg = _esc_as(message)
    esc_title = _esc_as(title)
    if subtitle:
        esc_sub = _esc_as(subtitle)
        script = f'display notification "{esc_msg}" with title "{esc_title}" subtitle "{esc_sub}"'
    else:
        script = f'display notification "{esc_msg}" with title "{esc_title}"'

    try:
        proc = subprocess.run(
            [osascript_bin, "-e", script],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        return proc.returncode == 0
    except (OSError, subprocess.SubprocessError) as exc:
        logger.debug("osascript notification failed: %s", exc)
        return False


# ---------------------------------------------------------------------------
# macOS Utilities Availability Check
# ---------------------------------------------------------------------------


def check_macos_utilities() -> dict[str, dict[str, Any]]:
    """Check availability of caffeinate, scutil, pmset, security, codesign, spctl, xattr."""
    utils = {
        "caffeinate": {
            "path": shutil.which("caffeinate")
            or ("/usr/bin/caffeinate" if os.path.exists("/usr/bin/caffeinate") else None),
            "fix": "Built-in macOS binary (/usr/bin/caffeinate)",
        },
        "scutil": {
            "path": shutil.which("scutil")
            or ("/usr/sbin/scutil" if os.path.exists("/usr/sbin/scutil") else None),
            "fix": "Built-in macOS binary (/usr/sbin/scutil)",
        },
        "pmset": {
            "path": shutil.which("pmset")
            or ("/usr/bin/pmset" if os.path.exists("/usr/bin/pmset") else None),
            "fix": "Built-in macOS binary (/usr/bin/pmset)",
        },
        "security": {
            "path": shutil.which("security")
            or ("/usr/bin/security" if os.path.exists("/usr/bin/security") else None),
            "fix": "Built-in macOS binary (/usr/bin/security)",
        },
        "codesign": {
            "path": shutil.which("codesign")
            or ("/usr/bin/codesign" if os.path.exists("/usr/bin/codesign") else None),
            "fix": "Run 'xcode-select --install' to install macOS developer tools",
        },
        "spctl": {
            "path": shutil.which("spctl")
            or ("/usr/sbin/spctl" if os.path.exists("/usr/sbin/spctl") else None),
            "fix": "Built-in macOS binary (/usr/sbin/spctl)",
        },
        "xattr": {
            "path": shutil.which("xattr")
            or ("/usr/bin/xattr" if os.path.exists("/usr/bin/xattr") else None),
            "fix": "Built-in macOS binary (/usr/bin/xattr)",
        },
    }

    result = {}
    for name, info in utils.items():
        found = bool(info["path"])
        result[name] = {
            "available": found,
            "path": str(info["path"]) if found else None,
            "fix": info["fix"],
        }
    return result
