"""Aria2 JSON-RPC 2.0 multi-channel download engine with Watchdog v1/v2 and Resilience Core supervision."""

from __future__ import annotations

import atexit
import base64
import contextlib
import logging
import os
import re
import secrets
import signal
import socket
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from paradl import platform
from paradl.clock import get_clock
from paradl.engines.watchdog import (
    WatchdogActionType,
    WatchdogController,
    compute_percentile,
)
from paradl.events import (
    ResilienceEventSource,
    check_free_disk_space,
    check_power_policy,
)
from paradl.history import HistoryManager
from paradl.mirrors import MetalinkCompanion, MirrorManager
from paradl.models import (
    DownloadItem,
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
    format_rate_for_aria2,
    format_speed,
    parse_rate,
)
from paradl.net import probe_range_support, resolve_dns
from paradl.postproc.checksum import ChecksumEngine
from paradl.queue import check_when_fast_gate
from paradl.toolchain import TOOLCHAIN
from paradl.ui import UI, LiveTransferDisplay, print_plain

logger = logging.getLogger(__name__)

_ACTIVE_PROCESSES: set[subprocess.Popen[Any]] = set()
_ACTIVE_TEMP_FILES: set[Path] = set()


def register_temp_path(path: str | Path) -> None:
    _ACTIVE_TEMP_FILES.add(Path(path))


def unregister_temp_path(path: str | Path) -> None:
    _ACTIVE_TEMP_FILES.discard(Path(path))


def cleanup_children_and_temps(*, remove_partial_raw: bool = False) -> None:
    """Terminate any running child processes and remove secret/temporary files."""
    for proc in list(_ACTIVE_PROCESSES):
        try:
            if proc.poll() is None:
                proc.terminate()
                try:
                    proc.wait(timeout=2.0)
                except subprocess.TimeoutExpired:
                    proc.kill()
        except OSError as exc:
            logger.debug("Error terminating child process: %s", exc)
        _ACTIVE_PROCESSES.discard(proc)

    for p in list(_ACTIVE_TEMP_FILES):
        try:
            if p.is_dir():
                import shutil

                shutil.rmtree(p, ignore_errors=True)
            elif p.exists():
                p.unlink()
        except OSError as exc:
            logger.debug("Error removing temp path %s: %s", p, exc)
        if remove_partial_raw:
            aria_ctrl = Path(f"{p}.aria2")
            if aria_ctrl.exists():
                with contextlib.suppress(OSError):
                    aria_ctrl.unlink()
        _ACTIVE_TEMP_FILES.discard(p)


atexit.register(cleanup_children_and_temps)


# ---------------------------------------------------------------------------
# Persistent aria2c Daemon Pool
# ---------------------------------------------------------------------------
# Reuses aria2c JSON-RPC daemon instances across transfers to eliminate
# per-download process startup overhead. Daemons are keyed by (ip_family, http3)
# and shut down after an idle timeout.


class _DaemonPoolEntry:
    """Tracks a pooled aria2c daemon with last-used timestamp and in-use flag."""

    def __init__(self, daemon: Aria2RpcDaemon) -> None:
        self.daemon = daemon
        self.last_used = time.monotonic()
        self.in_use = False


class Aria2DaemonPool:
    """Thread-safe pool of reusable aria2c JSON-RPC daemon instances.

    Daemons are keyed by (ip_family, http3). Acquiring a daemon either returns
    an existing idle one or starts a new one. Releasing returns it to the pool
    without stopping the process. Idle daemons are shut down after
    ``idle_timeout_sec`` seconds.
    """

    def __init__(self, idle_timeout_sec: float = 60.0) -> None:
        self.idle_timeout_sec = idle_timeout_sec
        self._entries: dict[tuple[str, bool], _DaemonPoolEntry] = {}
        self._lock = threading.Lock()
        self._started = False

    def _startup(self) -> None:
        if not self._started:
            atexit.register(self.shutdown_all)
            self._started = True

    def acquire(
        self,
        *,
        aria2c_bin: str,
        max_concurrent_downloads: int,
        ip_family: str,
        limit_rate: str | None,
        check_integrity: bool,
        http3: bool,
    ) -> Aria2RpcDaemon:
        """Acquire a daemon from the pool, starting a new one if needed."""
        self._startup()
        key = (ip_family, http3)
        with self._lock:
            entry = self._entries.get(key)
            if (
                entry is not None
                and entry.daemon._proc is not None
                and entry.daemon._proc.poll() is None
            ):
                # Reuse existing healthy daemon
                entry.in_use = True
                entry.last_used = time.monotonic()
                return entry.daemon

            # Stop any dead/stale daemon for this key
            if entry is not None:
                with contextlib.suppress(Exception):
                    entry.daemon.stop(graceful=False)
                del self._entries[key]

            # Start a new daemon
            daemon = Aria2RpcDaemon(
                aria2c_bin=aria2c_bin,
                max_concurrent_downloads=max_concurrent_downloads,
                ip_family=ip_family,
                limit_rate=limit_rate,
                check_integrity=check_integrity,
                http3=http3,
            )
            daemon.start()
            entry = _DaemonPoolEntry(daemon)
            entry.in_use = True
            entry.last_used = time.monotonic()
            self._entries[key] = entry
            return daemon

    def release(self, daemon: Aria2RpcDaemon) -> None:
        """Return a daemon to the pool without stopping it."""
        with self._lock:
            for entry in self._entries.values():
                if entry.daemon is daemon:
                    entry.in_use = False
                    entry.last_used = time.monotonic()
                    break

    def cleanup_idle(self) -> None:
        """Shut down daemons that have been idle beyond the timeout."""
        now = time.monotonic()
        with self._lock:
            for key, entry in list(self._entries.items()):
                if not entry.in_use and (now - entry.last_used) > self.idle_timeout_sec:
                    with contextlib.suppress(Exception):
                        entry.daemon.stop(graceful=False)
                    del self._entries[key]

    def shutdown_all(self) -> None:
        """Stop all pooled daemons (called at exit)."""
        with self._lock:
            for entry in list(self._entries.values()):
                with contextlib.suppress(Exception):
                    entry.daemon.stop(graceful=False)
            self._entries.clear()


_DAEMON_POOL: Aria2DaemonPool | None = None


