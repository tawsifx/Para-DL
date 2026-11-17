"""Native Rust (``rapids``) download engine adapter.

Conforms to :class:`paradl.engines.base.Engine` and preserves the exact
``TransferResult`` / ``ExitCode`` / JSONL event contract of the legacy aria2
path.  The native PyO3 module is consumed through the ``paradl._rust_engine``
shim; module-level ``_native`` is intentionally resolved at call time so tests
can inject a fake before the extension is compiled.
"""

from __future__ import annotations

import contextlib
import inspect
import logging
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from paradl import _rust_engine as _native
from paradl.core.clock import get_clock
from paradl.core.models import (
    DownloadItem,
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
    format_rate_for_aria2,
    format_speed,
    parse_rate,
    sanitize_filename,
)
from paradl.engines.watchdog import (
    WatchdogActionType,
    WatchdogController,
    apply_recycle_decision,
    compute_percentile,
)
from paradl.postproc.checksum import ChecksumEngine
from paradl.resilience.events import (
    ResilienceEventSource,
    check_power_policy,
)
from paradl.resilience.queue import check_when_fast_gate
from paradl.ui import UI, LiveTransferDisplay, print_plain

logger = logging.getLogger(__name__)

MAX_CONNECTIONS = 16

_UNAVAILABLE_MSG = getattr(
    _native,
    "_UNAVAILABLE_MSG",
    "Native Rust engine is not available; build it or use --engine aria2.",
)


def _native_exceptions(*names: str) -> tuple[type[BaseException], ...]:
    """Collect exception classes actually exposed by the compiled extension."""
    found: list[type[BaseException]] = []
    for name in names:
        candidate = getattr(_native, name, None)
        if isinstance(candidate, type) and issubclass(candidate, BaseException):
            found.append(candidate)
    return tuple(found)


# The compiled module exposes VerificationError / DownloadError (and may expose
# cancellation / unavailability variants).  The Python shim exposes
# RustEngineUnavailable before the extension is built.  All of them, plus an
# unexpected Exception, must be converted to a TransferResult at the boundary.
_VERIFICATION_ERRORS: tuple[type[BaseException], ...] = _native_exceptions("VerificationError")
_DOWNLOAD_ERRORS: tuple[type[BaseException], ...] = _native_exceptions("DownloadError")
_UNAVAILABLE_ERRORS: tuple[type[BaseException], ...] = (
    ImportError,
    *_native_exceptions("RustEngineUnavailable", "RustNativeUnavailable"),
)
_CANCEL_ERRORS: tuple[type[BaseException], ...] = _native_exceptions(
    "CancelledError", "CancellationError"
)
_NATIVE_ERRORS: tuple[type[BaseException], ...] = _UNAVAILABLE_ERRORS


def _native_error_exit_code(exc: Exception) -> int:
    """Classify a native exception into the documented ExitCode contract."""
    message = str(exc).lower()
    if isinstance(exc, _UNAVAILABLE_ERRORS) or any(
        hint in message for hint in ("not available", "unavailable", "not built", "no module")
    ):
        return int(ExitCode.MISSING_DEPENDENCY)
    if isinstance(exc, _VERIFICATION_ERRORS) or any(
        hint in message for hint in ("checksum", "verif", "digest", "mismatch")
    ):
        return int(ExitCode.VERIFICATION_FAILED)
    if isinstance(exc, _CANCEL_ERRORS) or "cancel" in message:
        return int(ExitCode.INTERRUPTED)
    # DownloadError and any other native/runtime failure.
    return int(ExitCode.FAILURE)


_TERMINAL_STATES = frozenset(
    {"complete", "completed", "done", "finished", "error", "failed", "cancelled", "canceled"}
)
_SUCCESS_STATES = frozenset({"complete", "completed", "done", "finished"})


def _first(mapping: dict[str, Any], keys: tuple[str, ...], default: Any = None) -> Any:
    """Return the first non-None value among ``keys`` in ``mapping``."""
    for key in keys:
        if key in mapping and mapping[key] is not None:
            return mapping[key]
    return default


