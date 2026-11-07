"""Clock protocol and implementations for deterministic time and zero-sleep testing."""

from __future__ import annotations

import time
from typing import Protocol, runtime_checkable


@runtime_checkable
class Clock(Protocol):
    """Abstract clock interface providing wall-clock, monotonic time, and sleep."""

    def now(self) -> float:
        """Return the current POSIX timestamp (seconds since epoch)."""
        ...

    def monotonic(self) -> float:
        """Return monotonic time in fractional seconds."""
        ...

    def sleep(self, seconds: float) -> None:
        """Suspend execution for the given duration (or advance fake time)."""
        ...


class SystemClock:
    """Production clock implementation backed by standard library `time`."""

    def now(self) -> float:
        return time.time()

    def monotonic(self) -> float:
        return time.monotonic()

    def sleep(self, seconds: float) -> None:
        if seconds > 0:
            time.sleep(seconds)


class FakeClock:
    """Deterministic in-memory fake clock for instant scenario testing without real sleeps."""

    def __init__(self, initial_time: float = 1700000000.0, initial_mono: float = 1000.0) -> None:
        self._time: float = float(initial_time)
        self._mono: float = float(initial_mono)
        self.sleep_history: list[float] = []

    def now(self) -> float:
        return self._time

    def monotonic(self) -> float:
        return self._mono

    def sleep(self, seconds: float) -> None:
        dur = max(0.0, float(seconds))
        self.sleep_history.append(dur)
        self._time += dur
        self._mono += dur

    def advance(self, seconds: float) -> None:
        """Explicitly step time forward by the given duration."""
        self.sleep(seconds)

    def jump_wall(self, seconds: float) -> None:
        """Simulate a wall-clock jump (e.g. system wake/sleep) without advancing monotonic time."""
        self._time += seconds


_DEFAULT_CLOCK: Clock = SystemClock()


def get_clock() -> Clock:
    """Return the currently active system or fake clock."""
    return _DEFAULT_CLOCK


def set_clock(clock: Clock) -> None:
    """Override the active clock (primarily for test harnesses)."""
    global _DEFAULT_CLOCK
    _DEFAULT_CLOCK = clock


def reset_clock() -> None:
    """Reset the active clock to the default SystemClock."""
    global _DEFAULT_CLOCK
    _DEFAULT_CLOCK = SystemClock()
