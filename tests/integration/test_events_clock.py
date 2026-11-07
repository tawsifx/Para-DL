"""Integration tests: ResilienceEventSource state machine (WAKE, NET_CHANGE, STALL) using FakeClock."""

from __future__ import annotations

import pytest

from pdl.clock import FakeClock, set_clock
from pdl.events import ResilienceEventSource, ResilienceEventType

pytestmark = pytest.mark.integration


def test_wake_jump_detection_via_fake_clock() -> None:
    """Simulated wall-clock jump triggers exactly one debounced WAKE event."""
    clock = FakeClock(initial_time=1700000000.0, initial_mono=1000.0)
    set_clock(clock)

    ev_src = ResilienceEventSource(
        clock=clock,
        net_watch=False,
        wake_watch=True,
        wake_jump_threshold_sec=3.0,
        debounce_sec=2.0,
    )

    events = ev_src.tick(downloaded_bytes=100)
    assert events is None

    # Advance wall clock by 45s without monotonic jump (simulates system sleep/wake)
    clock.jump_wall(45.0)
    event = ev_src.tick(downloaded_bytes=200)
    assert event is not None
    assert event.event_type == ResilienceEventType.WAKE
    assert "System wake" in event.reason

    # Second jump within debounce window must be debounced (no second event)
    clock.jump_wall(15.0)
    event_debounced = ev_src.tick(downloaded_bytes=300)
    assert event_debounced is None


def test_stall_timeout_detection_via_fake_clock() -> None:
    """Byte progress stalling triggers exactly one debounced STALL event."""
    clock = FakeClock(initial_time=1700000000.0, initial_mono=1000.0)
    set_clock(clock)

    ev_src = ResilienceEventSource(
        clock=clock,
        stall_timeout_sec=10.0,
        debounce_sec=2.0,
        wake_watch=False,
        net_watch=False,
    )

    event = ev_src.tick(downloaded_bytes=1000)
    assert event is None

    # Advance clock by 5s (half of stall timeout)
    clock.advance(5.0)
    event = ev_src.tick(downloaded_bytes=1000)
    assert event is None

    # Advance clock past 10s stall timeout with zero byte progress
    clock.advance(6.0)
    event = ev_src.tick(downloaded_bytes=1000)
    assert event is not None
    assert event.event_type == ResilienceEventType.STALL
    assert "no byte progress for" in event.reason


def test_net_change_burst_debouncing() -> None:
    """Multiple rapid network change notifications trigger only 1 event within debounce window."""
    clock = FakeClock(initial_time=1700000000.0, initial_mono=1000.0)
    set_clock(clock)

    ev_src = ResilienceEventSource(
        clock=clock,
        net_watch=True,
        debounce_sec=2.0,
    )

    ev_src.simulate_net_change("IPv4 address changed")
    clock.advance(0.1)
    ev_src.simulate_net_change("Wi-Fi SSID changed")

    event = ev_src.tick(downloaded_bytes=500)
    assert event is not None
    assert event.event_type == ResilienceEventType.NET_CHANGE

    # Next immediate poll has zero events
    assert ev_src.tick(downloaded_bytes=600) is None