def get_daemon_pool() -> Aria2DaemonPool:
    """Return the module-level daemon pool, creating it on first use."""
    global _DAEMON_POOL  # noqa: PLW0603
    if _DAEMON_POOL is None:
        _DAEMON_POOL = Aria2DaemonPool()
    return _DAEMON_POOL


def shutdown_daemon_pool() -> None:
    """Shut down all pooled daemons."""
    global _DAEMON_POOL  # noqa: PLW0603
    if _DAEMON_POOL is not None:
        _DAEMON_POOL.shutdown_all()
        _DAEMON_POOL = None


atexit.register(shutdown_daemon_pool)


def _pick_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


class Aria2RpcDaemon:
    """Manages an isolated aria2c JSON-RPC daemon instance bound to 127.0.0.1."""

    def __init__(
        self,
        *,
        aria2c_bin: str,
        max_concurrent_downloads: int = 1,
        ip_family: str = "auto",
        limit_rate: str | None = None,
        check_integrity: bool = False,
        http3: bool | None = None,
        proxy: str | None = None,
    ) -> None:
        self.aria2c_bin = aria2c_bin
        self.port = _pick_free_port()
        self.secret = secrets.token_hex(16)
        UI.register_secret(self.secret)
        self.max_concurrent_downloads = max(1, max_concurrent_downloads)
        self.ip_family = ip_family
        self.limit_rate = format_rate_for_aria2(limit_rate)
        self.check_integrity = check_integrity
        # http3=None means auto-detect based on aria2c version (>= 1.36.0 supports QUIC).
        self.http3 = self._aria2c_supports_http3() if http3 is None else http3
        self.proxy = proxy
        self._proc: subprocess.Popen[Any] | None = None
        self._conf_path: Path | None = None
        self._http_client: Any = None

    @staticmethod
    def _aria2c_supports_http3(aria2c_bin: str | None = None) -> bool:
        """Return True if `aria2c` supports HTTP/3 (QUIC) — introduced in aria2 1.36.0."""
        from paradl.toolchain import TOOLCHAIN

        bin_path = aria2c_bin or TOOLCHAIN.aria2c
        if not bin_path:
            return False
        try:
            proc = subprocess.run(
                [bin_path, "--version"],
                capture_output=True,
                text=True,
                timeout=3.0,
                check=False,
            )
            out = (proc.stdout or "") + "\n" + (proc.stderr or "")
            m = re.search(r"aria2 version\s+([0-9]+(?:\.[0-9]+)*)", out)
            if m:
                ver = tuple(int(p) for p in m.group(1).split(".")[:3])
                return ver >= (1, 36, 0)
        except Exception:
            pass
        return False

    def start(self) -> None:
        import httpx

        fd, tmp_conf = tempfile.mkstemp(prefix="pdl_aria2_", suffix=".conf")
        os.chmod(tmp_conf, 0o600)
        self._conf_path = Path(tmp_conf)
        register_temp_path(self._conf_path)

        conf_lines = [
            "enable-rpc=true",
            "rpc-listen-all=false",
            f"rpc-listen-port={self.port}",
            f"rpc-secret={self.secret}",
            f"stop-with-process={os.getpid()}",
            "summary-interval=0",
            "console-log-level=error",
            "quiet=true",
            "disk-cache=64M",
            "file-allocation=none",
            "continue=true",
            "allow-overwrite=true",
            "auto-file-renaming=false",
            "conditional-get=true",
            "remote-time=true",
            "max-tries=5",
            "retry-wait=1",
            "connect-timeout=15",
            "timeout=30",
        ]
        if self.max_concurrent_downloads > 1:
            conf_lines.append(f"max-concurrent-downloads={self.max_concurrent_downloads}")
        if self.ip_family == "ipv4":
            conf_lines.append("disable-ipv6=true")
        if self.limit_rate:
            conf_lines.append(f"max-overall-download-limit={self.limit_rate}")
        if self.check_integrity:
            conf_lines.append("check-integrity=true")
            conf_lines.append("realtime-chunk-checksum=true")

        if self.http3:
            conf_lines.append("http3=true")

        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write("\n".join(conf_lines) + "\n")
            f.flush()

        # Pass --conf-path so user's ~/.aria2/aria2.conf is ignored
        # and the RPC secret is never exposed in argv.
        cmd = [
            self.aria2c_bin,
            f"--conf-path={self._conf_path}",
        ]
        self._proc = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        _ACTIVE_PROCESSES.add(self._proc)

        # M9: Continuously drain stderr in a background thread to prevent pipe buffer deadlocks
        self._stderr_lines: list[str] = []

        def _drain_stderr() -> None:
            if self._proc and self._proc.stderr:
                try:
                    for line in self._proc.stderr:
                        if len(self._stderr_lines) < 100:
                            self._stderr_lines.append(line)
                        else:
                            self._stderr_lines.pop(0)
                            self._stderr_lines.append(line)
                except Exception:
                    pass

        self._drain_thread = threading.Thread(target=_drain_stderr, daemon=True)
        self._drain_thread.start()

        self._http_client = httpx.Client(timeout=5.0)

        # Wait up to 3s for RPC server to accept requests
        deadline = time.time() + 3.0
        while time.time() < deadline:
            if self._proc.poll() is not None:
                self._drain_thread.join(timeout=0.2)
                stderr_out = "".join(self._stderr_lines).strip()
                raise RuntimeError(f"aria2c daemon exited early: {stderr_out}")
            try:
                self.call("aria2.getVersion")
                return
            except Exception:
                get_clock().sleep(0.05)
        raise RuntimeError("Timed out waiting for aria2c JSON-RPC daemon to start")

    def call(self, method: str, *params: Any) -> Any:
        if self._http_client is None:
            raise RuntimeError("RPC client is not initialized")
        rpc_params = [f"token:{self.secret}", *params]
        payload = {
            "jsonrpc": "2.0",
            "id": "pdl",
            "method": method,
            "params": rpc_params,
        }
        resp = self._http_client.post(f"http://127.0.0.1:{self.port}/jsonrpc", json=payload)
        data = resp.json()
        if "error" in data:
            raise RuntimeError(f"aria2 RPC error in {method}: {data['error']}")
        return data.get("result")

    def add_uri(self, uris: list[str], options: dict[str, Any]) -> str:
        return str(self.call("aria2.addUri", uris, options))

    def add_metalink(self, raw_metalink: bytes, options: dict[str, Any]) -> list[str]:
        b64 = base64.b64encode(raw_metalink).decode("ascii")
        res = self.call("aria2.addMetalink", b64, options)
        if isinstance(res, list):
            return [str(g) for g in res]
        return [str(res)] if res else []

    def tell_status(self, gid: str) -> dict[str, Any]:
        res = self.call("aria2.tellStatus", gid)
        return dict(res) if isinstance(res, dict) else {}

    def get_servers(self, gid: str) -> list[dict[str, Any]]:
        try:
            res = self.call("aria2.getServers", gid)
            return list(res) if isinstance(res, list) else []
        except RuntimeError:
            return []

    def get_global_stat(self) -> dict[str, Any]:
        res = self.call("aria2.getGlobalStat")
        return dict(res) if isinstance(res, dict) else {}

    def force_pause(self, gid: str) -> str:
        return str(self.call("aria2.forcePause", gid))

    def unpause(self, gid: str) -> str:
        return str(self.call("aria2.unpause", gid))

    def change_option(self, gid: str, options: dict[str, Any]) -> str:
        return str(self.call("aria2.changeOption", gid, options))

    def save_session(self) -> None:
        try:
            self.call("aria2.saveSession")
        except Exception as exc:
            logger.debug("saveSession ignored: %s", exc)

    def stop(self, *, graceful: bool = True) -> None:
        if self._http_client is not None:
            if graceful and self._proc and self._proc.poll() is None:
                with contextlib.suppress(Exception):
                    self.call("aria2.shutdown")
            with contextlib.suppress(Exception):
                self._http_client.close()
            self._http_client = None

        if self._proc is not None:
            try:
                if self._proc.poll() is None:
                    if graceful:
                        self._proc.send_signal(signal.SIGINT)
                    else:
                        self._proc.terminate()
                    try:
                        self._proc.wait(timeout=3.0)
                    except subprocess.TimeoutExpired:
                        self._proc.kill()
            except OSError as exc:
                logger.debug("Error stopping aria2c daemon: %s", exc)
            _ACTIVE_PROCESSES.discard(self._proc)
            self._proc = None

        if self._conf_path is not None:
            unregister_temp_path(self._conf_path)
            if self._conf_path.exists():
                with contextlib.suppress(OSError):
                    self._conf_path.unlink()
            self._conf_path = None

    def __enter__(self) -> Aria2RpcDaemon:
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop(graceful=True)


