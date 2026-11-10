"""Resilience event source (WAKE, NET_CHANGE, STALL) and start-up guards (disk & power policy)."""

from __future__ import annotations

import logging
from collections.abc import Callable
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from typing import Any

from paradl import platform
from paradl.clock import Clock, get_clock
from paradl.models import format_size
from paradl.platform import macos

logger = logging.getLogger(__name__)


class ResilienceEventType(StrEnum):
    WAKE = "wake"
    NET_CHANGE = "net_change"
    STALL = "stall"


@dataclass
class ResilienceEvent:
    event_type: ResilienceEventType
    reason: str
    timestamp: float
    detail: dict[str, Any] = field(default_factory=dict)


class ResilienceEventSource:
    """Unified event source feeding the watchdog on every tick.

    Emits debounced ResilienceEvents for:
    - WAKE: wall-clock delta jumps ahead of monotonic delta (macOS sleep/wake).
    - NET_CHANGE: notifications from `scutil` watcher -> `route -n monitor` -> `scutil --nwi` poll.
    - STALL: no byte progress for `stall_timeout_sec` seconds (default 30s).
    """

    def __init__(
        self,
        *,
        net_watch: bool = True,
        stall_timeout_sec: float = 30.0,
        wake_jump_threshold_sec: float = 3.0,
        debounce_sec: float = 2.0,
        wake_watch: bool = True,
        wall_clock: Callable[[], float] | None = None,
        mono_clock: Callable[[], float] | None = None,
        clock: Clock | None = None,
        net_watcher: Any | None = None,
    ) -> None:
        self.net_watch = bool(net_watch)
        self.stall_timeout_sec = float(stall_timeout_sec)
        self.wake_jump_threshold_sec = max(0.5, float(wake_jump_threshold_sec))
        self.debounce_sec = max(0.0, float(debounce_sec))
        self.wake_watch = bool(wake_watch)
        active_clock = clock or get_clock()
        self.clock = active_clock
        self.wall_clock = wall_clock or active_clock.now
        self.mono_clock = mono_clock or active_clock.monotonic

        if net_watcher is not None:
            self.net_watcher = net_watcher
        elif platform.is_windows():
            watcher_cls = platform.get_network_watcher_cls()
            self.net_watcher = watcher_cls(
                on_change=self.simulate_net_change,
                enabled=self.net_watch,
                mono_clock=self.mono_clock,
            )
        else:
            self.net_watcher = macos.MacOSNetworkWatcher(
                enabled=self.net_watch,
                mono_clock=self.mono_clock,
            )

        now_m = self.mono_clock()
        now_w = self.wall_clock()
        self._simulated_wall_offset: float = 0.0
        self._last_mono: float = now_m
        self._last_wall: float = now_w
        self._last_progress_bytes: int = 0
        self._last_progress_mono: float = now_m

        # Initialize last event timestamps far in the past so the first real event can fire immediately
        init_past = now_m - max(self.debounce_sec * 10.0, 1000.0)
        self._last_event_mono: float = init_past
        self._last_wake_mono: float = init_past
        self._last_net_change_mono: float = init_past
        self._last_stall_mono: float = init_past

    def start(self) -> None:
        """Start the underlying network watcher if enabled."""
        now_m = self.mono_clock()
        now_w = self.wall_clock() + self._simulated_wall_offset
        self._last_mono = now_m
        self._last_wall = now_w
        self._last_progress_mono = now_m
        if self.net_watch:
            self.net_watcher.enabled = True
            self.net_watcher.start()

    def stop(self) -> None:
        """Stop the underlying network watcher."""
        self.net_watcher.stop()

    def __enter__(self) -> ResilienceEventSource:
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop()

    def simulate_clock_jump(self, jump_seconds: float = 30.0) -> None:
        """Simulate a system sleep/wake wall-clock jump of `jump_seconds`."""
        self._simulated_wall_offset += float(jump_seconds)

    def simulate_net_change(
        self, detail: str = "scutil: changed key [0] = State:/Network/Global/IPv4"
    ) -> None:
        """Simulate a raw network-change notification arriving from the watcher."""
        self.net_watcher.inject_notification(detail)

    def simulate_stall(self, stall_seconds: float | None = None) -> None:
        """Simulate a byte-progress stall exceeding stall_timeout_sec."""
        shift = (
            stall_seconds if stall_seconds is not None else (max(self.stall_timeout_sec, 1.0) + 1.0)
        )
        self._last_progress_mono -= float(shift)

    def reset_progress_timer(self, downloaded_bytes: int | None = None) -> None:
        """Reset stall progress baseline after a connection recycle."""
        now_m = self.mono_clock()
        self._last_progress_mono = now_m
        if downloaded_bytes is not None:
            self._last_progress_bytes = max(self._last_progress_bytes, int(downloaded_bytes))

    def tick(self, downloaded_bytes: int = 0) -> ResilienceEvent | None:
        """Evaluate all resilience triggers on a watchdog tick and return at most one debounced event."""
        now_m = self.mono_clock()
        now_w = self.wall_clock() + self._simulated_wall_offset

        mono_delta = max(0.0, now_m - self._last_mono)
        wall_delta = now_w - self._last_wall
        self._last_mono = now_m
        self._last_wall = now_w

        if downloaded_bytes > self._last_progress_bytes:
            self._last_progress_bytes = downloaded_bytes
            self._last_progress_mono = now_m

        # 1. WAKE detection: wall-clock jump relative to monotonic clock
        clock_jump = wall_delta - mono_delta
        if self.wake_watch and clock_jump >= self.wake_jump_threshold_sec:
            # Reset stall timer on wake so a sleep cycle does not double-fire STALL
            self._last_progress_mono = now_m
            if (now_m - self._last_wake_mono) >= self.debounce_sec and (
                now_m - self._last_event_mono
            ) >= self.debounce_sec:
                self._last_wake_mono = now_m
                self._last_event_mono = now_m
                return ResilienceEvent(
                    event_type=ResilienceEventType.WAKE,
                    reason=f"System wake detected (wall-clock jumped +{clock_jump:.1f}s over monotonic)",
                    timestamp=now_w,
                    detail={
                        "wall_delta": round(wall_delta, 3),
                        "mono_delta": round(mono_delta, 3),
                        "clock_jump": round(clock_jump, 3),
                    },
                )

        # 2. NET_CHANGE detection: drain watcher notifications and debounce bursts (~2s)
        if self.net_watch:
            raw_notes = self.net_watcher.poll_notifications()
            if (
                raw_notes
                and (now_m - self._last_net_change_mono) >= self.debounce_sec
                and (now_m - self._last_event_mono) >= self.debounce_sec
            ):
                self._last_net_change_mono = now_m
                self._last_event_mono = now_m
                self._last_progress_mono = now_m
                return ResilienceEvent(
                    event_type=ResilienceEventType.NET_CHANGE,
                    reason=(
                        f"Network change detected via {self.net_watcher.backend} ({len(raw_notes)} notification(s))"
                    ),
                    timestamp=now_w,
                    detail={
                        "backend": self.net_watcher.backend,
                        "count": len(raw_notes),
                        "notifications": raw_notes[:5],
                    },
                )

        # 3. STALL detection: no byte progress for stall_timeout_sec
        if self.stall_timeout_sec > 0:
            stall_elapsed = now_m - self._last_progress_mono
            if stall_elapsed >= self.stall_timeout_sec:
                self._last_progress_mono = now_m
                if (now_m - self._last_stall_mono) >= self.debounce_sec and (
                    now_m - self._last_event_mono
                ) >= self.debounce_sec:
                    self._last_stall_mono = now_m
                    self._last_event_mono = now_m
                    return ResilienceEvent(
                        event_type=ResilienceEventType.STALL,
                        reason=(
                            f"Transfer stalled: no byte progress for {stall_elapsed:.1f}s "
                            f"(stall timeout {self.stall_timeout_sec:.1f}s)"
                        ),
                        timestamp=now_w,
                        detail={
                            "stall_seconds": round(stall_elapsed, 3),
                            "stall_timeout_sec": self.stall_timeout_sec,
                            "downloaded_bytes": downloaded_bytes,
                        },
                    )

        return None


