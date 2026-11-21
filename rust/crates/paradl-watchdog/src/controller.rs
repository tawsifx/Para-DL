//! The v1/v2 watchdog state machine and its supervision loop.
//!
//! Direct port of `WatchdogController.step` in
//! `src/paradl/engines/watchdog.py`. Time is always obtained from the injected
//! [`Clock`]; the controller never calls real time functions itself.

use std::sync::Arc;
use std::time::SystemTime;

use paradl_core::{Clock, SystemClock, WatchdogMode, format_speed};

use crate::profile::{InMemoryProfileStore, LearnedProfile, ProfileStore};
use crate::types::{
    ResilienceEvent, ThroughputSample, WatchdogAction, WatchdogActionType, WatchdogState,
    compute_percentile, py_round,
};

/// Staleness threshold for learned profiles (30 days, in seconds).
pub const STALE_PROFILE_SECONDS: f64 = 2_592_000.0;

/// Configuration for a [`WatchdogController`].
#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    /// Watchdog mode (`v1`, `v2`, or `off`).
    pub mode: WatchdogMode,
    /// User speed floor in bytes per second; `<= 0` disables the controller.
    pub user_floor_bps: f64,
    /// Samples required before enforcing the adaptive threshold (minimum 2).
    pub grace_period: i32,
    /// Requested parallel connections; clamped into `1..=16`.
    pub connections: i32,
    /// Host name used to load and persist learned profiles.
    pub host: String,
    /// Whether the transfer supports HTTP 206 Partial Content.
    pub supports_range: bool,
    /// Minimum seconds between two socket recycles.
    pub min_cooldown_sec: f64,
    /// Periodic recycle interval in seconds (`0` disables periodic cycling).
    pub recycle_interval_sec: f64,
    /// Reserved recycle budget for tier escalation.
    pub max_recycles: i64,
    /// 4-hour bucket for learned profiles (`0..=5`, from `compute_hour_bucket`).
    ///
    /// `None` resolves to the sentinel `-1`, meaning **unknown** — callers MUST
    /// pass `Some(compute_hour_bucket(local_hour))` when profiles are in use.
    /// `0` is a real bucket (midnight–04:00), NOT a safe default.
    pub hour_bucket: Option<i32>,
    /// Network id; when `None`/empty the controller uses `"default"`.
    pub network_id: Option<String>,
    /// Whether to load a persisted learned profile at construction.
    pub apply_learned_profile: bool,
    /// Samples used for stale-profile ramp-up re-validation.
    pub revalidation_ticks: i32,
    /// Low-speed cycles before sustained-throttle detection is recorded.
    pub sustained_throttle_threshold: i32,
    /// Ratio at which one flow is considered a straggler versus the best flow.
    pub straggler_ratio: f64,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            mode: WatchdogMode::V2,
            user_floor_bps: 0.0,
            grace_period: 5,
            connections: 16,
            host: String::new(),
            supports_range: true,
            min_cooldown_sec: 2.0,
            recycle_interval_sec: 5.0,
            max_recycles: 999_999,
            // Unknown-bucket sentinel: -1 never collides with a real 0..=5
            // bucket. Callers must supply Some(compute_hour_bucket(local_hour)).
            hour_bucket: None,
            network_id: None,
            apply_learned_profile: true,
            revalidation_ticks: 2,
            sustained_throttle_threshold: 10,
            straggler_ratio: 4.0,
        }
    }
}

/// Stateful watchdog controller implementing v1 and v2 policies.
pub struct WatchdogController {
    mode: WatchdogMode,
    user_floor_bps: f64,
    grace_period: i32,
    min_cooldown_sec: f64,
    recycle_interval_sec: f64,
    initial_connections: i32,
    connections: i32,
    host: String,
    supports_range: bool,
    // -1 = unknown sentinel; always 0..=5 when a caller supplies a real bucket.
    hour_bucket: i32,
    network_id: String,
    revalidation_ticks: i32,
    sustained_throttle_threshold: i32,
    straggler_ratio: f64,