def _count_active_connections(servers_info: list[dict[str, Any]]) -> tuple[int, list[float]]:
    count = 0
    speeds: list[float] = []
    for srv_group in servers_info:
        for srv in srv_group.get("servers", []):
            count += 1
            spd = float(srv.get("downloadSpeed", 0) or 0)
            speeds.append(spd)
    return count, speeds


class Aria2Engine:
    """JSON-RPC Aria2 transport engine with Watchdog v1/v2 and Resilience Core support."""

    @staticmethod
    def parse_speed_str(speed_str: str | None) -> float:
        return parse_rate(speed_str)

    @classmethod
    def download(
        cls,
        target_info: TargetInfo,
        out_dir: Path,
        connections: int = 16,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V1,
        poll_interval: float = 0.2,
        min_cooldown_sec: float = 2.0,
        max_duration_sec: float | None = None,
        *,
        caffeinate: bool = True,
        net_watch: bool = True,
        stall_timeout: float = 30.0,
        require_ac: bool = False,
        pause_on_low_power: bool = False,
        write_xattr: bool = True,
        notify: bool = False,
        mirrors: list[str] | None = None,
        when_fast_bps: float = 0.0,
        when_fast_retest_interval_sec: float = 15.0,
        when_fast_prober: Callable[[str], float] | None = None,
        event_source: ResilienceEventSource | None = None,
        http3: bool = True,
        proxy: str | None = None,
        doh_endpoint: str | None = None,
        on_progress: Callable[[int, int | None, float, int], None] | None = None,
    ) -> TransferResult:
        merged_mirrors = list(target_info.mirrors)
        for m in mirrors or []:
            if m and m not in merged_mirrors:
                merged_mirrors.append(m)

        item = DownloadItem(
            url=target_info.clean_url,
            out_dir=out_dir,
            filename=target_info.filename_hint,
            headers=dict(target_info.headers),
            expected_length=target_info.content_length,
            expected_digest=target_info.expected_digest,
            etag_sha256=target_info.etag_sha256,
            mirrors=merged_mirrors,
            metalink_path=target_info.metalink_path,
            cookie_file=target_info.cookie_file,
            referer=target_info.referer,
            user_agent=target_info.user_agent,
        )
        return cls.download_batch(
            items=[item],
            out_dir=out_dir,
            connections=connections,
            limit_rate=limit_rate,
            anti_throttle=anti_throttle,
            grace_period=grace_period,
            watchdog_mode=watchdog_mode,
            accept_ranges_hint=target_info.accept_ranges,
            is_torrent=(target_info.target_type == TargetType.TORRENT),
            poll_interval=poll_interval,
            min_cooldown_sec=min_cooldown_sec,
            max_duration_sec=max_duration_sec,
            caffeinate=caffeinate,
            net_watch=net_watch,
            stall_timeout=stall_timeout,
            require_ac=require_ac,
            pause_on_low_power=pause_on_low_power,
            write_xattr=write_xattr,
            notify=notify,
            when_fast_bps=when_fast_bps,
            when_fast_retest_interval_sec=when_fast_retest_interval_sec,
            when_fast_prober=when_fast_prober,
            event_source=event_source,
            http3=http3,
            proxy=proxy,
            doh_endpoint=doh_endpoint,
            on_progress=on_progress,
        )

    @classmethod
    def download_batch(
        cls,
        items: list[DownloadItem],
        out_dir: Path,
        connections: int = 16,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V1,
        accept_ranges_hint: bool | None = None,
        is_torrent: bool = False,
        poll_interval: float = 0.2,
        min_cooldown_sec: float = 2.0,
        max_duration_sec: float | None = None,
        *,
        caffeinate: bool = True,
        net_watch: bool = True,
        stall_timeout: float = 30.0,
        require_ac: bool = False,
        pause_on_low_power: bool = False,
        write_xattr: bool = True,
        notify: bool = False,
        when_fast_bps: float = 0.0,
        when_fast_retest_interval_sec: float = 15.0,
        when_fast_prober: Callable[[str], float] | None = None,
        event_source: ResilienceEventSource | None = None,
        http3: bool = True,
        proxy: str | None = None,
        doh_endpoint: str | None = None,
        on_progress: Callable[[int, int | None, float, int], None] | None = None,
    ) -> TransferResult:
        aria2c_bin = TOOLCHAIN.aria2c
        if not aria2c_bin:
            msg = "aria2c binary not found. Run 'brew install aria2'."
            print_plain(f"  [error]Error: {msg}[/error]", force=True)
            return TransferResult(
                success=False, error=msg, exit_code=int(ExitCode.MISSING_DEPENDENCY)
            )

        if not items:
            return TransferResult(success=True, file_path=out_dir)

        # A2 Start-up guard 1: Power policy check (--require-ac / --pause-on-low-power)
        pwr_ok, pwr_err = check_power_policy(
            require_ac=require_ac,
            pause_on_low_power=pause_on_low_power,
        )
        if not pwr_ok:
            err_msg = pwr_err or "Power policy check failed."
            print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
            UI.emit_event("error", reason="power_policy", message=err_msg)
            return TransferResult(success=False, error=err_msg, exit_code=int(ExitCode.FAILURE))

        out_dir.mkdir(parents=True, exist_ok=True)
        safe_conn = min(max(int(connections), 1), 16)
        wd_mode = (
            WatchdogMode(watchdog_mode.lower()) if isinstance(watchdog_mode, str) else watchdog_mode
        )
        threshold_bps = parse_rate(anti_throttle) if anti_throttle else 0.0
        if threshold_bps <= 0:
            wd_mode = WatchdogMode.OFF

        # B1: Mirror & Metalink resolution + pre-flight size confirmation
        item_metalinks: dict[int, MetalinkCompanion] = {}
        supports_range = True
        for idx_it, it in enumerate(items):
            if is_torrent:
                continue
            m_res = MirrorManager.resolve_and_validate(
                it.url,
                item_mirrors=it.mirrors,
                expected_length=it.expected_length,
                expected_digest=it.expected_digest,
                filename_hint=it.filename,
                accept_ranges_hint=accept_ranges_hint if idx_it == 0 else None,
                headers=it.headers,
                out_dir=it.out_dir,
                explicit_metalink=it.metalink_path,
            )
            if m_res.metalink is not None:
                item_metalinks[idx_it] = m_res.metalink
            if m_res.active_uris:
                it.url = m_res.active_uris[0]
                it.mirrors = m_res.active_uris[1:]
            else:
                it.mirrors = []
            if it.expected_length is None and m_res.expected_length is not None:
                it.expected_length = m_res.expected_length
            if not it.expected_digest and m_res.expected_digest:
                it.expected_digest = m_res.expected_digest
            if not it.filename and m_res.filename:
                it.filename = m_res.filename
            if idx_it == 0:
                supports_range = m_res.accept_ranges

        first_url = items[0].url
        parsed_host = (urlparse(first_url).hostname or "").lower()

        # Phase 3 #1: Up-front Range support detection for HTTP/HTTPS transfers if not already probed
        if (
            not is_torrent
            and first_url.startswith(("http://", "https://"))
            and accept_ranges_hint is None
            and not items[0].mirrors
            and 0 not in item_metalinks
        ):
            probe_res = probe_range_support(first_url, headers=items[0].headers)
            if probe_res.ok:
                supports_range = probe_res.accept_ranges
                if len(items) == 1:
                    if items[0].expected_length is None and probe_res.content_length is not None:
                        items[0].expected_length = probe_res.content_length
                    if not items[0].filename and probe_res.filename:
                        items[0].filename = probe_res.filename
                    if not items[0].etag_sha256 and probe_res.etag_sha256:
                        items[0].etag_sha256 = probe_res.etag_sha256
                        if not items[0].expected_digest:
                            items[0].expected_digest = f"sha256:{probe_res.etag_sha256}"

        # B3: --when-fast pre-flight short speed test gate
        if when_fast_bps > 0 and first_url.startswith(("http://", "https://")):
            fast_ok, measured_bps, fast_err = check_when_fast_gate(
                first_url,
                when_fast_bps,
                headers=items[0].headers,
                speed_prober=when_fast_prober,
            )
            if not fast_ok:
                err_msg = (
                    fast_err or f"Deferred by --when-fast (measured {format_speed(measured_bps)})"
                )
                print_plain(f"  [warning]⏸ {err_msg}[/warning]", force=True)
                UI.emit_event(
                    "error",
                    reason="when_fast",
                    measured_bps=round(measured_bps, 2),
                    threshold_bps=round(when_fast_bps, 2),
                    message=err_msg,
                )
                return TransferResult(success=False, error=err_msg, exit_code=int(ExitCode.FAILURE))

        # A2 Start-up guard 2: Free-disk check against expected size + safety margin
        total_expected = sum(it.expected_length or 0 for it in items)
        existing_bytes = 0
        for it in items:
            if it.filename:
                part_file = it.out_dir / it.filename
                if part_file.is_file():
                    with contextlib.suppress(OSError):
                        existing_bytes += part_file.stat().st_size
        disk_ok, disk_err = check_free_disk_space(
            out_dir,
            total_expected if total_expected > 0 else None,
            existing_bytes=existing_bytes,
        )
        if not disk_ok:
            err_msg = disk_err or "Insufficient free disk space."
            print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
            UI.emit_event("error", reason="insufficient_disk_space", message=err_msg)
            return TransferResult(success=False, error=err_msg, exit_code=int(ExitCode.FAILURE))

        if not supports_range:
            print_plain(
                "  [yellow]• Server does not honor HTTP Range (206 Partial Content); "
                "using single stream — anti-throttle watchdog remains active.[/yellow]"
            )
            UI.emit_event(
                "watchdog_action",
                action="single_stream",
                reason="Server returned 200 instead of 206 on Range probe",
                supports_range=False,
            )
            safe_conn = 1

        watchdog = WatchdogController(
            mode=wd_mode,
            user_floor_bps=threshold_bps,
            grace_period=grace_period,
            connections=safe_conn,
            host=parsed_host,
            supports_range=supports_range,
            min_cooldown_sec=min_cooldown_sec,
        )
        safe_conn = watchdog.connections

        if watchdog.is_active:
            print_plain(
                f"  [dim]• Anti-Throttle Watchdog ({wd_mode.value}):[/dim] "
                f"[yellow]Active (< {format_speed(threshold_bps)} floor)[/yellow]"
            )

        max_conc = min(len(items), 4)
        t_start = time.time()
        all_speed_samples: list[float] = []
        completed_items: set[int] = set()
        completed_paths: dict[int, Path] = {}

        # Adaptive polling state: faster when volatile, slower when stable
        _poll_interval = poll_interval
        _last_speed: float | None = None
        _stable_ticks = 0

        title_label = (
            items[0].filename
            or os.path.basename(first_url.split("?", 1)[0])
            or f"{len(items)} file(s)"
        )

        ev_src = event_source or ResilienceEventSource(
            net_watch=net_watch,
            stall_timeout_sec=stall_timeout,
        )

        with (
            platform.get_sleep_assertion(enabled=caffeinate),
            ev_src,
            LiveTransferDisplay(title=title_label, total_bytes=total_expected) as live,
        ):
            while len(completed_items) < len(items):
                remaining_indices = [i for i in range(len(items)) if i not in completed_items]
                curr_conc = min(len(remaining_indices), max_conc)

                # --- Adaptive concurrency based on per-host RTT/loss ---
                # Read learned host strategy (RTT, loss) to tune connection count.
                # High RTT or loss reduces concurrent flows to avoid head-of-line blocking
                # and socket contention; clean paths can use the full budget.
                parsed_host = (urlparse(first_url).hostname or "").lower()
                host_strategy = (
                    HistoryManager.get_host_strategy(parsed_host) if parsed_host else None
                )
                avg_rtt_ms = host_strategy.avg_rtt_ms if host_strategy else 0.0
                pkt_loss_pct = host_strategy.packet_loss_pct if host_strategy else 0.0

                # Start from watchdog connection budget, then adapt:
                #   RTT > 100ms  -> scale down toward 4 conns (high latency)
                #   Loss > 5%    -> scale down toward 4 conns (unstable path)
                #   RTT < 50ms & Loss < 1% -> keep full budget (clean path)
                budget = watchdog.connections
                if supports_range and budget > 1:
                    rtt_factor = 1.0
                    if avg_rtt_ms > 100.0:
                        rtt_factor = max(0.25, 1.0 - (avg_rtt_ms - 100.0) / 500.0)
                    loss_factor = 1.0
                    if pkt_loss_pct > 5.0:
                        loss_factor = max(0.25, 1.0 - (pkt_loss_pct - 5.0) / 20.0)
                    adaptive_factor = min(rtt_factor, loss_factor)
                    budget = max(4, int(budget * adaptive_factor))

                # Global connection budget across concurrent files capped at safe_conn (<= 16)
                per_file_conn = max(1, budget // max(curr_conc, 1))

                # Acquire a pooled daemon (reuses existing process instead of spawning a new one)
                daemon = get_daemon_pool().acquire(
                    aria2c_bin=aria2c_bin,
                    max_concurrent_downloads=max_conc,
                    ip_family=watchdog.ip_family,
                    limit_rate=limit_rate,
                    check_integrity=bool(item_metalinks),
                    http3=http3,
                )
                gid_to_idx: dict[str, int] = {}
                try:
                    for idx in remaining_indices:
                        it = items[idx]
                        it.out_dir.mkdir(parents=True, exist_ok=True)
                        opts: dict[str, Any] = {
                            "dir": str(it.out_dir),
                            "split": str(per_file_conn),
                            "max-connection-per-server": str(per_file_conn),
                            "min-split-size": "1M",
                            "continue": "true" if supports_range else "false",
                            "allow-overwrite": "true",
                            "auto-file-renaming": "false",
                        }
                        if it.filename:
                            opts["out"] = it.filename
                        if it.headers:
                            opts["header"] = [f"{k}: {v}" for k, v in it.headers.items()]
                        if it.cookie_file:
                            opts["load-cookies"] = str(it.cookie_file)
                        if it.referer:
                            opts["referer"] = it.referer
                        if it.user_agent:
                            opts["user-agent"] = it.user_agent
                        if proxy:
                            opts["all-proxy"] = proxy
                        if len(it.mirrors) > 0:
                            opts["uri-selector"] = "adaptive"
                        if (
                            watchdog.mode == WatchdogMode.V2
                            and watchdog.is_active
                            and per_file_conn >= 1
                        ):
                            # Tier 1: native lowest-speed-limit for dead-slow individual flows
                            lowest_lim = max(int(threshold_bps / (per_file_conn * 8)), 4096)
                            opts["lowest-speed-limit"] = str(lowest_lim)

                        if idx in item_metalinks:
                            opts["check-integrity"] = "true"
                            opts["uri-selector"] = "adaptive"
                            gids = daemon.add_metalink(item_metalinks[idx].raw_bytes, opts)
                            if gids:
                                gid_to_idx[gids[0]] = idx
                            else:
                                uris = [it.url, *it.mirrors]
                                gid = daemon.add_uri(uris, opts)
                                gid_to_idx[gid] = idx
                        else:
                            uris = [it.url, *it.mirrors]
                            gid = daemon.add_uri(uris, opts)
                            gid_to_idx[gid] = idx

                    restart_daemon_needed = False
                    last_watch_step_ts = 0.0
                    last_when_fast_check_ts = time.time()

                    while gid_to_idx and not restart_daemon_needed:
                        now = time.time()

                        gstat = daemon.get_global_stat()
                        global_speed = float(gstat.get("downloadSpeed", 0) or 0)
                        if global_speed > 0:
                            all_speed_samples.append(global_speed)

                        total_done = 0
                        total_len = 0
                        total_conns = 0
                        conn_speeds: list[float] = []
                        failed_error: str | None = None

                        for gid in list(gid_to_idx.keys()):
                            idx = gid_to_idx[gid]
                            st = daemon.tell_status(gid)
                            status_str = st.get("status", "")
                            done_b = int(st.get("completedLength", 0) or 0)
                            len_b = int(st.get("totalLength", 0) or 0)
                            total_done += done_b
                            total_len += len_b or (items[idx].expected_length or 0)

                            if status_str == "active":
                                srvs = daemon.get_servers(gid)
                                c_cnt, c_spds = _count_active_connections(srvs)
                                total_conns += c_cnt
                                conn_speeds.extend(c_spds)
                            elif status_str == "complete":
                                files_meta = st.get("files", [])
                                if files_meta and files_meta[0].get("path"):
                                    actual_p = Path(files_meta[0]["path"])
                                    completed_paths[idx] = actual_p
                                    if not items[idx].filename:
                                        items[idx].filename = actual_p.name
                                if len_b > 0 and items[idx].expected_length is None:
                                    items[idx].expected_length = len_b
                                completed_items.add(idx)
                                del gid_to_idx[gid]
                            elif status_str == "error":
                                err_code = st.get("errorCode", "unknown")
                                err_msg = st.get("errorMessage", "Transfer error")
                                # B1: If mirrors are available, record failure on current URI and fail over
                                if items[idx].mirrors:
                                    bad_uri = items[idx].url
                                    MirrorManager.record_failure(bad_uri)
                                    items[idx].url = items[idx].mirrors[0]
                                    items[idx].mirrors = items[idx].mirrors[1:]
                                    del gid_to_idx[gid]
                                    restart_daemon_needed = True
                                    break
                                failed_error = f"aria2 error ({err_code}): {err_msg}"
                                del gid_to_idx[gid]
                                break

                        if restart_daemon_needed:
                            break

                        if failed_error:
                            return TransferResult(
                                success=False,
                                duration=time.time() - t_start,
                                recycle_count=watchdog.cycle_count,
                                error=failed_error,
                                exit_code=int(ExitCode.FAILURE),
                            )

                        # Collect per-file progress across all active GIDs
                        all_file_progress: list[dict[str, Any]] = []
                        all_conn_speeds: list[float] = []
                        for gid in list(gid_to_idx.keys()):
                            idx = gid_to_idx[gid]
                            st = daemon.tell_status(gid)
                            # Per-file progress from aria2 'files' array
                            for f in st.get("files", []) or []:
                                f_path = f.get("path", "")
                                f_name = os.path.basename(f_path) if f_path else f"file_{idx}"
                                f_done = int(f.get("completedLength", 0) or 0)
                                f_len = int(f.get("length", 0) or 0)
                                all_file_progress.append(
                                    {
                                        "name": f_name,
                                        "downloaded": f_done,
                                        "total": f_len if f_len > 0 else None,
                                    }
                                )
                            # Per-connection speeds from get_servers
                            try:
                                srvs = daemon.get_servers(gid)
                                for srv_group in srvs:
                                    for srv in srv_group.get("servers", []):
                                        spd = float(srv.get("downloadSpeed", 0) or 0)
                                        if spd > 0:
                                            all_conn_speeds.append(spd)
                            except Exception:
                                pass

                        live.update(
                            downloaded_bytes=total_done,
                            total_bytes=total_len,
                            speed_bps=global_speed,
                            connections=total_conns or per_file_conn,
                            watchdog_state=watchdog.status_label(),
                            file_progress=all_file_progress if all_file_progress else None,
                            connection_speeds=all_conn_speeds if all_conn_speeds else None,
                        )

                        # Report progress to an optional caller-supplied callback
                        # (used by the REST API server to surface live task progress).
                        if on_progress is not None:
                            try:
                                on_progress(
                                    total_done,
                                    total_len if total_len > 0 else None,
                                    global_speed,
                                    total_conns or per_file_conn,
                                )
                            except Exception:
                                # Never let a callback exception break a download.
                                logger.debug("on_progress callback raised", exc_info=True)

                        if not gid_to_idx:
                            break

                        # B3: Periodic --when-fast re-test during active download
                        if (
                            when_fast_bps > 0
                            and when_fast_retest_interval_sec > 0
                            and (now - last_when_fast_check_ts) >= when_fast_retest_interval_sec
                        ):
                            last_when_fast_check_ts = now
                            fast_ok, measured_bps, fast_err = check_when_fast_gate(
                                items[0].url,
                                when_fast_bps,
                                headers=items[0].headers,
                                speed_prober=when_fast_prober,
                            )
                            if not fast_ok:
                                err_msg = (
                                    fast_err
                                    or f"Deferred by --when-fast (measured {format_speed(measured_bps)})"
                                )
                                print_plain(f"\n  [warning]⏸ {err_msg}[/warning]", force=True)
                                return TransferResult(
                                    success=False,
                                    bytes_downloaded=total_done,
                                    duration=time.time() - t_start,
                                    recycle_count=watchdog.cycle_count,
                                    error=err_msg,
                                    exit_code=int(ExitCode.FAILURE),
                                )

                        # Mid-transfer power policy check (--pause-on-low-power / --require-ac)
                        if require_ac or pause_on_low_power:
                            pwr_state = platform.get_power_status()
                            if require_ac and not pwr_state.on_ac_power:
                                err_msg = (
                                    f"Power policy guard aborted transfer (--require-ac): "
                                    f"switched to {pwr_state.raw_source}."
                                )
                                print_plain(f"\n  [error]✗ {err_msg}[/error]", force=True)
                                UI.emit_event("error", reason="power_policy", message=err_msg)
                                return TransferResult(
                                    success=False,
                                    bytes_downloaded=total_done,
                                    duration=time.time() - t_start,
                                    recycle_count=watchdog.cycle_count,
                                    error=err_msg,
                                    exit_code=int(ExitCode.FAILURE),
                                )
                            if pause_on_low_power and pwr_state.low_power_mode:
                                UI.emit_event(
                                    "watchdog_action",
                                    action="paused_low_power",
                                    reason="Paused download due to Low Power / Battery Saver mode (--pause-on-low-power)",
                                )
                                for active_gid in list(gid_to_idx.keys()):
                                    with contextlib.suppress(Exception):
                                        daemon.force_pause(active_gid)
                                get_clock().sleep(min(poll_interval, 0.1))
                                pwr_state_after = platform.get_power_status()
                                if not pwr_state_after.low_power_mode:
                                    for active_gid in list(gid_to_idx.keys()):
                                        with contextlib.suppress(Exception):
                                            daemon.unpause(active_gid)
                                    ev_src.reset_progress_timer(total_done)
                                else:
                                    err_msg = "Transfer paused (--pause-on-low-power): macOS Low Power Mode is active."
                                    print_plain(f"\n  [warning]⏸ {err_msg}[/warning]", force=True)
                                    return TransferResult(
                                        success=False,
                                        bytes_downloaded=total_done,
                                        duration=time.time() - t_start,
                                        recycle_count=watchdog.cycle_count,
                                        error=err_msg,
                                        exit_code=int(ExitCode.FAILURE),
                                    )

                        if max_duration_sec is not None and (now - t_start) >= max_duration_sec:
                            dur_cap = max(now - t_start, 0.001)
                            avg_cap = total_done / dur_cap
                            p10_cap = (
                                compute_percentile(all_speed_samples, 10.0)
                                if all_speed_samples
                                else avg_cap
                            )
                            return TransferResult(
                                success=True,
                                file_path=out_dir,
                                bytes_downloaded=total_done,
                                duration=dur_cap,
                                avg_speed_bps=avg_cap,
                                p10_speed_bps=p10_cap,
                                recycle_count=watchdog.cycle_count,
                                exit_code=int(ExitCode.OK),
                            )

                        # Poll ResilienceEventSource on each tick (WAKE, NET_CHANGE, STALL)
                        res_event = ev_src.tick(downloaded_bytes=total_done)

                        # Evaluate watchdog once per ~1s (or immediately when a ResilienceEvent arrives)
                        step_interval = min(1.0, poll_interval * 2)
                        if (
                            res_event is not None
                            or (now - last_watch_step_ts) >= step_interval - 0.02
                        ):
                            last_watch_step_ts = now
                            decision = watchdog.step(
                                elapsed_sec=now - t_start,
                                speed_bps=global_speed,
                                per_conn_speeds=conn_speeds,
                                resilience_event=res_event,
                            )

                            if decision.action != WatchdogActionType.NONE:
                                UI.emit_event(
                                    "watchdog_action",
                                    cycle=decision.cycle,
                                    mode=watchdog.mode.value,
                                    action=decision.action.value,
                                    resilience_event=decision.event_type,
                                    reason=decision.reason,
                                    speed_bps=round(global_speed, 2),
                                    threshold_bps=round(decision.threshold_bps, 2),
                                    rolling_p90_bps=round(decision.rolling_p90_bps, 2),
                                    connections=watchdog.connections,
                                )
                                print_plain(
                                    f"\n  [bold yellow]⚡ [Watchdog Cycle #{decision.cycle}][/bold yellow] "
                                    f"[white]{decision.reason}[/white]"
                                )

                                if (
                                    decision.action == WatchdogActionType.SINGLE_STREAM_FALLBACK
                                    and notify
                                ):
                                    from paradl.hooks import HookRunner

                                    HookRunner.notify_watchdog_giveup(
                                        parsed_host, decision.reason, enabled=notify
                                    )

                                if decision.action == WatchdogActionType.RESILIENCE_RECYCLE:
                                    # Every resilience event (WAKE, NET_CHANGE, STALL) triggers the same action:
                                    # 1. Re-resolve DNS (via DoH if configured)
                                    resolve_dns(parsed_host, doh_endpoint=doh_endpoint)
                                    # 2. Reset the speed baseline
                                    all_speed_samples.clear()
                                    watchdog.reset_speed_baseline()
                                    ev_src.reset_progress_timer(total_done)
                                    # 3. Recycle all connections
                                    for active_gid in list(gid_to_idx.keys()):
                                        try:
                                            daemon.force_pause(active_gid)
                                        except Exception as rpc_err:
                                            logger.debug("Resilience pause failed: %s", rpc_err)
                                    get_clock().sleep(0.15)
                                    for active_gid in list(gid_to_idx.keys()):
                                        try:
                                            daemon.unpause(active_gid)
                                        except Exception as rpc_err:
                                            logger.debug("Resilience unpause failed: %s", rpc_err)

                                elif decision.action in (
                                    WatchdogActionType.PAUSE_UNPAUSE,
                                    WatchdogActionType.ADJUST_CONNECTIONS,
                                ):
                                    new_per_file = max(
                                        1, watchdog.connections // max(len(gid_to_idx), 1)
                                    )
                                    for active_gid in list(gid_to_idx.keys()):
                                        try:
                                            daemon.force_pause(active_gid)
                                            if (
                                                decision.action
                                                == WatchdogActionType.ADJUST_CONNECTIONS
                                            ):
                                                daemon.change_option(
                                                    active_gid,
                                                    {
                                                        "split": str(new_per_file),
                                                        "max-connection-per-server": str(
                                                            new_per_file
                                                        ),
                                                    },
                                                )
                                        except Exception as rpc_err:
                                            logger.debug("Pause/changeOption failed: %s", rpc_err)
                                    get_clock().sleep(0.15)
                                    for active_gid in list(gid_to_idx.keys()):
                                        try:
                                            daemon.unpause(active_gid)
                                        except Exception as rpc_err:
                                            logger.debug("Unpause failed: %s", rpc_err)
                                    ev_src.reset_progress_timer(total_done)
                                else:
                                    # Tier 3 process restart / IP family switch / single-stream fallback
                                    restart_daemon_needed = True
                                    break

                        # Sleep before next poll tick if transfers are still active.
                        # Adaptive polling: slow down when stable, speed up when volatile.
                        if gid_to_idx and not restart_daemon_needed:
                            if global_speed > 0 and _last_speed is not None:
                                speed_delta = abs(global_speed - _last_speed) / max(
                                    _last_speed, 1.0
                                )
                                if speed_delta < 0.10:
                                    _stable_ticks += 1
                                    if _stable_ticks >= 5 and _poll_interval < 1.0:
                                        _poll_interval = min(1.0, _poll_interval + 0.1)
                                else:
                                    _stable_ticks = 0
                                    if _poll_interval > 0.2:
                                        _poll_interval = max(0.2, _poll_interval - 0.1)
                            _last_speed = global_speed
                            get_clock().sleep(_poll_interval)
                finally:
                    # Release daemon back to pool instead of stopping it.
                    # If restart is needed, force-stop and remove from pool.
                    if restart_daemon_needed:
                        with contextlib.suppress(Exception):
                            daemon.stop(graceful=False)
                        # Remove from pool so a fresh one is acquired next iteration
                        pool = get_daemon_pool()
                        with pool._lock:
                            for key, entry in list(pool._entries.items()):
                                if entry.daemon is daemon:
                                    del pool._entries[key]
                                    break
                    else:
                        get_daemon_pool().release(daemon)

        duration = max(time.time() - t_start, 0.001)

        # Resolve final files and verify expected lengths + integrity hints
        final_files: list[Path] = []
        total_bytes = 0
        item_verified_flags: list[bool | None] = []
        item_algos: list[str | None] = []
        item_digests: list[str | None] = []

        for idx, it in enumerate(items):
            actual_reported = completed_paths.get(idx)
            if actual_reported and actual_reported.is_file():
                f_path = actual_reported
            else:
                f_path = it.out_dir / (it.filename or "download")

            if not f_path.is_file():
                candidates = [
                    p
                    for p in sorted(it.out_dir.glob("*"), key=os.path.getmtime, reverse=True)
                    if p.is_file()
                    and not p.name.endswith(".aria2")
                    and os.path.getmtime(p) >= (t_start - 1.0)
                    and (it.expected_length is None or p.stat().st_size == it.expected_length)
                ]
                if candidates:
                    f_path = candidates[0]

            if not f_path.is_file():
                return TransferResult(
                    success=False,
                    duration=duration,
                    recycle_count=watchdog.cycle_count,
                    error=f"Expected output file missing: {f_path}",
                    exit_code=int(ExitCode.FAILURE),
                )

            # Remove leftover .aria2 control file if present
            ctrl_file = Path(f"{f_path}.aria2")
            if ctrl_file.exists():
                with contextlib.suppress(OSError):
                    ctrl_file.unlink()

            actual_sz = f_path.stat().st_size
            total_bytes += actual_sz

            # Phase 2: Final size integrity check against expected length
            if (
                it.expected_length is not None
                and it.expected_length > 0
                and actual_sz != it.expected_length
            ):
                err_msg = f"Size mismatch for {f_path.name}: expected {it.expected_length} bytes, got {actual_sz} bytes"
                print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                return TransferResult(
                    success=False,
                    file_path=f_path,
                    bytes_downloaded=actual_sz,
                    duration=duration,
                    recycle_count=watchdog.cycle_count,
                    error=err_msg,
                    exit_code=int(ExitCode.VERIFICATION_FAILED),
                )

            # Phase 0 #12: Automatic server integrity hint verification (HF ETag SHA-256 / GitHub digest)
            if it.expected_digest:
                matched, algo, digest = ChecksumEngine.verify(f_path, it.expected_digest)
                item_verified_flags.append(matched)
                item_algos.append(algo)
                item_digests.append(digest)
                UI.emit_event(
                    "verification",
                    file=str(f_path),
                    algorithm=algo,
                    expected=it.expected_digest,
                    computed=digest,
                    verified=matched,
                    source="server_integrity_hint",
                )
                if matched:
                    print_plain(
                        f"  [green]✓ Verified server integrity digest ({algo.upper()})[/green]"
                    )
                else:
                    err_msg = f"Server integrity digest mismatch for {f_path.name}: expected {it.expected_digest}, got {digest}"
                    print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                    return TransferResult(
                        success=False,
                        file_path=f_path,
                        bytes_downloaded=actual_sz,
                        duration=duration,
                        recycle_count=watchdog.cycle_count,
                        checksum_verified=False,
                        checksum_algo=algo,
                        checksum_digest=digest,
                        error=err_msg,
                        exit_code=int(ExitCode.VERIFICATION_FAILED),
                    )
            else:
                item_verified_flags.append(None)
                item_algos.append(None)
                item_digests.append(None)

            final_files.append(f_path)

        avg_speed = total_bytes / duration if duration > 0 else 0.0
        p10_speed = compute_percentile(all_speed_samples, 10.0) if all_speed_samples else avg_speed
        primary_path = final_files[0] if len(final_files) == 1 else out_dir

        if write_xattr:
            for idx_f, f_p in enumerate(final_files):
                src_url = (
                    items[idx_f].url if idx_f < len(items) else (items[0].url if items else "")
                )
                if src_url and f_p.is_file():
                    platform.set_where_froms(f_p, [src_url])

        if len(items) == 1:
            verified_flag = item_verified_flags[0] if item_verified_flags else None
            verified_algo = item_algos[0] if item_algos else None
            verified_digest = item_digests[0] if item_digests else None
        else:
            has_any = any(v is not None for v in item_verified_flags)
            if not has_any:
                verified_flag = None
            elif all(v is True for v in item_verified_flags):
                verified_flag = True
            else:
                verified_flag = False
            unique_algos = {a for a in item_algos if a}
            verified_algo = unique_algos.pop() if len(unique_algos) == 1 else None
            verified_digest = None

        return TransferResult(
            success=True,
            file_path=primary_path,
            files=final_files,
            bytes_downloaded=total_bytes,
            duration=duration,
            avg_speed_bps=avg_speed,
            p10_speed_bps=p10_speed,
            recycle_count=watchdog.cycle_count,
            checksum_verified=verified_flag,
            checksum_algo=verified_algo,
            checksum_digest=verified_digest,
            exit_code=int(ExitCode.OK),
        )

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        safe_conn = min(max(connections, 1), 16)
        print_plain(f"  [bold white]Downloading:[/bold white] [cyan]{target_info.raw}[/cyan]")
        print_plain(
            f"  [dim]Destination:[/dim] [dim]{out_dir}[/dim] [dim]({safe_conn} parallel streams)[/dim]"
        )
        return self.download(
            target_info=target_info,
            out_dir=out_dir,
            connections=safe_conn,
            limit_rate=options.get("limit_rate"),
            anti_throttle=options.get("anti_throttle", "4MBps"),
            grace_period=options.get("grace_period", 5),
            watchdog_mode=options.get("watchdog_mode", WatchdogMode.V1),
            poll_interval=float(options.get("poll_interval", 0.2)),
            min_cooldown_sec=float(options.get("min_cooldown_sec", 2.0)),
            caffeinate=bool(options.get("caffeinate", True)),
            net_watch=bool(options.get("net_watch", True)),
            stall_timeout=float(options.get("stall_timeout", 30.0)),
            require_ac=bool(options.get("require_ac", False)),
            pause_on_low_power=bool(options.get("pause_on_low_power", False)),
            write_xattr=bool(options.get("write_xattr", not bool(options.get("no_xattr", False)))),
            notify=bool(options.get("notify", False)),
            mirrors=options.get("mirrors"),
            when_fast_bps=float(options.get("when_fast_bps", 0.0) or 0.0),
            when_fast_retest_interval_sec=float(
                options.get("when_fast_retest_interval_sec", 15.0) or 15.0
            ),
            when_fast_prober=options.get("when_fast_prober"),
            http3=bool(options.get("http3", True)),
            proxy=options.get("proxy"),
            doh_endpoint=options.get("doh_endpoint"),
        )
