//! Clock abstraction for deterministic time and zero-sleep testing.
//!
//! Port of the `paradl.core.clock` protocol. The authoritative Rust trait
//! signature is `fn now(&self) -> std::time::SystemTime` (framework charter §2).
//! [`FakeClock`] additionally exposes `monotonic`, `sleep`, and `jump_wall`
//! helpers so scenario tests can advance time without real sleeps.

use std::time::{Duration, SystemTime};

/// Abstract clock interface provided by every clock implementation.
pub trait Clock {
    /// Return the current wall-clock time.
    fn now(&self) -> SystemTime;
}

/// Production clock backed by [`SystemTime::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Deterministic in-memory clock for instant scenario testing.
///
/// Defaults mirror Python's `FakeClock`: wall time `1_700_000_000.0` seconds
/// since the epoch and monotonic time `1000.0` seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct FakeClock {
    wall: f64,
    mono: f64,
    sleeps: Vec<f64>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            wall: 1_700_000_000.0,
            mono: 1000.0,
            sleeps: Vec::new(),
        }
    }
}

impl FakeClock {
    /// Construct a fake clock with explicit initial wall/monotonic times.
    pub fn new(initial_time: f64, initial_mono: f64) -> Self {
        Self {
            wall: initial_time,
            mono: initial_mono,
            sleeps: Vec::new(),
        }
    }

    /// Return monotonic time in fractional seconds.
    pub fn monotonic(&self) -> f64 {
        self.mono
    }

    /// Advance both wall and monotonic time by `seconds` and record the sleep.
    pub fn sleep(&mut self, seconds: f64) {
        let duration = seconds.max(0.0);
        self.sleeps.push(duration);
        self.wall += duration;
        self.mono += duration;
    }

    /// Recorded sleep durations, mirroring Python's `sleep_history`.
    pub fn sleep_history(&self) -> &[f64] {
        &self.sleeps
    }

    /// Explicitly step time forward by `seconds` (alias for [`FakeClock::sleep`]).
    pub fn advance(&mut self, seconds: f64) {
        self.sleep(seconds);
    }

    /// Simulate a wall-clock jump without advancing monotonic time.
    pub fn jump_wall(&mut self, seconds: f64) {
        self.wall += seconds;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs_f64(self.wall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_clamps_negative_durations() {
        let mut clock = FakeClock::default();
        clock.sleep(-1.0);
        assert_eq!(clock.sleep_history(), &[0.0]);
        assert_eq!(clock.monotonic(), 1000.0);
    }
}
