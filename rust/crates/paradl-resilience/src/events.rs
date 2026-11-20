//! Resilience event source: WAKE, NET_CHANGE, STALL.
//!
//! Faithful port of `src/paradl/resilience/events.py:34-225`. The source reads
//! time only through [`ResilienceClock`], emits at most one debounced event per
//! tick, and converts each observation into a typed `resilience_event` value
//! for the single emitter.
//!
//! ```
//! use paradl_resilience::events::{
//!     ManualClock, ResilienceConfig, ResilienceEventSource, ResilienceEventType,
//! };
//! let clock = ManualClock::new(1_700_000_000.0, 1000.0);
//! let mut source = ResilienceEventSource::new(ResilienceConfig::default(), clock.clone());
//! assert!(source.tick(0).is_none());
//! clock.jump_wall(10.0);
//! let event = source.tick(0).expect("wake");
//! assert_eq!(event.event_type, ResilienceEventType::Wake);
//! ```

use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use paradl_events::{Event, ResilienceEventEvent, py_round};
use serde_json::{Value, json};

pub use paradl_watchdog::{ResilienceEvent, ResilienceEventType};

use crate::{Emitted, now};

/// Injected time source for wake/stall detection.
pub trait ResilienceClock: Send + Sync {
    /// Wall-clock epoch seconds.
    fn wall(&self) -> f64;
    /// Monotonic seconds.
    fn mono(&self) -> f64;
    /// Simulate a sleep/wake wall-clock jump (no-op for real clocks).
    fn jump_wall(&self, _seconds: f64) {}
    /// Shift monotonic time (no-op for real clocks).
    fn advance_mono(&self, _seconds: f64) {}
}

/// Production clock backed by [`SystemTime`] and [`Instant`].
#[derive(Debug)]
pub struct SystemResilienceClock {
    origin: Instant,
    wall_origin: f64,
}

impl Default for SystemResilienceClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            wall_origin: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        }
    }
}

impl ResilienceClock for SystemResilienceClock {
    fn wall(&self) -> f64 {
        self.wall_origin + self.origin.elapsed().as_secs_f64()
    }

    fn mono(&self) -> f64 {
        self.origin.elapsed().as_secs_f64()
    }
}

/// Deterministic clock for tests and replay.
#[derive(Debug, Clone)]
pub struct ManualClock {
    wall: std::sync::Arc<Mutex<f64>>,
    mono: std::sync::Arc<Mutex<f64>>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

impl ManualClock {
    /// Construct a clock with explicit wall/monotonic readings.
    pub fn new(wall: f64, mono: f64) -> Self {
        Self {
            wall: std::sync::Arc::new(Mutex::new(wall)),
            mono: std::sync::Arc::new(Mutex::new(mono)),
        }
    }

    fn read(value: &Mutex<f64>) -> f64 {
        *value.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn shift(value: &Mutex<f64>, seconds: f64) {
        let mut guard = value.lock().unwrap_or_else(|error| error.into_inner());
        *guard += seconds;
    }

    /// Shift wall time forward by `seconds`.
    pub fn jump_wall(&self, seconds: f64) {
        Self::shift(&self.wall, seconds);
    }

    /// Shift monotonic time forward by `seconds`.
    pub fn advance_mono(&self, seconds: f64) {
        Self::shift(&self.mono, seconds);
    }
}

impl ResilienceClock for ManualClock {
    fn wall(&self) -> f64 {
        Self::read(&self.wall)
    }

    fn mono(&self) -> f64 {
        Self::read(&self.mono)
    }

    fn jump_wall(&self, seconds: f64) {
        ManualClock::jump_wall(self, seconds);
    }

    fn advance_mono(&self, seconds: f64) {
        ManualClock::advance_mono(self, seconds);
    }
}

/// Tunable thresholds for the event source (`resilience/events.py:43-64`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResilienceConfig {
    /// Whether network-change notifications are consumed.
    pub net_watch: bool,
    /// No-byte-progress stall timeout in seconds (`0` disables).
    pub stall_timeout_sec: f64,
    /// Wall-clock jump that counts as a wake.
    pub wake_jump_threshold_sec: f64,
    /// Minimum seconds between two emitted events.
    pub debounce_sec: f64,
    /// Whether wake detection is enabled.
    pub wake_watch: bool,
    /// Watcher backend name included in NET_CHANGE reasons.
    pub backend: String,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            net_watch: true,
            stall_timeout_sec: 30.0,
            wake_jump_threshold_sec: 3.0,
            debounce_sec: 2.0,
            wake_watch: true,
            backend: "scutil".to_string(),
        }
    }
}

/// One debounced observation ready for the watchdog and the emitter.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceEvent {
    /// Wake, network change, or stall.
    pub event_type: ResilienceEventType,
    /// Exact Python-format reason string.
    pub reason: String,
    /// Wall-clock time of the event.
    pub timestamp: f64,
    /// Rounded numeric detail keys.
    pub detail: Value,
    /// Typed event for `paradl_events::emit_jsonl`.
    pub emitted: Emitted,
}

/// Unified event source fed on every watchdog tick.
pub struct ResilienceEventSource<C: ResilienceClock> {
    config: ResilienceConfig,
    clock: C,
    last_mono: f64,
    last_wall: f64,
    last_progress_bytes: i64,
    last_progress_mono: f64,
    last_event_mono: f64,
    last_wake_mono: f64,
    last_net_change_mono: f64,
    last_stall_mono: f64,
    notifications: Mutex<Vec<String>>,
}

