//! Learned per-(host, hour-bucket, network) profiles and their storage port.
//!
//! Ported from `src/paradl/storage/history.py` (`LearnedProfile`,
//! `HistoryManager.get_best_profile`, `save_learned_profile`). Persistence is
//! injected so the watchdog crate stays storage-agnostic; `paradl-store` (W1-1)
//! will supply a SQLite-backed implementation.

use std::sync::Mutex;

use crate::types::WatchdogActionType;

/// A learned outcome for one `(host, hour_bucket, network_id)` tuple.
#[derive(Debug, Clone, PartialEq)]
pub struct LearnedProfile {
    /// Lower-cased host name.
    pub host: String,
    /// 4-hour bucket (0..5), or -1 for "unknown".
    pub hour_bucket: i32,
    /// Network identifier (SSID or primary interface), or `"default"`.
    pub network_id: String,
    /// Best-known parallel connection count (1..=16).
    pub connections: i32,
    /// Best-known IP family.
    pub ip_family: String,
    /// Recovery action that produced the outcome.
    pub recovery_action: String,
    /// Throughput observed before the mitigation.
    pub speed_before: f64,
    /// Throughput observed after the mitigation.
    pub speed_after: f64,
    /// Whether the host served HTTP 206 Partial Content.
    pub supports_range: bool,
    /// Optional throttle classification.
    pub throttle_class: Option<String>,
    /// Human-readable update timestamp.
    pub updated_at: String,
    /// Epoch seconds used for staleness checks.
    pub updated_epoch: f64,
    /// Profile source (`"watchdog"` by default).
    pub source: String,
    /// Optional average RTT in milliseconds.
    pub avg_rtt_ms: f64,
    /// Optional packet-loss percentage.
    pub packet_loss_pct: f64,
}

impl Default for LearnedProfile {
    fn default() -> Self {
        Self {
            host: String::new(),
            hour_bucket: -1,
            network_id: "default".to_string(),
            connections: 16,
            ip_family: "auto".to_string(),
            recovery_action: WatchdogActionType::PauseUnpause.as_str().to_string(),
            speed_before: 0.0,
            speed_after: 0.0,
            supports_range: true,
            throttle_class: None,
            updated_at: String::new(),
            updated_epoch: 0.0,
            source: "watchdog".to_string(),
            avg_rtt_ms: 0.0,
            packet_loss_pct: 0.0,
        }
    }
}

impl LearnedProfile {
    /// Effective epoch: `updated_epoch` when positive, else parse `updated_at`.
    pub fn effective_epoch(&self) -> f64 {
        if self.updated_epoch > 0.0 {
            self.updated_epoch
        } else {
            0.0
        }
    }

    /// Age in seconds relative to `now_epoch` (never negative).
    pub fn age_seconds(&self, now_epoch: f64) -> f64 {
        let effective = self.effective_epoch();
        if effective <= 0.0 {
            return 0.0;
        }
        (now_epoch - effective).max(0.0)
    }

    /// Return `true` when the profile is older than `max_age_days` (default 30).
    pub fn is_stale(&self, now_epoch: f64, max_age_days: f64) -> bool {
        let effective = self.effective_epoch();
        if effective <= 0.0 {
            return false;
        }
        (now_epoch - effective) > max_age_days * 86_400.0
    }
}

/// Storage port for learned profiles, mirroring the Python `HistoryManager`
/// class methods used by the watchdog.
pub trait ProfileStore: Send + Sync {
    /// Select the best profile for a host, preferring exact
    /// `(hour_bucket, network_id)` then `network_id`, then `hour_bucket`, then
    /// any profile for that host (highest `speed_after` wins ties).
    fn get_best_profile(
        &self,
        host: &str,
        hour_bucket: i32,
        network_id: &str,
    ) -> Option<LearnedProfile>;

    /// Persist (upsert) a learned profile.
    fn save_learned_profile(&self, profile: LearnedProfile);
}

/// In-memory [`ProfileStore`] used by default and by scenario tests.
#[derive(Debug, Default)]
pub struct InMemoryProfileStore {
    profiles: Mutex<Vec<LearnedProfile>>,
}

impl InMemoryProfileStore {
    /// Construct an empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a snapshot of every stored profile (insertion order).
    pub fn all_profiles(&self) -> Vec<LearnedProfile> {
        self.profiles.lock().unwrap().clone()
    }
}

impl ProfileStore for InMemoryProfileStore {
    fn get_best_profile(
        &self,
        host: &str,
        hour_bucket: i32,
        network_id: &str,
    ) -> Option<LearnedProfile> {
        let host_lower = host.to_ascii_lowercase();
        let profiles = self.profiles.lock().unwrap();
        profiles
            .iter()
            .filter(|p| p.host.eq_ignore_ascii_case(&host_lower))
            .max_by(|a, b| {
                let score = |p: &LearnedProfile| -> (i32, f64, f64) {
                    let hb = p.hour_bucket == hour_bucket;
                    let net = p.network_id == network_id;
                    let specificity = match (hb, net) {
                        (true, true) => 3,
                        (false, true) => 2,
                        (true, false) => 1,
                        (false, false) => 0,
                    };
                    (specificity, p.speed_after, p.effective_epoch())
                };
                let sa = score(a);
                let sb = score(b);
                sa.0.cmp(&sb.0)
                    .then_with(|| sa.1.total_cmp(&sb.1))
                    .then_with(|| sa.2.total_cmp(&sb.2))
            })
            .cloned()
    }

    fn save_learned_profile(&self, profile: LearnedProfile) {
        let mut profiles = self.profiles.lock().unwrap();
        if let Some(existing) = profiles.iter_mut().find(|p| {
            p.host.eq_ignore_ascii_case(&profile.host)
                && p.hour_bucket == profile.hour_bucket
                && p.network_id == profile.network_id
        }) {
            *existing = profile;
        } else {
            profiles.push(profile);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_uses_thirty_day_default() {
        let profile = LearnedProfile {
            updated_epoch: 1000.0,
            ..LearnedProfile::default()
        };
        assert!(!profile.is_stale(1000.0 + 2_592_000.0, 30.0));
        assert!(profile.is_stale(1000.0 + 2_592_001.0, 30.0));
        assert_eq!(profile.age_seconds(1500.0), 500.0);
    }

    #[test]
    fn best_profile_prefers_exact_match() {
        let store = InMemoryProfileStore::new();
        store.save_learned_profile(LearnedProfile {
            host: "h".into(),
            hour_bucket: 0,
            network_id: "wifi".into(),
            connections: 4,
            ..LearnedProfile::default()
        });
        store.save_learned_profile(LearnedProfile {
            host: "h".into(),
            hour_bucket: 1,
            network_id: "wifi".into(),
            connections: 8,
            ..LearnedProfile::default()
        });
        let best = store.get_best_profile("h", 0, "wifi").unwrap();
        assert_eq!(best.connections, 4);
    }
}