    cycle_count: i64,
    warm_up_complete: bool,
    low_speed_consecutive: i32,
    straggler_consecutive: i32,
    post_recycle_samples: Vec<f64>,
    sustained_low_speed_cycles: i32,
    sustained_throttle_detected: bool,
    speed_samples: Vec<f64>,
    last_recycle_elapsed: f64,
    last_action_taken: String,
    ip_family: String,
    disabled_reason: Option<String>,
    exhausted: bool,

    applied_profile: Option<LearnedProfile>,
    revalidation_pending: bool,
    revalidation_passed: Option<bool>,
    revalidation_samples: Vec<f64>,

    clock: Arc<dyn Clock + Send + Sync>,
    store: Arc<dyn ProfileStore>,
}

impl std::fmt::Debug for WatchdogController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Trait objects are opaque; omit them from the debug snapshot.
        f.debug_struct("WatchdogController")
            .field("mode", &self.mode)
            .field("user_floor_bps", &self.user_floor_bps)
            .field("connections", &self.connections)
            .field("cycle_count", &self.cycle_count)
            .field("disabled_reason", &self.disabled_reason)
            .finish_non_exhaustive()
    }
}

impl WatchdogController {
    /// Build a controller with the production clock and in-memory profile store.
    pub fn new(config: WatchdogConfig) -> Self {
        Self::with_store(
            config,
            Arc::new(SystemClock),
            Arc::new(InMemoryProfileStore::new()),
        )
    }

    /// Build a controller with an injected clock (no direct real-time reads).
    pub fn with_clock(config: WatchdogConfig, clock: Arc<dyn Clock + Send + Sync>) -> Self {
        Self::with_store(config, clock, Arc::new(InMemoryProfileStore::new()))
    }

    /// Build a controller with an injected clock and learned-profile store.
    pub fn with_store(
        config: WatchdogConfig,
        clock: Arc<dyn Clock + Send + Sync>,
        store: Arc<dyn ProfileStore>,
    ) -> Self {
        let grace_period = config.grace_period.max(2);
        let initial_connections = config.connections.clamp(1, 16);
        let network_id = config
            .network_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| "default".to_string());
        // -1 is the "unknown" sentinel; 0 is a real bucket and must not be
        // silently substituted for a missing local-hour value.
        let hour_bucket = config.hour_bucket.unwrap_or(-1);
        let disabled_reason = if config.mode == WatchdogMode::Off || config.user_floor_bps <= 0.0 {
            Some("off".to_string())
        } else {
            None
        };

        let mut controller = Self {
            mode: config.mode,
            user_floor_bps: config.user_floor_bps,
            grace_period,
            min_cooldown_sec: config.min_cooldown_sec,
            recycle_interval_sec: config.recycle_interval_sec,
            initial_connections,
            connections: initial_connections,
            host: config.host.clone(),
            supports_range: config.supports_range,
            hour_bucket,
            network_id: network_id.clone(),
            revalidation_ticks: config.revalidation_ticks,
            sustained_throttle_threshold: config.sustained_throttle_threshold,
            straggler_ratio: config.straggler_ratio,
            cycle_count: 0,
            warm_up_complete: false,
            low_speed_consecutive: 0,
            straggler_consecutive: 0,
            post_recycle_samples: Vec::new(),
            sustained_low_speed_cycles: 0,
            sustained_throttle_detected: false,
            speed_samples: Vec::new(),
            last_recycle_elapsed: -999.0,
            last_action_taken: WatchdogActionType::PauseUnpause.as_str().to_string(),
            ip_family: "auto".to_string(),
            disabled_reason,
            exhausted: false,
            applied_profile: None,
            revalidation_pending: false,
            revalidation_passed: None,
            revalidation_samples: Vec::new(),
            clock,
            store,
        };

        if !controller.supports_range {
            // Non-Range servers reject mid-stream resume; keep the watchdog
            // active for telemetry but lock it to a single stream.
            controller.initial_connections = 1;
            controller.connections = 1;
        }