# ---------------------------------------------------------------------------
# A2: Start-Up Guards (Free-Disk & Power Policy)
# ---------------------------------------------------------------------------


def compute_disk_margin(expected_bytes: int) -> int:
    """Compute safety margin bytes required above expected_bytes."""
    if expected_bytes <= 0:
        return 0
    # 5% of file size, or 10% up to 64 MiB for smaller files, minimum 1 KiB
    return max(
        int(expected_bytes * 0.05),
        min(int(expected_bytes * 0.10), 64 * 1024 * 1024),
        1024,
    )


def check_free_disk_space(
    out_dir: Path,
    expected_bytes: int | None,
    *,
    existing_bytes: int = 0,
    margin_bytes: int | None = None,
) -> tuple[bool, str | None]:
    """Verify free disk space in out_dir against expected_bytes + margin.

    Returns (ok, error_message).
    """
    if expected_bytes is None or expected_bytes <= 0:
        return True, None

    remaining = max(0, int(expected_bytes) - max(0, int(existing_bytes)))
    if remaining <= 0:
        return True, None

    margin = margin_bytes if margin_bytes is not None else compute_disk_margin(remaining)
    required = remaining + margin
    free_bytes = platform.get_current_platform_module().get_free_disk_bytes(out_dir)

    if free_bytes < required:
        msg = (
            f"Insufficient disk space in '{out_dir}': required {format_size(required)} "
            f"({format_size(remaining)} expected + {format_size(margin)} margin), "
            f"available {format_size(free_bytes)}."
        )
        return False, msg
    return True, None


def check_power_policy(
    *,
    require_ac: bool = False,
    pause_on_low_power: bool = False,
    max_pause_polls: int = 2,
    poll_interval: float = 0.05,
) -> tuple[bool, str | None]:
    """Enforce start-up power policy via system power status and Low Power / Battery Saver mode.

    - `--require-ac`: fails immediately if not on AC power.
    - `--pause-on-low-power`: pauses and re-polls; if Low Power / Battery Saver remains active, fails/defers cleanly.
    """
    if not require_ac and not pause_on_low_power:
        return True, None

    status = platform.get_power_status()
    if require_ac and not status.on_ac_power:
        pct_str = f" ({status.battery_percent}%)" if status.battery_percent is not None else ""
        return (
            False,
            f"Power policy guard failed (--require-ac): system is on {status.raw_source}{pct_str}; AC power is required.",
        )

    if pause_on_low_power and status.low_power_mode:
        for _ in range(max(0, int(max_pause_polls) - 1)):
            get_clock().sleep(poll_interval)
            status = platform.get_power_status()
            if require_ac and not status.on_ac_power:
                return (
                    False,
                    f"Power policy guard failed (--require-ac): system switched to {status.raw_source}; AC power is required.",
                )
            if not status.low_power_mode:
                return True, None
        return (
            False,
            "Power policy guard paused transfer (--pause-on-low-power): Low Power / Battery Saver mode is active.",
        )

    return True, None
