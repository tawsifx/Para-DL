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

    def test_no_range_support_disables(self) -> None:
        wd = WatchdogController(
            mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com", supports_range=False
        )
        self.assertFalse(wd.is_active)
        self.assertEqual(wd.disabled_reason, "no_range_support")

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
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        wd.speed_samples = [5000.0, 5000.0, 5000.0]  # warm up complete
        wd.warm_up_complete = True
        decision = wd.step(elapsed_sec=10.0, speed_bps=5000.0)
        self.assertEqual(decision.action, WatchdogActionType.NONE)

    def test_speed_below_threshold_triggers_recycle(self) -> None:
        wd = WatchdogController(mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com")
        wd.speed_samples = [5000.0, 5000.0, 5000.0]
        wd.warm_up_complete = True
        # First low sample: just increments counter, returns NONE
        decision = wd.step(elapsed_sec=10.0, speed_bps=500.0)
        self.assertEqual(decision.action, WatchdogActionType.NONE)
        self.assertEqual(wd.low_speed_consecutive, 1)

        # Second low sample: triggers PAUSE_UNPAUSE
        decision = wd.step(elapsed_sec=11.0, speed_bps=500.0)
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
            mode=WatchdogMode.V2, user_floor_bps=1000, host="example.com", supports_range=False
        )
        self.assertEqual(wd.status_label(), "Watchdog: Off (200-only server)")


if __name__ == "__main__":
    unittest.main()