def _coerce_mapping(value: Any) -> dict[str, Any]:
    """Normalise a native result / snapshot into a plain dict."""
    if isinstance(value, dict):
        return value
    for method in ("as_dict", "to_dict", "_asdict"):
        candidate = getattr(value, method, None)
        if callable(candidate):
            try:
                result = candidate()
            except Exception:  # pragma: no cover - defensive around native code
                continue
            if isinstance(result, dict):
                return result
    if hasattr(value, "__dict__"):
        return dict(vars(value))
    return {}


def _snapshot_field(snapshot: Any, name: str, default: Any = 0) -> Any:
    mapping = _coerce_mapping(snapshot)
    aliases: dict[str, tuple[str, ...]] = {
        "downloaded": ("downloaded", "downloaded_bytes", "bytes_downloaded", "completed"),
        "total": ("total", "total_bytes", "length", "size"),
        "speed_bps": ("speed_bps", "speed", "download_speed", "bps"),
        "connections": ("connections", "num_connections", "active_connections", "channels"),
        "state": ("state", "status"),
    }
    return _first(mapping, aliases.get(name, (name,)), default=default)


def _is_terminal(snapshot: Any) -> bool:
    state = str(_snapshot_field(snapshot, "state", "")).lower()
    if state in _TERMINAL_STATES:
        return True
    downloaded = int(_snapshot_field(snapshot, "downloaded", 0) or 0)
    total = int(_snapshot_field(snapshot, "total", 0) or 0)
    return bool(total > 0 and downloaded >= total and state not in {"", "running", "active"})


def _native_available() -> bool:
    probe = getattr(_native, "is_available", None)
    if callable(probe):
        try:
            return bool(probe())
        except Exception:
            return False
    return callable(getattr(_native, "start_download", None))


def _call_native(fn: Any, positional: tuple[Any, ...], candidates: dict[str, Any]) -> Any:
    """Call a native function with only the keyword arguments it accepts."""
    try:
        params = inspect.signature(fn).parameters
    except (TypeError, ValueError):
        return fn(*positional, **{k: v for k, v in candidates.items() if v is not None})
    if any(p.kind is p.VAR_KEYWORD for p in params.values()):
        return fn(*positional, **candidates)
    kwargs = {k: v for k, v in candidates.items() if k in params and v is not None}
    return fn(*positional, **kwargs)


def _unavailable_result() -> TransferResult:
    return TransferResult(
        success=False,
        error=str(_UNAVAILABLE_MSG),
        exit_code=int(ExitCode.MISSING_DEPENDENCY),
    )


def _map_native_result(
    native: Any,
    *,
    out_path: Path,
    expected_digest: str | None = None,
    duration: float = 0.0,
    recycle_count: int = 0,
    avg_speed_bps: float = 0.0,
    p10_speed_bps: float = 0.0,
) -> TransferResult:
    """Map a native wait() result to a TransferResult with an identical ExitCode."""
    data = _coerce_mapping(native)
    state = str(_first(data, ("state", "status"), default="")).lower()
    success = bool(
        _first(data, ("success", "ok", "completed"), default=False) or state in _SUCCESS_STATES
    )
    error_value = _first(data, ("error", "message", "error_message"))
    error = str(error_value) if error_value else None

    file_value = _first(data, ("file_path", "path", "out_path", "destination"))
    file_path = Path(str(file_value)) if file_value else out_path
    if not file_path.is_file():
        file_path = out_path

    downloaded_value = _first(
        data,
        ("downloaded", "bytes_downloaded", "downloaded_bytes", "completed", "total_done"),
        default=0,
    )
    try:
        downloaded = int(downloaded_value or 0)
    except (TypeError, ValueError):
        downloaded = 0

    verified_value = _first(data, ("checksum_verified", "verified"))
    algo_value = _first(data, ("checksum_algo", "algorithm"))
    digest_value = _first(data, ("checksum_digest", "computed", "computed_digest"))
    checksum_verified = bool(verified_value) if verified_value is not None else None
    algo = str(algo_value) if algo_value else None
    digest = str(digest_value) if digest_value else None
    has_checksum = bool(expected_digest) or checksum_verified is not None
    verification_failed = checksum_verified is False

    if expected_digest and file_path.is_file():
        try:
            matched, computed_algo, computed_digest = ChecksumEngine.verify(
                file_path, expected_digest
            )
        except (OSError, ValueError) as exc:
            logger.debug("Checksum verification error for %s: %s", file_path, exc)
            matched, computed_algo, computed_digest = False, "sha256", ""
        checksum_verified = matched
        algo = computed_algo
        digest = computed_digest
        verification_failed = not matched
    if "verif" in state and ("fail" in state or "mismatch" in state):
        verification_failed = True

    explicit_value = _first(data, ("exit_code", "exitCode", "code"))
    try:
        explicit_code = int(explicit_value) if explicit_value is not None else None
    except (TypeError, ValueError):
        explicit_code = None
    valid_codes = {int(code) for code in ExitCode}

    if verification_failed:
        success = False
        exit_code = int(ExitCode.VERIFICATION_FAILED)
        if not error:
            error = f"Server integrity digest mismatch for {file_path.name}"
    elif explicit_code in valid_codes:
        exit_code = int(explicit_code)
    elif success:
        exit_code = int(ExitCode.OK)
    else:
        exit_code = int(ExitCode.FAILURE)

    return TransferResult(
        success=success,
        file_path=file_path,
        bytes_downloaded=downloaded,
        duration=duration,
        avg_speed_bps=avg_speed_bps,
        p10_speed_bps=p10_speed_bps,
        recycle_count=recycle_count,
        checksum_verified=checksum_verified if has_checksum else None,
        checksum_algo=algo,
        checksum_digest=digest,
        error=error,
        exit_code=exit_code,
    )


