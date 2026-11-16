"""Unit tests for the anti-throttle Watchdog v1/v2 state machine."""

from __future__ import annotations

import unittest
from unittest.mock import MagicMock, patch

from paradl.engines.watchdog import (
    WatchdogActionType,
    WatchdogController,
    compute_percentile,
)
from paradl.models import WatchdogMode


class TestComputePercentile(unittest.TestCase):
    """Tests for compute_percentile() linear interpolation."""

    def test_single_sample(self) -> None:
        self.assertEqual(compute_percentile([100.0], 90), 100.0)

    def test_multiple_samples_p90(self) -> None:
        samples = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]
        # p90 of 10 items: k = 9 * 0.9 = 8.1, interpolate between s[8]=9 and s[9]=10
        result = compute_percentile(samples, 90)
        self.assertAlmostEqual(result, 9.1)

    def test_empty_list(self) -> None:
        self.assertEqual(compute_percentile([], 90), 0.0)


class TestWatchdogControllerInit(unittest.TestCase):
    """Tests for WatchdogController initialization and learned profile loading."""

    def test_defaults(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        self.assertEqual(wd.connections, 16)
        self.assertEqual(wd.mode, WatchdogMode.V2)
        self.assertEqual(wd.user_floor_bps, 1000)
        self.assertTrue(wd.is_active)
        self.assertFalse(wd.exhausted)
        self.assertIsNone(wd.disabled_reason)

    def test_watchdog_off_disables(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.OFF, user_floor_bps=1000, host="example.com")
        self.assertFalse(wd.is_active)
        self.assertEqual(wd.disabled_reason, "off")

    def test_zero_floor_disables(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=0, host="example.com")
        self.assertFalse(wd.is_active)
        self.assertEqual(wd.disabled_reason, "off")

    def test_no_range_support_stays_active(self) -> None:
        with patch("paradl.engines.watchdog.HistoryManager.get_best_profile") as mock_get:
            mock_get.return_value = MagicMock(
                connections=16,
                ip_family="ipv4",
                recovery_action="tier2_pause_unpause",
            )
            mock_get.return_value.is_stale.return_value = False
            wd = WatchdogController(
                mode=WatchdogMode.V2,
                user_floor_bps=1000,
                host="example.com",
                connections=16,
                supports_range=False,
                apply_learned_profile=True,
            )
        # Watchdog remains active in single-stream mode for telemetry and
        # status reporting, locked to 1 connection without socket cycling.
        self.assertTrue(wd.is_active)
        self.assertIsNone(wd.disabled_reason)
        self.assertEqual(wd.connections, 1)
        self.assertEqual(wd.initial_connections, 1)

    def test_connections_clamped(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2, user_floor_bps=1000, connections=32, host="example.com"
        )
        self.assertEqual(wd.connections, 16)
        self.assertEqual(wd.initial_connections, 16)

    def test_learned_profile_applies_connections(self) -> None:
        with patch("paradl.engines.watchdog.HistoryManager.get_best_profile") as mock_get:
            mock_get.return_value = MagicMock(
                connections=8,
                ip_family="ipv4",
                recovery_action="tier2_pause_unpause",
                is_stale_return=False,
            )
            mock_get.return_value.is_stale.return_value = False
            wd = WatchdogController(
                mode=WatchdogMode.V2,
                user_floor_bps=1000,
                host="example.com",
                connections=16,
                apply_learned_profile=True,
            )
            self.assertEqual(wd.connections, 16)  # max(initial=16, saved=8) = 16
            self.assertEqual(wd.ip_family, "ipv4")


class TestWatchdogThreshold(unittest.TestCase):
    """Tests for current_threshold_bps() and rolling_p90()."""

    def test_v1_threshold_equals_user_floor(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V1, user_floor_bps=5000, host="example.com")
        self.assertEqual(wd.current_threshold_bps(), 5000)

    def test_v2_threshold_without_samples(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        # No samples -> p90 = 0 -> threshold = max(floor, 0) = floor
        self.assertEqual(wd.current_threshold_bps(), 1000)

    def test_v2_threshold_with_samples(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        wd.speed_samples = [1000.0, 2000.0, 5000.0, 10000.0, 20000.0]
        # p90 of [1000, 2000, 5000, 10000, 20000] -> k = 4 * 0.9 = 3.6
        # interpolate between s[3]=10000 and s[4]=20000 -> 14000
        p90 = wd.rolling_p90()
        threshold = wd.current_threshold_bps()
        # threshold = max(1000, 0.5 * p90)
        self.assertEqual(threshold, max(1000, 0.5 * p90))


class TestWatchdogStep(unittest.TestCase):
    """Tests for WatchdogController.step() telemetry processing."""

    def test_speed_above_threshold_returns_none(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            recycle_interval_sec=0,
        )
        wd.speed_samples = [5000.0, 5000.0, 5000.0]  # warm up complete
        wd.warm_up_complete = True
        decision = wd.step(elapsed_sec=10.0, speed_bps=5000.0)
        self.assertEqual(decision.action, WatchdogActionType.NONE)

    def test_speed_below_threshold_triggers_recycle(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            recycle_interval_sec=0,
        )
        wd.speed_samples = [5000.0, 5000.0, 5000.0]
        wd.warm_up_complete = True
        # First four low samples: just increment counter, return NONE
        for i in range(4):
            decision = wd.step(elapsed_sec=10.0 + i, speed_bps=500.0)
            self.assertEqual(decision.action, WatchdogActionType.NONE)
        self.assertEqual(wd.low_speed_consecutive, 4)

        # Fifth low sample: triggers PAUSE_UNPAUSE
        decision = wd.step(elapsed_sec=14.0, speed_bps=500.0)
        self.assertEqual(decision.action, WatchdogActionType.PAUSE_UNPAUSE)
        self.assertEqual(wd.cycle_count, 1)

    def test_post_recycle_cooldown_returns_none(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com", min_cooldown_sec=5.0
        )
        wd.speed_samples = [5000.0, 5000.0, 5000.0]
        wd.warm_up_complete = True
        wd.last_recycle_elapsed = 0.0
        # Within cooldown period -> NONE even if speed is low
        decision = wd.step(elapsed_sec=2.0, speed_bps=500.0)
        self.assertEqual(decision.action, WatchdogActionType.NONE)
        self.assertEqual(wd.low_speed_consecutive, 0)  # reset during cooldown

    def test_resilience_event_triggers_recycle(self) -> None:
        from paradl.events import ResilienceEvent, ResilienceEventType

        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        event = ResilienceEvent(
            event_type=ResilienceEventType.STALL,
            reason="no progress for 30s",
            timestamp=100.0,
        )
        decision = wd.step(elapsed_sec=100.0, speed_bps=5000.0, resilience_event=event)
        self.assertEqual(decision.action, WatchdogActionType.RESILIENCE_RECYCLE)
        self.assertEqual(wd.cycle_count, 1)

    def test_not_active_returns_none(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.OFF, user_floor_bps=1000, host="example.com")
        decision = wd.step(elapsed_sec=10.0, speed_bps=100.0)
        self.assertEqual(decision.action, WatchdogActionType.NONE)

    def test_no_range_low_speed_does_not_recycle_socket(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            supports_range=False,
            apply_learned_profile=False,
        )
        wd.speed_samples = [5000.0, 5000.0, 5000.0]
        wd.warm_up_complete = True
        for tick in range(10, 15):
            decision = wd.step(elapsed_sec=float(tick), speed_bps=200.0)
            self.assertEqual(decision.action, WatchdogActionType.NONE)
        self.assertEqual(wd.cycle_count, 0)
        self.assertTrue(wd.is_active)

    def test_no_range_resilience_event_skips_socket_recycle(self) -> None:
        from paradl.events import ResilienceEvent, ResilienceEventType

        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            supports_range=False,
            apply_learned_profile=False,
        )
        event = ResilienceEvent(
            event_type=ResilienceEventType.STALL,
            reason="no progress for 30s",
            timestamp=100.0,
        )
        decision = wd.step(elapsed_sec=100.0, speed_bps=0.0, resilience_event=event)
        self.assertEqual(decision.action, WatchdogActionType.NONE)
        self.assertEqual(wd.cycle_count, 0)

    def test_relative_straggler_triggers_recycle(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=100_000,
            host="example.com",
            apply_learned_profile=False,
            recycle_interval_sec=0,
        )
        wd.speed_samples = [1_000_000.0, 1_000_000.0, 1_000_000.0]
        wd.warm_up_complete = True

        # Global speed is above floor (1 MB/s >= 500 KB/s p90/2), but one flow is < best/4
        conns = [500_000.0, 450_000.0, 50_000.0]
        d1 = wd.step(elapsed_sec=10.0, speed_bps=1_000_000.0, per_conn_speeds=conns)
        self.assertEqual(d1.action, WatchdogActionType.NONE)
        self.assertEqual(wd.straggler_consecutive, 1)

        d2 = wd.step(elapsed_sec=11.0, speed_bps=1_000_000.0, per_conn_speeds=conns)
        self.assertEqual(d2.action, WatchdogActionType.PAUSE_UNPAUSE)
        self.assertIn("Straggler connection", d2.reason)
        self.assertEqual(wd.cycle_count, 1)

    def test_periodic_5s_recycle_on_206_range_default(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            supports_range=True,
            apply_learned_profile=False,
        )
        self.assertEqual(wd.recycle_interval_sec, 5.0)

        # Even with speed above threshold, cycles repeat every 5s
        for t in (1.0, 2.0, 3.0, 4.0):
            d = wd.step(elapsed_sec=t, speed_bps=5000.0)
            self.assertEqual(d.action, WatchdogActionType.NONE)

        d_5 = wd.step(elapsed_sec=5.0, speed_bps=5000.0)
        self.assertEqual(d_5.action, WatchdogActionType.PAUSE_UNPAUSE)
        self.assertEqual(wd.cycle_count, 1)

        for t in (6.0, 7.0, 8.0, 9.0):
            d = wd.step(elapsed_sec=t, speed_bps=5000.0)
            self.assertEqual(d.action, WatchdogActionType.NONE)

        d_10 = wd.step(elapsed_sec=10.0, speed_bps=5000.0)
        self.assertEqual(d_10.action, WatchdogActionType.PAUSE_UNPAUSE)
        self.assertEqual(wd.cycle_count, 2)

    def test_periodic_5s_recycle_skipped_without_range(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            supports_range=False,
            apply_learned_profile=False,
        )
        for t in (5.0, 10.0, 15.0):
            d = wd.step(elapsed_sec=t, speed_bps=5000.0)
            self.assertEqual(d.action, WatchdogActionType.NONE)
        self.assertEqual(wd.cycle_count, 0)


class TestWatchdogStatusLabel(unittest.TestCase):
    """Tests for WatchdogController.status_label()."""

    def test_active_label(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        label = wd.status_label()
        self.assertIn("Watchdog v2", label)
        self.assertIn("16 streams", label)

    def test_off_label(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.OFF, user_floor_bps=1000, host="example.com")
        self.assertEqual(wd.status_label(), "Watchdog: Off")

    def test_no_range_label(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2,
            user_floor_bps=1000,
            host="example.com",
            supports_range=False,
            connections=1,
        )
        label = wd.status_label()
        self.assertIn("Watchdog v2", label)
        self.assertIn("single stream", label)
        self.assertIn("1 streams", label)


class TestLearnedProfileSpeedAfter(unittest.TestCase):
    """Regression: a recycle must persist post-action throughput, not the session peak."""

    def test_recycle_records_post_action_speed_not_session_peak(self) -> None:
        saved = []
        with (
            patch(
                "paradl.engines.watchdog.HistoryManager.save_learned_profile",
                side_effect=saved.append,
            ),
            patch(
                "paradl.engines.watchdog.HistoryManager.get_best_profile",
                return_value=None,
            ),
        ):
            wd = WatchdogController(
                mode=WatchdogMode.V2,
                user_floor_bps=100_000,
                host="cdn.example.com",
                supports_range=True,
                apply_learned_profile=False,
                recycle_interval_sec=5.0,
                now_epoch=1000.0,
            )
            # Early healthy burst, then a collapse that triggers the periodic cycle.
            for t in (1.0, 2.0, 3.0, 4.0):
                wd.step(elapsed_sec=t, speed_bps=10_000_000.0)
            decision = wd.step(elapsed_sec=5.0, speed_bps=120_000.0)

        self.assertEqual(decision.action, WatchdogActionType.PAUSE_UNPAUSE)
        self.assertTrue(saved, "recycle must persist a learned profile")
        profile = saved[-1]
        self.assertEqual(profile.speed_before, 120_000)
        self.assertLessEqual(
            profile.speed_after,
            500_000,
            f"speed_after must reflect post-action speed, got {profile.speed_after}",
        )

    def test_second_recycle_uses_samples_since_first_recycle(self) -> None:
        saved = []
        with (
            patch(
                "paradl.engines.watchdog.HistoryManager.save_learned_profile",
                side_effect=saved.append,
            ),
            patch(
                "paradl.engines.watchdog.HistoryManager.get_best_profile",
                return_value=None,
            ),
        ):
            wd = WatchdogController(
                mode=WatchdogMode.V2,
                user_floor_bps=100_000,
                host="cdn.example.com",
                supports_range=True,
                apply_learned_profile=False,
                recycle_interval_sec=5.0,
                now_epoch=1000.0,
            )
            for t in (1.0, 2.0, 3.0, 4.0):
                wd.step(elapsed_sec=t, speed_bps=10_000_000.0)
            wd.step(elapsed_sec=5.0, speed_bps=120_000.0)  # first recycle
            # Post-recycle throughput settles at 200 kB/s.
            for t in (6.0, 7.0, 8.0, 9.0):
                wd.step(elapsed_sec=t, speed_bps=200_000.0)
            wd.step(elapsed_sec=10.0, speed_bps=200_000.0)  # second recycle

        self.assertEqual(len(saved), 2)
        self.assertLessEqual(saved[0].speed_after, 500_000)
        self.assertLessEqual(saved[1].speed_after, 500_000)


if __name__ == "__main__":
    unittest.main()
