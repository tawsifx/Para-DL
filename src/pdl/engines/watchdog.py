"""Anti-Throttle Watchdog v1 (legacy) and v2 (adaptive p90, tiered RPC socket cycling, escalation ladder, resilience events, and learned profiles)."""

from __future__ import annotations

import logging
import math
from dataclasses import dataclass, field
from enum import StrEnum

from pdl.clock import get_clock
from pdl.events import ResilienceEvent
from pdl.history import HistoryManager, LearnedProfile, compute_hour_bucket
from pdl.models import WatchdogMode, format_speed
from pdl import platform

logger = logging.getLogger(__name__)


class WatchdogActionType(StrEnum):
    NONE = "none"
    PAUSE_UNPAUSE = "tier2_pause_unpause"
    RESILIENCE_RECYCLE = "resilience_recycle"
    ADJUST_CONNECTIONS = "adjust_connections"
    SWITCH_IP_FAMILY = "switch_ip_family"
    PROCESS_RESTART = "tier3_restart"
    SINGLE_STREAM_FALLBACK = "single_stream_fallback"


@dataclass
class WatchdogDecision:
    action: WatchdogActionType = WatchdogActionType.NONE
    reason: str = ""
    new_connections: int | None = None
    new_ip_family: str | None = None
    threshold_bps: float = 0.0
    rolling_p90_bps: float = 0.0
    cycle: int = 0
    event_type: str | None = None


def compute_percentile(samples: list[float], pct: float) -> float:
    """Compute percentile (0..100) of non-empty float list."""
    if not samples:
        return 0.0
    s = sorted(samples)
    k = (len(s) - 1) * (pct / 100.0)
    f = math.floor(k)
    c = math.ceil(k)
    if f == c:
        return s[int(k)]
    return s[f] * (c - k) + s[c] * (k - f)