def _filename_for(url: str, hint: str | None) -> str:
    if hint:
        return sanitize_filename(hint)
    candidate = Path(urlparse(url).path).name
    return sanitize_filename(candidate) if candidate else "download"


def _host_for(url: str) -> str:
    return (urlparse(url).hostname or "").lower().rstrip(".")


def _watchdog_mode(value: WatchdogMode | str | None) -> WatchdogMode:
    if isinstance(value, WatchdogMode):
        return value
    try:
        return WatchdogMode(str(value or "v2").strip().lower())
    except ValueError:
        return WatchdogMode.V2


def _start_handle(
    url: str,
    out_path: Path,
    *,
    connections: int,
    options: dict[str, Any],
    resume: bool,
) -> Any:
    candidates: dict[str, Any] = {
        "connections": connections,
        "headers": options.get("headers") or None,
        "mirrors": options.get("mirrors") or None,
        # The compiled PyO3 signature (rust/paradl-rs/src/lib.rs) declares
        # `checksum` and `cookies`; using expected_digest/cookie_file here would
        # be silently dropped by _call_native's declared-parameter filter.
        "checksum": options.get("expected_digest"),
        "cookies": options.get("cookie_file"),
        "referer": options.get("referer"),
        "user_agent": options.get("user_agent"),
        "proxy": options.get("proxy"),
        "ip_family": options.get("ip_family"),
        "rate_limit_bps": options.get("rate_limit_bps"),
        "accept_ranges": options.get("accept_ranges"),
        "total": options.get("total"),
        # Pass-through hints for native versions that support them; the
        # signature filter below drops any the compiled extension does not
        # accept (see the report for the no-equivalent flag inventory).
        "http3": options.get("http3"),
        "doh_endpoint": options.get("doh_endpoint"),
    }
    fn = getattr(_native, "resume_download", None) if resume else None
    if not callable(fn):
        fn = _native.start_download
    try:
        return _call_native(fn, (url, str(out_path)), candidates)
    except TypeError as exc:
        # Fall back to an all-keyword call for keyword-only native signatures.
        if "argument" not in str(exc).lower():
            raise
        return _call_native(
            fn,
            (),
            {**candidates, "url": url, "out_path": str(out_path), "dest": str(out_path)},
        )