        if controller.disabled_reason.is_none()
            && !config.host.is_empty()
            && config.apply_learned_profile
        {
            let now = controller.now_epoch();
            if let Some(saved) = controller
                .store
                .get_best_profile(&config.host, hour_bucket, &network_id)
            {
                if controller.supports_range && (1..=16).contains(&saved.connections) {
                    controller.connections = controller.initial_connections.max(saved.connections);
                }
                if matches!(saved.ip_family.as_str(), "ipv4" | "ipv6" | "auto") {
                    controller.ip_family = saved.ip_family.clone();
                }
                if !saved.recovery_action.is_empty() {
                    controller.last_action_taken = saved.recovery_action.clone();
                }
                if saved.is_stale(now, STALE_PROFILE_SECONDS) {
                    controller.revalidation_pending = true;
                    controller.revalidation_passed = None;
                    controller.revalidation_samples.clear();
                } else {
                    controller.revalidation_pending = false;
                    controller.revalidation_passed = Some(true);
                }
                controller.applied_profile = Some(saved);
            }
        }

        controller
    }

    /// Return the injected clock's wall time as epoch seconds.
    fn now_epoch(&self) -> f64 {
        self.clock
            .now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Whether the controller will act on samples.
    pub fn is_active(&self) -> bool {
        self.disabled_reason.is_none() && !self.exhausted && self.mode != WatchdogMode::Off
    }

    /// Reset rolling samples and hysteresis counters after a topology change.
    pub fn reset_speed_baseline(&mut self) {
        self.speed_samples.clear();
        self.post_recycle_samples.clear();
        self.low_speed_consecutive = 0;
        self.straggler_consecutive = 0;
        self.sustained_low_speed_cycles = 0;
        self.sustained_throttle_detected = false;
        self.warm_up_complete = false;
    }

    /// Rolling p90 (last 60 positive samples), or `0.0` with fewer than 3.
    pub fn rolling_p90(&self) -> f64 {
        self.rolling_percentile(90.0)
    }

    /// Rolling percentile (p90 / p95) over the last 60 positive samples.
    pub fn rolling_percentile(&self, pct: f64) -> f64 {
        let positive: Vec<f64> = self
            .speed_samples
            .iter()
            .rev()
            .take(60)
            .copied()
            .filter(|s| *s > 0.0)
            .collect();
        if positive.len() < 3 {
            return 0.0;
        }
        compute_percentile(&positive, pct)
    }

    /// Effective threshold: the user floor for v1, otherwise
    /// `max(user_floor, 0.5 * rolling_p90)`.
    pub fn current_threshold_bps(&self) -> f64 {
        if self.mode == WatchdogMode::V1 {
            return self.user_floor_bps;
        }
        self.user_floor_bps.max(0.5 * self.rolling_p90())
    }

    /// Python-compatible human-readable status label.
    pub fn status_label(&self) -> String {
        if self.disabled_reason.is_some() || self.mode == WatchdogMode::Off {
            return "Watchdog: Off".to_string();
        }
        if self.revalidation_pending {
            return format!("Watchdog {}: Re-validating profile", self.mode.as_str());
        }
        let suffix = if self.supports_range { "" } else { " (single stream)" };
        format!(
            "Watchdog {}: <{} (cycles: {}, {} streams){}",
            self.mode.as_str(),
            format_speed(self.current_threshold_bps()),
            self.cycle_count,
            self.connections,
            suffix,
        )
    }

    /// Build a fully-contextualized action at this tick.
    fn action(
        &self,
        action: WatchdogActionType,
        threshold: f64,
        p90: f64,
        cycle: i64,
        speed: f64,
        event_type: Option<String>,
    ) -> WatchdogAction {
        let mut result = WatchdogAction::new(action);
        result.threshold_bps = threshold;
        result.rolling_p90_bps = p90;
        result.cycle = cycle;
        result.speed_bps = speed;
        result.connections = self.connections;
        result.mode = self.mode;
        result.timestamp = py_round(self.now_epoch(), 3);
        result.event_type = event_type;
        result
    }

    /// Record a pause/unpause recycle and persist the learned profile.
    fn trigger_pause_unpause(
        &mut self,
        elapsed: f64,
        speed: f64,
        threshold: f64,
        p90: f64,
        reason: String,
    ) -> WatchdogAction {
        self.low_speed_consecutive = 0;
        self.straggler_consecutive = 0;
        self.cycle_count += 1;
        let post_peak = if self.post_recycle_samples.is_empty() {
            speed
        } else {
            self.post_recycle_samples
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max)
        };
        self.post_recycle_samples.clear();
        self.last_recycle_elapsed = elapsed;
        self.last_action_taken = WatchdogActionType::PauseUnpause.as_str().to_string();

        if !self.host.is_empty() {
            let profile = LearnedProfile {
                host: self.host.clone(),
                hour_bucket: self.hour_bucket,
                network_id: self.network_id.clone(),
                connections: self.connections,
                ip_family: self.ip_family.clone(),
                recovery_action: WatchdogActionType::PauseUnpause.as_str().to_string(),
                speed_before: speed,
                speed_after: post_peak,
                supports_range: self.supports_range,
                updated_epoch: self.now_epoch(),
                ..LearnedProfile::default()
            };
            self.store.save_learned_profile(profile);
        }

        let mut result = self.action(
            WatchdogActionType::PauseUnpause,
            threshold,
            p90,
            self.cycle_count,
            speed,
            None,
        );
        result.reason = reason;
        result.new_connections = Some(self.connections);
        result
    }

    /// Handle a WAKE / NET_CHANGE / STALL event.
    fn handle_resilience_event(
        &mut self,
        event: &ResilienceEvent,
        elapsed: f64,
        speed: f64,
    ) -> WatchdogAction {
        if !self.is_active() {
            return self.action(WatchdogActionType::None, 0.0, 0.0, 0, speed, None);
        }
        if elapsed - self.last_recycle_elapsed < self.min_cooldown_sec {
            return self.action(WatchdogActionType::None, 0.0, 0.0, 0, speed, None);
        }
        self.reset_speed_baseline();
        let event_type = Some(event.event_type.as_str().to_string());
        if !self.supports_range {
            let threshold = self.current_threshold_bps();
            return self.action(
                WatchdogActionType::None,
                threshold,
                0.0,
                self.cycle_count,
                speed,
                event_type,
            );
        }
        self.cycle_count += 1;
        self.last_recycle_elapsed = elapsed;
        self.last_action_taken = WatchdogActionType::ResilienceRecycle
            .as_str()
            .to_string();
        let threshold = self.current_threshold_bps();
        let mut result = self.action(
            WatchdogActionType::ResilienceRecycle,
            threshold,
            0.0,
            self.cycle_count,
            speed,
            event_type,
        );
        result.reason = format!(
            "[{}] {}",
            event.event_type.as_str().to_uppercase(),
            event.reason
        );
        result
    }

    /// Process one telemetry tick and return the resulting action.
    pub fn observe(&mut self, sample: ThroughputSample) -> WatchdogAction {
        self.step(&sample)
    }

    /// Snapshot the controller's runtime state.
    pub fn state(&self) -> WatchdogState {
        WatchdogState {
            mode: self.mode,
            user_floor_bps: self.user_floor_bps,
            connections: self.connections,
            initial_connections: self.initial_connections,
            is_active: self.is_active(),
            exhausted: self.exhausted,
            disabled_reason: self.disabled_reason.clone(),
            cycle_count: self.cycle_count,
            warm_up_complete: self.warm_up_complete,
            low_speed_consecutive: self.low_speed_consecutive,
            straggler_consecutive: self.straggler_consecutive,
            current_threshold_bps: self.current_threshold_bps(),
            rolling_p90_bps: self.rolling_p90(),
            ip_family: self.ip_family.clone(),
            hour_bucket: self.hour_bucket,
            recycle_interval_sec: self.recycle_interval_sec,
            status_label: self.status_label(),
        }
    }

    /// Supervision loop: process an ordered stream of telemetry samples and
    /// return one action per sample, stamping every action with the injected
    /// clock (never with real time).
    pub fn supervise<I>(&mut self, samples: I) -> Vec<WatchdogAction>
    where
        I: IntoIterator<Item = ThroughputSample>,
    {
        samples.into_iter().map(|sample| self.observe(sample)).collect()
    }

    fn step(&mut self, sample: &ThroughputSample) -> WatchdogAction {
        if let Some(event) = &sample.resilience_event {
            return self.handle_resilience_event(event, sample.elapsed_sec, sample.speed_bps);
        }

        let speed = sample.speed_bps;
        let elapsed = sample.elapsed_sec;

        if speed > 0.0 {
            self.speed_samples.push(speed);
            if self.last_recycle_elapsed >= 0.0 && elapsed > self.last_recycle_elapsed {
                self.post_recycle_samples.push(speed);
            }
        }

        if !self.is_active() {
            return self.action(WatchdogActionType::None, 0.0, 0.0, 0, speed, None);
        }

        let threshold = self.current_threshold_bps();
        let p90 = self.rolling_p90();

        // Short ramp-up re-validation for profiles older than 30 days.
        if self.revalidation_pending && self.applied_profile.is_some() {
            self.revalidation_samples.push(speed.max(0.0));
            if self.revalidation_samples.len() >= self.revalidation_ticks.max(1) as usize {
                self.revalidation_pending = false;
                let best_ramp = self
                    .revalidation_samples
                    .iter()
                    .copied()
                    .fold(0.0_f64, f64::max);
                let profile = self.applied_profile.clone().unwrap();
                let expected_min = self
                    .user_floor_bps
                    .max(if profile.speed_after > 0.0 {
                        profile.speed_after * 0.70
                    } else {
                        self.user_floor_bps
                    });
                if best_ramp >= expected_min && best_ramp > 0.0 {
                    self.revalidation_passed = Some(true);
                    let refreshed = LearnedProfile {
                        host: profile.host.clone(),
                        hour_bucket: if profile.hour_bucket >= 0 {
                            profile.hour_bucket
                        } else {
                            self.hour_bucket
                        },
                        network_id: if profile.network_id.is_empty() {
                            self.network_id.clone()
                        } else {
                            profile.network_id.clone()
                        },
                        connections: self.connections,
                        ip_family: self.ip_family.clone(),
                        recovery_action: self.last_action_taken.clone(),
                        speed_before: profile.speed_before,
                        speed_after: profile.speed_after.max(best_ramp),
                        supports_range: self.supports_range,
                        throttle_class: profile.throttle_class.clone(),
                        updated_epoch: self.now_epoch(),
                        ..LearnedProfile::default()
                    };
                    self.store.save_learned_profile(refreshed);
                } else {
                    self.revalidation_passed = Some(false);
                    self.connections = self.initial_connections;
                    self.ip_family = "auto".to_string();
                    self.last_action_taken = WatchdogActionType::PauseUnpause
                        .as_str()
                        .to_string();
                    self.reset_speed_baseline();
                    if !self.supports_range {
                        return self.action(
                            WatchdogActionType::None,
                            threshold,
                            p90,
                            self.cycle_count,
                            speed,
                            None,
                        );
                    }
                    self.cycle_count += 1;
                    self.last_recycle_elapsed = elapsed;
                    let mut result = self.action(
                        WatchdogActionType::AdjustConnections,
                        threshold,
                        p90,
                        self.cycle_count,
                        speed,
                        None,
                    );
                    result.reason = format!(
                        "Stale profile (>30d) failed ramp-up re-validation ({} < {}); reverting to {} connections",
                        format_speed(best_ramp),
                        format_speed(expected_min),
                        self.connections,
                    );
                    result.new_connections = Some(self.connections);
                    result.new_ip_family = Some(self.ip_family.clone());
                    return result;
                }
            }
        }

        if speed >= threshold {
            self.warm_up_complete = true;
        }

        // Post-recycle warm-up: allow TCP handshakes to establish.
        if elapsed - self.last_recycle_elapsed < self.min_cooldown_sec {
            self.low_speed_consecutive = 0;
            self.straggler_consecutive = 0;
            return self.action(
                WatchdogActionType::None,
                threshold,
                p90,
                self.cycle_count,
                speed,
                None,
            );
        }

        let can_check = self.warm_up_complete || elapsed >= self.grace_period as f64;
        if !can_check {
            return self.action(
                WatchdogActionType::None,
                threshold,
                p90,
                self.cycle_count,
                speed,
                None,
            );
        }

        // Periodic socket cycling on 206 Partial Content, independent of speed.
        if self.supports_range && self.recycle_interval_sec > 0.0 {
            let since_last = if self.last_recycle_elapsed >= 0.0 {
                elapsed - self.last_recycle_elapsed
            } else {
                elapsed
            };
            if since_last >= self.recycle_interval_sec {
                let reason = format!(
                    "Periodic {}s cycle (speed {}); cycling {} RPC sockets",
                    self.recycle_interval_sec,
                    format_speed(speed),
                    self.connections,
                );
                return self.trigger_pause_unpause(elapsed, speed, threshold, p90, reason);
            }
        }

        if speed < threshold {
            self.low_speed_consecutive += 1;
            self.straggler_consecutive = 0;
            self.sustained_low_speed_cycles += 1;
            if self.sustained_low_speed_cycles >= self.sustained_throttle_threshold {
                self.sustained_throttle_detected = true;
            }
            if self.low_speed_consecutive >= 5 {
                if !self.supports_range {
                    self.low_speed_consecutive = 0;
                    return self.action(
                        WatchdogActionType::None,
                        threshold,
                        p90,
                        self.cycle_count,
                        speed,
                        None,
                    );
                }
                let suffix = if self.sustained_throttle_detected {
                    " (sustained throttle)"
                } else {
                    ""
                };
                let reason = format!(
                    "Speed {} < {} floor; cycling {} RPC sockets{}",
                    format_speed(speed),
                    format_speed(threshold),
                    self.connections,
                    suffix,
                );
                return self.trigger_pause_unpause(elapsed, speed, threshold, p90, reason);
            }
        } else {
            self.low_speed_consecutive = 0;
            if self.sustained_throttle_detected {
                self.sustained_low_speed_cycles = 0;
                self.sustained_throttle_detected = false;
            }

            // Relative per-connection straggler check (shed-slow pattern).
            if self.supports_range {
                if let Some(per_conn) = &sample.per_conn_speeds {
                    if per_conn.len() >= 2 {
                        let best = per_conn.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                        let slowest = per_conn.iter().copied().fold(f64::INFINITY, f64::min);
                        if best >= 64.0 * 1024.0 && slowest * self.straggler_ratio < best {
                            self.straggler_consecutive += 1;
                            let limit = if self.sustained_throttle_detected {
                                self.sustained_throttle_threshold
                            } else {
                                2
                            };
                            if self.straggler_consecutive >= limit {
                                let reason = format!(
                                    "Straggler connection {} < {}/{}; cycling {} RPC sockets",
                                    format_speed(slowest),
                                    format_speed(best),
                                    self.straggler_ratio as i32,
                                    self.connections,
                                );
                                return self.trigger_pause_unpause(
                                    elapsed, speed, threshold, p90, reason,
                                );
                            }
                        } else {
                            self.straggler_consecutive = 0;
                        }
                    } else {
                        self.straggler_consecutive = 0;
                    }
                } else {
                    self.straggler_consecutive = 0;
                }
            } else {
                self.straggler_consecutive = 0;
            }
        }

        self.action(
            WatchdogActionType::None,
            threshold,
            p90,
            self.cycle_count,
            speed,
            None,
        )
    }
}

impl crate::Watchdog for WatchdogController {
    fn observe(&mut self, sample: ThroughputSample) -> WatchdogAction {
        WatchdogController::observe(self, sample)
    }

    fn state(&self) -> WatchdogState {
        WatchdogController::state(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ResilienceEventType;

    #[test]
    fn v1_ignores_p90() {
        let wd = WatchdogController::new(WatchdogConfig {
            mode: WatchdogMode::V1,
            user_floor_bps: 4000.0,
            ..WatchdogConfig::default()
        });
        assert!((wd.current_threshold_bps() - 4000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resilience_event_types_stringify() {
        assert_eq!(ResilienceEventType::Wake.as_str(), "wake");
        assert_eq!(ResilienceEventType::NetChange.as_str(), "net_change");
        assert_eq!(ResilienceEventType::Stall.as_str(), "stall");
    }
}