impl<C: ResilienceClock> ResilienceEventSource<C> {
    /// Construct a source, seeding all debounce timers in the past.
    pub fn new(config: ResilienceConfig, clock: C) -> Self {
        let now_m = clock.mono();
        let now_w = clock.wall();
        let past = now_m - (config.debounce_sec * 10.0).max(1000.0);
        Self {
            config,
            clock,
            last_mono: now_m,
            last_wall: now_w,
            last_progress_bytes: 0,
            last_progress_mono: now_m,
            last_event_mono: past,
            last_wake_mono: past,
            last_net_change_mono: past,
            last_stall_mono: past,
            notifications: Mutex::new(Vec::new()),
        }
    }

    /// Enqueue a raw network-change notification from the platform watcher.
    pub fn inject_net_change(&mut self, detail: impl Into<String>) {
        self.notifications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(detail.into());
    }

    /// Simulate a byte-progress stall of `seconds` seconds.
    pub fn simulate_stall(&mut self, seconds: f64) {
        self.last_progress_mono -= seconds;
    }

    /// Reset the stall baseline, e.g. after a connection recycle.
    pub fn reset_progress(&mut self, downloaded_bytes: Option<i64>) {
        self.last_progress_mono = self.clock.mono();
        if let Some(bytes) = downloaded_bytes {
            self.last_progress_bytes = self.last_progress_bytes.max(bytes);
        }
    }

    fn build(
        &self,
        event_type: ResilienceEventType,
        reason: String,
        detail: Value,
    ) -> SourceEvent {
        let timestamp = self.clock.wall();
        let event = ResilienceEventEvent::new(
            now(),
            event_type.as_str().to_string(),
            reason.clone(),
            detail.clone(),
        );
        SourceEvent {
            event_type,
            reason,
            timestamp,
            detail,
            emitted: Emitted::new(Event::ResilienceEvent(event)),
        }
    }

    /// Evaluate all three triggers and return at most one debounced event.
    pub fn tick(&mut self, downloaded_bytes: i64) -> Option<SourceEvent> {
        let now_m = self.clock.mono();
        let now_w = self.clock.wall();
        let mono_delta = (now_m - self.last_mono).max(0.0);
        let wall_delta = now_w - self.last_wall;
        self.last_mono = now_m;
        self.last_wall = now_w;

        if downloaded_bytes > self.last_progress_bytes {
            self.last_progress_bytes = downloaded_bytes;
            self.last_progress_mono = now_m;
        }

        // 1. WAKE: wall clock advanced further than monotonic time.
        let clock_jump = wall_delta - mono_delta;
        if self.config.wake_watch && clock_jump >= self.config.wake_jump_threshold_sec.max(0.5) {
            self.last_progress_mono = now_m;
            if now_m - self.last_wake_mono >= self.config.debounce_sec
                && now_m - self.last_event_mono >= self.config.debounce_sec
            {
                self.last_wake_mono = now_m;
                self.last_event_mono = now_m;
                return Some(self.build(
                    ResilienceEventType::Wake,
                    format!(
                        "System wake detected (wall-clock jumped +{:.1}s over monotonic)",
                        clock_jump
                    ),
                    json!({
                        "wall_delta": py_round(wall_delta, 3),
                        "mono_delta": py_round(mono_delta, 3),
                        "clock_jump": py_round(clock_jump, 3),
                    }),
                ));
            }
        }

        // 2. NET_CHANGE: drain and debounce watcher notifications.
        let mut notifications = self
            .notifications
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let raw_notes = std::mem::take(&mut *notifications);
        drop(notifications);
        if self.config.net_watch
            && !raw_notes.is_empty()
            && now_m - self.last_net_change_mono >= self.config.debounce_sec
            && now_m - self.last_event_mono >= self.config.debounce_sec
        {
            self.last_net_change_mono = now_m;
            self.last_event_mono = now_m;
            self.last_progress_mono = now_m;
            let count = raw_notes.len();
            return Some(self.build(
                ResilienceEventType::NetChange,
                format!(
                    "Network change detected via {} ({} notification(s))",
                    self.config.backend, count
                ),
                json!({
                    "backend": self.config.backend,
                    "count": count,
                    "notifications": raw_notes.iter().take(5).cloned().collect::<Vec<_>>(),
                }),
            ));
        }

        // 3. STALL: no byte progress for the configured timeout.
        if self.config.stall_timeout_sec > 0.0 {
            let stall_elapsed = now_m - self.last_progress_mono;
            if stall_elapsed >= self.config.stall_timeout_sec {
                self.last_progress_mono = now_m;
                if now_m - self.last_stall_mono >= self.config.debounce_sec
                    && now_m - self.last_event_mono >= self.config.debounce_sec
                {
                    self.last_stall_mono = now_m;
                    self.last_event_mono = now_m;
                    return Some(self.build(
                        ResilienceEventType::Stall,
                        format!(
                            "Transfer stalled: no byte progress for {:.1}s (stall timeout {:.1}s)",
                            stall_elapsed, self.config.stall_timeout_sec
                        ),
                        json!({
                            "stall_seconds": py_round(stall_elapsed, 3),
                            "stall_timeout_sec": self.config.stall_timeout_sec,
                            "downloaded_bytes": downloaded_bytes,
                        }),
                    ));
                }
            }
        }

        None
    }
}