class RustEngine:
    """Adapter exposing the native in-process engine through the Engine protocol."""

    @staticmethod
    def parse_speed_str(speed_str: str | None) -> float:
        return parse_rate(speed_str)

    @classmethod
    def download(
        cls,
        target_info: TargetInfo,
        out_dir: Path,
        connections: int = MAX_CONNECTIONS,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
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
        event_source: Any = None,
        http3: bool = True,
        proxy: str | None = None,
        doh_endpoint: str | None = None,
        ip_family: str | None = None,
        on_progress: Callable[[int, int | None, float, int], None] | None = None,
        **_extra: Any,
    ) -> TransferResult:
        merged_mirrors = list(target_info.mirrors)
        for mirror in mirrors or []:
            if mirror and mirror not in merged_mirrors:
                merged_mirrors.append(mirror)

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
            ip_family=ip_family,
            on_progress=on_progress,
        )

    @classmethod
    def download_batch(
        cls,
        items: list[DownloadItem],
        out_dir: Path,
        connections: int = MAX_CONNECTIONS,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
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
        event_source: Any = None,
        http3: bool = True,
        proxy: str | None = None,
        doh_endpoint: str | None = None,
        ip_family: str | None = None,
        on_progress: Callable[[int, int | None, float, int], None] | None = None,
        **_extra: Any,
    ) -> TransferResult:
        if is_torrent:
            # Ruling: torrent/magnet/metalink stay on the aria2 backend in v1.
            from paradl.engines.aria2 import Aria2Engine

            return Aria2Engine._legacy_download_batch(
                items,
                out_dir,
                connections=connections,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
                accept_ranges_hint=accept_ranges_hint,
                is_torrent=True,
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

        if not _native_available():
            return _unavailable_result()

        clock = get_clock()
        wall_start = time.time()
        mono_start = clock.monotonic()
        safe_conn = min(max(int(connections), 1), MAX_CONNECTIONS)
        rate_limit_bps = format_rate_for_aria2(limit_rate)
        floor_bps = parse_rate(anti_throttle) if anti_throttle else 0.0

        # Python-side guards with no native equivalent.  These preserve the
        # observable behaviour of the legacy aria2 path (see report).
        power_ok, power_err = check_power_policy(
            require_ac=require_ac,
            pause_on_low_power=pause_on_low_power,
        )
        if not power_ok:
            err_msg = power_err or "Power policy check failed."
            UI.emit_event("error", reason="power_policy", message=err_msg)
            return TransferResult(success=False, error=err_msg, exit_code=int(ExitCode.FAILURE))

        first_url = items[0].url if items else ""
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

        # Resilience events (WAKE / NET_CHANGE / STALL) drive the same watchdog
        # state machine as the legacy aria2 loop.  Without this, RESILIENCE_RECYCLE
        # decisions -- the exact cause of the diagnosed watchdog-cycle bug --
        # can never reach ``handle.recycle_connections()`` in Rust mode.
        ev_src: Any = event_source or ResilienceEventSource(
            net_watch=net_watch,
            stall_timeout_sec=stall_timeout,
        )

        watchdog = WatchdogController(
            mode=_watchdog_mode(watchdog_mode),
            user_floor_bps=floor_bps,
            grace_period=grace_period,
            connections=safe_conn,
            host=_host_for(items[0].url) if items else "",
            supports_range=(accept_ranges_hint if accept_ranges_hint is not None else True),
            min_cooldown_sec=min_cooldown_sec,
        )

        started: list[tuple[DownloadItem, Path, Any]] = []
        speed_samples: list[float] = []
        primary_out_path: Path | None = (
            out_dir / _filename_for(items[0].url, items[0].filename) if items else None
        )
        try:
            for item in items:
                out_path = out_dir / _filename_for(item.url, item.filename)
                out_dir.mkdir(parents=True, exist_ok=True)
                resume = any(
                    Path(f"{out_path}{suffix}").exists()
                    for suffix in (".pdl.part", ".pdl.json", ".aria2")
                )
                handle = _start_handle(
                    item.url,
                    out_path,
                    connections=safe_conn,
                    options={
                        "headers": item.headers,
                        "mirrors": item.mirrors,
                        "expected_digest": item.expected_digest,
                        "cookie_file": item.cookie_file,
                        "referer": item.referer,
                        "user_agent": item.user_agent,
                        "proxy": proxy,
                        "ip_family": ip_family,
                        "rate_limit_bps": rate_limit_bps,
                        "accept_ranges": accept_ranges_hint,
                        "total": item.expected_length,
                        "http3": http3,
                        "doh_endpoint": doh_endpoint,
                    },
                    resume=resume,
                )
                started.append((item, out_path, handle))

            primary_total = (
                int(_snapshot_field(started[0][2].poll_snapshot(), "total", 0) or 0)
                if started
                else 0
            )
            display = LiveTransferDisplay(
                title=Path(_filename_for(items[0].url, items[0].filename)).name
                if items
                else "download",
                total_bytes=primary_total,
            )

            last_downloaded = 0
            last_mono = clock.monotonic()
            with contextlib.ExitStack() as stack:
                stack.enter_context(display)
                if hasattr(ev_src, "__enter__"):
                    stack.enter_context(ev_src)
                while started:
                    now_mono = clock.monotonic()
                    aggregate_downloaded = 0
                    aggregate_total = 0
                    aggregate_speed = 0.0
                    aggregate_connections = 0
                    all_terminal = True
                    for _item, _out_path, handle in started:
                        snapshot = handle.poll_snapshot()
                        downloaded = int(_snapshot_field(snapshot, "downloaded", 0) or 0)
                        total = int(_snapshot_field(snapshot, "total", 0) or 0)
                        speed = float(_snapshot_field(snapshot, "speed_bps", 0.0) or 0.0)
                        connections = int(_snapshot_field(snapshot, "connections", 0) or 0)
                        aggregate_downloaded += downloaded
                        aggregate_total = max(aggregate_total, total)
                        aggregate_speed += speed
                        aggregate_connections += connections
                        if not _is_terminal(snapshot):
                            all_terminal = False

                    delta = max(now_mono - last_mono, 0.0)
                    if delta > 0:
                        instant_speed = max(0.0, (aggregate_downloaded - last_downloaded) / delta)
                        if instant_speed > 0:
                            speed_samples.append(instant_speed)
                    else:
                        instant_speed = aggregate_speed
                    last_downloaded = aggregate_downloaded
                    last_mono = now_mono

                    # Poll WAKE / NET_CHANGE / STALL triggers on every tick,
                    # exactly like the legacy aria2 supervision loop.
                    res_event = ev_src.tick(downloaded_bytes=aggregate_downloaded)

                    smooth_speed = aggregate_speed or instant_speed
                    display.update(
                        downloaded_bytes=aggregate_downloaded,
                        total_bytes=aggregate_total or None,
                        speed_bps=smooth_speed,
                        connections=aggregate_connections,
                        watchdog_state=watchdog.status_label(),
                    )
                    if on_progress is not None:
                        on_progress(
                            aggregate_downloaded,
                            aggregate_total or None,
                            smooth_speed,
                            aggregate_connections,
                        )

                    decision = watchdog.step(
                        elapsed_sec=now_mono - mono_start,
                        speed_bps=smooth_speed,
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
                            speed_bps=round(smooth_speed, 2),
                            threshold_bps=round(decision.threshold_bps, 2),
                            rolling_p90_bps=round(decision.rolling_p90_bps, 2),
                            connections=watchdog.connections,
                        )
                        print_plain(
                            f"\n  [bold yellow]⚡ [Watchdog Cycle #{decision.cycle}][/bold yellow] "
                            f"[white]{decision.reason}[/white]"
                        )
                        for _item, _out_path, handle in started:
                            apply_recycle_decision(decision, handle)
                        reset_timer = getattr(ev_src, "reset_progress_timer", None)
                        if callable(reset_timer):
                            reset_timer(aggregate_downloaded)

                    if all_terminal:
                        break
                    if max_duration_sec is not None and (now_mono - mono_start) >= max_duration_sec:
                        for _item, _out_path, handle in started:
                            handle.cancel()
                        break
                    clock.sleep(poll_interval)

            duration = max(time.time() - wall_start, 0.001)
            avg_speed = sum(speed_samples) / len(speed_samples) if speed_samples else 0.0
            p10_speed = compute_percentile(speed_samples, 10.0) if speed_samples else avg_speed

            final_files: list[Path] = []
            total_bytes = 0
            verified_flags: list[bool | None] = []
            algos: list[str | None] = []
            digests: list[str | None] = []
            failures: list[TransferResult] = []

            for item, out_path, handle in started:
                wait_result = handle.wait(timeout=stall_timeout)
                result = _map_native_result(
                    wait_result,
                    out_path=out_path,
                    expected_digest=item.expected_digest,
                    duration=duration,
                    recycle_count=watchdog.cycle_count,
                    avg_speed_bps=avg_speed,
                    p10_speed_bps=p10_speed,
                )
                if item.expected_digest:
                    UI.emit_event(
                        "verification",
                        file=str(result.file_path or out_path),
                        algorithm=result.checksum_algo or "sha256",
                        expected=item.expected_digest,
                        computed=result.checksum_digest,
                        verified=bool(result.checksum_verified),
                        source="server_integrity_hint",
                    )
                if not result.success:
                    failures.append(result)
                    continue
                final_path = result.file_path or out_path
                if final_path.is_file():
                    actual_size = final_path.stat().st_size
                    if item.expected_length and actual_size != item.expected_length:
                        err_msg = (
                            f"Size mismatch for {final_path.name}: expected "
                            f"{item.expected_length} bytes, got {actual_size} bytes"
                        )
                        UI.emit_event("error", reason="size_mismatch", message=err_msg)
                        failures.append(
                            TransferResult(
                                success=False,
                                file_path=final_path,
                                bytes_downloaded=actual_size,
                                duration=duration,
                                recycle_count=watchdog.cycle_count,
                                error=err_msg,
                                exit_code=int(ExitCode.VERIFICATION_FAILED),
                            )
                        )
                        continue
                    # Count bytes for every successful transfer, even when no
                    # expected_length hint was supplied (e.g. streamed ranges).
                    total_bytes += actual_size
                if write_xattr and final_path.is_file():
                    from paradl import platform

                    with contextlib.suppress(OSError):
                        platform.set_where_froms(final_path, [item.url])
                final_files.append(final_path)
                verified_flags.append(result.checksum_verified)
                algos.append(result.checksum_algo)
                digests.append(result.checksum_digest)

            if failures:
                primary = failures[0]
                if len(started) == 1:
                    return primary
                return TransferResult(
                    success=False,
                    file_path=primary.file_path,
                    files=[f.file_path for f in failures if f.file_path],
                    bytes_downloaded=sum(f.bytes_downloaded for f in failures),
                    duration=duration,
                    avg_speed_bps=avg_speed,
                    p10_speed_bps=p10_speed,
                    recycle_count=watchdog.cycle_count,
                    error=primary.error,
                    exit_code=primary.exit_code,
                )

            if len(started) == 1:
                verified_flag = verified_flags[0] if verified_flags else None
                verified_algo = algos[0] if algos else None
                verified_digest = digests[0] if digests else None
            else:
                flags = [flag for flag in verified_flags if flag is not None]
                verified_flag = all(flags) if flags else None
                unique_algos = {algo for algo in algos if algo}
                verified_algo = unique_algos.pop() if len(unique_algos) == 1 else None
                verified_digest = None

            return TransferResult(
                success=True,
                file_path=final_files[0] if len(final_files) == 1 else out_dir,
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
        except Exception as exc:  # Engine protocol boundary: never leak native errors
            exit_code = _native_error_exit_code(exc)
            is_verification = exit_code == int(ExitCode.VERIFICATION_FAILED)
            is_unavailable = exit_code == int(ExitCode.MISSING_DEPENDENCY)
            reason = (
                "verification_failed"
                if is_verification
                else "missing_native_engine"
                if is_unavailable
                else "download_failed"
            )
            logger.warning("Native Rust engine error (%s): %s", reason, exc)
            UI.emit_event("error", reason=reason, message=str(exc))
            if is_unavailable:
                return TransferResult(
                    success=False,
                    error=str(exc),
                    exit_code=int(ExitCode.MISSING_DEPENDENCY),
                )
            return TransferResult(
                success=False,
                file_path=primary_out_path,
                error=str(exc),
                checksum_verified=False if is_verification else None,
                exit_code=exit_code,
            )

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = MAX_CONNECTIONS,
        **options: Any,
    ) -> TransferResult:
        safe_conn = min(max(connections, 1), MAX_CONNECTIONS)
        print_plain(f"  [bold white]Downloading:[/bold white] [cyan]{target_info.raw}[/cyan]")
        print_plain(
            f"  [dim]Destination:[/dim] [dim]{out_dir}[/dim] "
            f"[dim]({safe_conn} parallel streams)[/dim]"
        )
        return self.download(target_info, out_dir, connections=safe_conn, **options)