@dataclass
class WatchdogController:
    """Enhanced Single-Channel Anti-Throttle Watchdog.

    Locks 16 parallel channels, eliminates slow process restarts and fallback
    degradation, and uses instant in-process JSON-RPC socket cycling (~150ms)
    to reset ISP per-flow token buckets at maximum line rate.
    """

    mode: WatchdogMode
    user_floor_bps: float
    grace_period: int = 5
    connections: int = 16
    host: str = ""
    supports_range: bool = True
    min_cooldown_sec: float = 2.0
    max_recycles: int = 999999
    hour_bucket: int | None = None
    network_id: str | None = None
    apply_learned_profile: bool = True
    revalidation_ticks: int = 2
    now_epoch: float | None = None
    sustained_throttle_threshold: int = 10

    # Runtime state
    initial_connections: int = 16
    cycle_count: int = 0
    warm_up_complete: bool = False
    low_speed_consecutive: int = 0
    sustained_low_speed_cycles: int = 0
    sustained_throttle_detected: bool = False
    speed_samples: list[float] = field(default_factory=list)
    last_recycle_elapsed: float = -999.0
    current_cooldown: float = 0.0
    last_action_taken: str = "tier2_pause_unpause"
    ip_family: str = "auto"
    disabled_reason: str | None = None
    exhausted: bool = False

    # Learned profile & staleness re-validation state
    applied_profile: LearnedProfile | None = None
    revalidation_pending: bool = False
    revalidation_passed: bool | None = None
    revalidation_samples: list[float] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.grace_period = max(int(self.grace_period), 2)
        self.current_cooldown = self.min_cooldown_sec
        self.initial_connections = min(max(int(self.connections), 1), 16)
        self.connections = self.initial_connections

        if self.hour_bucket is None:
            self.hour_bucket = compute_hour_bucket()
        if not self.network_id:
            self.network_id = platform.get_network_id() or "default"

        if not self.supports_range:
            self.disabled_reason = "no_range_support"
            return

        if self.mode == WatchdogMode.OFF or self.user_floor_bps <= 0:
            self.disabled_reason = "off"
            return

        # Load persisted learned profile if available
        if self.host and self.apply_learned_profile:
            saved = HistoryManager.get_best_profile(
                self.host,
                hour_bucket=self.hour_bucket,
                network_id=self.network_id,
            )
            if saved is not None:
                self.applied_profile = saved
                if saved.connections and 1 <= saved.connections <= 16:
                    self.connections = max(self.initial_connections, int(saved.connections))
                if saved.ip_family in ("ipv4", "ipv6", "auto"):
                    self.ip_family = saved.ip_family
                if saved.recovery_action:
                    self.last_action_taken = saved.recovery_action

                if saved.is_stale(now_epoch=self.now_epoch):
                    self.revalidation_pending = True
                    self.revalidation_passed = None
                    self.revalidation_samples.clear()
                else:
                    self.revalidation_pending = False
                    self.revalidation_passed = True

    @property
    def is_active(self) -> bool:
        return self.disabled_reason is None and not self.exhausted and self.mode != WatchdogMode.OFF

    def reset_speed_baseline(self) -> None:
        """Reset rolling speed samples, hysteresis counters, and tracking."""
        self.speed_samples.clear()
        self.low_speed_consecutive = 0
        self.sustained_low_speed_cycles = 0
        self.sustained_throttle_detected = False
        self.warm_up_complete = False

    def handle_resilience_event(
        self,
        event: ResilienceEvent,
        *,
        elapsed_sec: float = 0.0,
    ) -> WatchdogDecision:
        """Handle a WAKE, NET_CHANGE, or STALL event from ResilienceEventSource.

        Every event triggers the same action: recycle all connections, re-resolve DNS,
        reset the speed baseline, and log the reason.
        """
        if not self.supports_range:
            return WatchdogDecision(action=WatchdogActionType.NONE)

        self.reset_speed_baseline()
        self.cycle_count += 1
        self.last_recycle_elapsed = elapsed_sec
        self.last_action_taken = WatchdogActionType.RESILIENCE_RECYCLE.value

        logger.info("Watchdog resilience event [%s]: %s", event.event_type.value, event.reason)
        return WatchdogDecision(
            action=WatchdogActionType.RESILIENCE_RECYCLE,
            reason=f"[{event.event_type.value.upper()}] {event.reason}",
            threshold_bps=self.current_threshold_bps(),
            rolling_p90_bps=0.0,
            cycle=self.cycle_count,
            event_type=event.event_type.value,
        )

    def rolling_p90(self) -> float:
        positive = [s for s in self.speed_samples[-60:] if s > 0]
        if len(positive) < 3:
            return 0.0
        return compute_percentile(positive, 90.0)

    def current_threshold_bps(self) -> float:
        if self.mode == WatchdogMode.V1:
            return self.user_floor_bps
        p90 = self.rolling_p90()
        return max(self.user_floor_bps, 0.5 * p90)

    def status_label(self) -> str:
        if self.disabled_reason == "no_range_support":
            return "Watchdog: Off (200-only server)"
        if self.disabled_reason is not None or self.mode == WatchdogMode.OFF:
            return "Watchdog: Off"
        if self.revalidation_pending:
            return f"Watchdog {self.mode.value}: Re-validating profile"
        thresh = self.current_threshold_bps()
        return f"Watchdog {self.mode.value}: <{format_speed(thresh)} (cycles: {self.cycle_count}, {self.connections} streams)"

    def step(
        self,
        elapsed_sec: float,
        speed_bps: float,
        per_conn_speeds: list[float] | None = None,
        resilience_event: ResilienceEvent | None = None,
    ) -> WatchdogDecision:
        """Process one telemetry sample and return an instant WatchdogDecision."""
        if resilience_event is not None:
            return self.handle_resilience_event(resilience_event, elapsed_sec=elapsed_sec)

        if speed_bps > 0:
            self.speed_samples.append(speed_bps)

        if not self.is_active:
            return WatchdogDecision(action=WatchdogActionType.NONE)

        threshold = self.current_threshold_bps()
        p90 = self.rolling_p90()

        # Short ramp-up re-validation for profiles older than 30 days
        if self.revalidation_pending and self.applied_profile is not None:
            self.revalidation_samples.append(max(0.0, speed_bps))
            if len(self.revalidation_samples) >= max(1, int(self.revalidation_ticks)):
                self.revalidation_pending = False
                best_ramp = max(self.revalidation_samples) if self.revalidation_samples else 0.0
                prof = self.applied_profile
                expected_min = max(
                    self.user_floor_bps,
                    (prof.speed_after * 0.70) if prof.speed_after > 0 else self.user_floor_bps,
                )
                if best_ramp >= expected_min and best_ramp > 0:
                    self.revalidation_passed = True
                    now_ts = get_clock().now() if self.now_epoch is None else float(self.now_epoch)
                    HistoryManager.save_learned_profile(
                        LearnedProfile(
                            host=prof.host,
                            hour_bucket=prof.hour_bucket if prof.hour_bucket >= 0 else int(self.hour_bucket or 0),
                            network_id=prof.network_id or str(self.network_id or "default"),
                            connections=self.connections,
                            ip_family=self.ip_family,
                            recovery_action=self.last_action_taken,
                            speed_before=prof.speed_before,
                            speed_after=max(prof.speed_after, best_ramp),
                            supports_range=self.supports_range,
                            throttle_class=prof.throttle_class,
                            updated_epoch=now_ts,
                        )
                    )
                else:
                    self.revalidation_passed = False
                    self.connections = self.initial_connections
                    self.ip_family = "auto"
                    self.last_action_taken = WatchdogActionType.PAUSE_UNPAUSE.value
                    self.reset_speed_baseline()
                    self.cycle_count += 1
                    self.last_recycle_elapsed = elapsed_sec
                    return WatchdogDecision(
                        action=WatchdogActionType.ADJUST_CONNECTIONS,
                        reason=(
                            f"Stale profile (>30d) failed ramp-up re-validation "
                            f"({format_speed(best_ramp)} < {format_speed(expected_min)}); "
                            f"reverting to {self.connections} connections"
                        ),
                        new_connections=self.connections,
                        new_ip_family=self.ip_family,
                        threshold_bps=threshold,
                        rolling_p90_bps=p90,
                        cycle=self.cycle_count,
                    )

        # Dynamic warm-up check
        if speed_bps >= threshold:
            self.warm_up_complete = True

        # Post-recycle warm-up cooldown: grant grace for sockets to establish TCP handshake
        if (elapsed_sec - self.last_recycle_elapsed) < self.min_cooldown_sec:
            self.low_speed_consecutive = 0
            return WatchdogDecision(
                action=WatchdogActionType.NONE,
                threshold_bps=threshold,
                rolling_p90_bps=p90,
                cycle=self.cycle_count,
            )

        can_check = self.warm_up_complete or (elapsed_sec >= self.grace_period)
        if not can_check:
            return WatchdogDecision(
                action=WatchdogActionType.NONE,
                threshold_bps=threshold,
                rolling_p90_bps=p90,
                cycle=self.cycle_count,
            )

        # ------------------------------------------------------------------
        # Enhanced Watchdog: Strict 16-Channel Lock + Fast RPC Reset
        # ------------------------------------------------------------------
        if speed_bps < threshold:
            self.low_speed_consecutive += 1
            self.sustained_low_speed_cycles += 1
            if self.sustained_low_speed_cycles >= self.sustained_throttle_threshold:
                self.sustained_throttle_detected = True
            # Under sustained ISP throttling, reduce recycle frequency to avoid
            # tearing down working connections when the bandwidth cap is the
            # limiting factor, not a per-flow issue.
            recycle_interval = (
                self.sustained_throttle_threshold
                if self.sustained_throttle_detected
                else 2
            )
            if self.low_speed_consecutive >= recycle_interval:
                self.low_speed_consecutive = 0
                self.cycle_count += 1
                self.last_recycle_elapsed = elapsed_sec
                self.last_action_taken = WatchdogActionType.PAUSE_UNPAUSE.value

                # Save best performance profile in history
                if self.host:
                    now_ts = get_clock().now() if self.now_epoch is None else float(self.now_epoch)
                    HistoryManager.save_learned_profile(
                        LearnedProfile(
                            host=self.host,
                            hour_bucket=int(
                                self.hour_bucket if self.hour_bucket is not None else compute_hour_bucket()
                            ),
                            network_id=str(self.network_id or platform.get_network_id() or "default"),
                            connections=self.connections,
                            ip_family=self.ip_family,
                            recovery_action=WatchdogActionType.PAUSE_UNPAUSE.value,
                            speed_before=speed_bps,
                            speed_after=max(self.speed_samples) if self.speed_samples else speed_bps,
                            supports_range=self.supports_range,
                            updated_epoch=now_ts,
                        )
                    )

                return WatchdogDecision(
                    action=WatchdogActionType.PAUSE_UNPAUSE,
                    reason=(
                        f"Speed {format_speed(speed_bps)} < {format_speed(threshold)} floor; "
                        f"cycling {self.connections} RPC sockets"
                        + (" (sustained throttle)" if self.sustained_throttle_detected else "")
                    ),
                    threshold_bps=threshold,
                    rolling_p90_bps=p90,
                    cycle=self.cycle_count,
                    new_connections=self.connections,
                )
        else:
            self.low_speed_consecutive = 0
            # Speed recovered above threshold: clear sustained throttle detection
            if self.sustained_throttle_detected:
                self.sustained_low_speed_cycles = 0
                self.sustained_throttle_detected = False

        return WatchdogDecision(
            action=WatchdogActionType.NONE,
            threshold_bps=threshold,
            rolling_p90_bps=p90,
            cycle=self.cycle_count,
        )
