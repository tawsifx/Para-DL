//! Hard total data budget, connection clamping, seeded jitter, and failure backoff.
//!
//! Port of `src/paradl/throttle/budget.py`. The jitter RNG is a faithful port of
//! CPython's Mersenne Twister (`random.Random(int(seed))`), so seeded jitter
//! sequences are byte-identical to the Python oracle.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use crate::Error;
use crate::Result;
use crate::models::BudgetSummary;

/// Default quick-run byte budget (300 MiB).
pub const DEFAULT_QUICK_BUDGET_BYTES: i64 = 300 * 1024 * 1024;
/// Default full-run byte budget (3 GiB).
pub const DEFAULT_FULL_BUDGET_BYTES: i64 = 3 * 1024 * 1024 * 1024;
/// Default `throttle sample` byte budget (25 MiB).
pub const DEFAULT_SAMPLE_BUDGET_BYTES: i64 = 25 * 1024 * 1024;

static SIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([a-zA-Z]*)\s*$").unwrap());

/// Parse a byte budget string such as `300MB`, `3GB`, `50M`, or `1048576`.
///
/// Binary multipliers are used (`1 KiB = 1024`). The result is clamped to at
/// least 1024 bytes, matching the Python oracle.
///
/// ```
/// use paradl_throttle::budget::parse_byte_budget;
/// assert_eq!(parse_byte_budget(Some("300MB"), 0).unwrap(), 314_572_800);
/// assert_eq!(parse_byte_budget(Some("2KiB"), 0).unwrap(), 2048);
/// ```
pub fn parse_byte_budget(value: Option<&str>, default_bytes: i64) -> Result<i64> {
    let Some(value) = value else {
        return Ok(default_bytes);
    };
    let s = value.trim();
    if s.is_empty() {
        return Ok(default_bytes);
    }
    let Some(caps) = SIZE_RE.captures(s) else {
        return Err(Error::usage(format!(
            "Unparseable --data-budget {value:?} (expected e.g. '300MB', '3GB', '50M')"
        )));
    };
    let num: f64 = caps[1]
        .parse()
        .map_err(|_| Error::usage(format!("Unparseable --data-budget {value:?}")))?;
    if num <= 0.0 {
        return Err(Error::usage(format!(
            "Data budget must be > 0, got {value:?}"
        )));
    }
    let unit = caps[2].to_ascii_uppercase();
    let multiplier: i64 = match unit.as_str() {
        "" | "B" => 1,
        "K" | "KB" | "KIB" => 1024,
        "M" | "MB" | "MIB" => 1024_i64.pow(2),
        "G" | "GB" | "GIB" => 1024_i64.pow(3),
        _ => {
            return Err(Error::usage(format!(
                "Unknown unit {unit:?} in --data-budget {value:?}"
            )));
        }
    };
    Ok(((num * multiplier as f64) as i64).max(1024))
}

/// Enforce the connection safety cap: 16 per host (32 only when explicitly allowed).
///
/// ```
/// use paradl_throttle::budget::clamp_connections;
/// assert_eq!(clamp_connections(0, false), 1);
/// assert_eq!(clamp_connections(40, false), 16);
/// assert_eq!(clamp_connections(40, true), 32);
/// ```
pub fn clamp_connections(connections: i32, allow_32: bool) -> i32 {
    let cap = if allow_32 { 32 } else { 16 };
    connections.clamp(1, cap)
}

/// CPython-compatible 32-bit Mersenne Twister.
#[derive(Debug, Clone)]
pub struct MersenneTwister {
    state: [u32; 624],
    index: usize,
}

impl MersenneTwister {
    /// Seed the generator exactly as `random.Random(int(seed))` does.
    pub fn new(seed: u64) -> Self {
        let mut mt = [0_u32; 624];
        let key: Vec<u32> = if seed == 0 {
            vec![0]
        } else {
            let mut words = vec![(seed & 0xFFFF_FFFF) as u32];
            if seed > 0xFFFF_FFFF {
                words.push((seed >> 32) as u32);
            }
            words
        };
        // init_by_array, matching CPython's random_seed for integer seeds.
        mt[0] = 19_650_218;
        for i in 1..624 {
            mt[i] = 1_812_433_253_u32
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        let mut i = 1_usize;
        let mut j = 0_usize;
        let mut k = 624_usize.max(key.len());
        while k > 0 {
            mt[i] = (mt[i] ^ ((mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_664_525_u32)))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= 624 {
                mt[0] = mt[623];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = 623;
        while k > 0 {
            mt[i] = (mt[i] ^ ((mt[i - 1] ^ (mt[i - 1] >> 30)).wrapping_mul(1_566_083_941_u32)))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= 624 {
                mt[0] = mt[623];
                i = 1;
            }
            k -= 1;
        }
        mt[0] = 0x8000_0000;
        Self {
            state: mt,
            index: 624,
        }
    }

    fn next_u32(&mut self) -> u32 {
        if self.index >= 624 {
            for i in 0..624 {
                let y = (self.state[i] & 0x8000_0000) | (self.state[(i + 1) % 624] & 0x7FFF_FFFF);
                self.state[i] = self.state[(i + 397) % 624] ^ (y >> 1);
                if y & 1 != 0 {
                    self.state[i] ^= 0x9908_B0DF;
                }
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    /// Return the next float in `[0.0, 1.0)`, matching `random.Random.random()`.
    pub fn random(&mut self) -> f64 {
        let a = (self.next_u32() >> 5) as f64;
        let b = (self.next_u32() >> 6) as f64;
        (a * 67_108_864.0 + b) / 9_007_199_254_740_992.0
    }

    /// Return a uniform float in `[min, max)`, matching `random.uniform`.
    pub fn uniform(&mut self, min: f64, max: f64) -> f64 {
        min + (max - min) * self.random()
    }
}

/// Thread-safe-enforced budget tracker (exclusive `&mut self` access).
///
/// Guarantees that `used_bytes` never exceeds `limit_bytes` across sequential
/// calls, and drops targets after `max_failures_per_target` consecutive
/// failures with exponential backoff on 429/5xx responses.
#[derive(Debug, Clone)]
pub struct DataBudgetTracker {
    /// Hard limit in bytes.
    pub limit_bytes: i64,
    /// Planned worst-case byte usage.
    pub planned_worst_case_bytes: i64,
    /// RNG seed used for jitter/backoff.
    pub seed: u64,
    /// Failures before a target is dropped.
    pub max_failures_per_target: i64,
    /// Base backoff interval in seconds.
    pub base_backoff_sec: f64,
    /// Bytes consumed so far.
    pub used_bytes: i64,
    /// Consecutive failure counts per target.
    pub target_failures: HashMap<String, i64>,
    /// Targets permanently dropped this run.
    pub dropped_targets: BTreeSet<String>,
    /// `(target, status_code, backoff_seconds)` history.
    pub backoff_history: Vec<(String, i64, f64)>,
    /// Seeded jitter draws.
    pub jitter_history: Vec<f64>,
    rng: MersenneTwister,
}

impl DataBudgetTracker {
    /// Construct a tracker with the given hard limit and RNG seed.
    pub fn new(limit_bytes: i64, seed: u64) -> Self {
        let limit = limit_bytes.max(1024);
        let planned = (limit as f64 * 0.85) as i64;
        Self {
            limit_bytes: limit,
            planned_worst_case_bytes: planned,
            seed,
            max_failures_per_target: 3,
            base_backoff_sec: 0.5,
            used_bytes: 0,
            target_failures: HashMap::new(),
            dropped_targets: BTreeSet::new(),
            backoff_history: Vec::new(),
            jitter_history: Vec::new(),
            rng: MersenneTwister::new(seed),
        }
    }

    /// Whether the hard byte limit has been reached.
    pub fn exhausted(&self) -> bool {
        self.used_bytes >= self.limit_bytes
    }

    /// Bytes remaining before the hard limit.
    pub fn remaining_bytes(&self) -> i64 {
        (self.limit_bytes - self.used_bytes).max(0)
    }

    /// Maximum bytes a probe may transfer within the remaining budget.
    pub fn allocate_probe_budget(&self, desired_bytes: i64) -> i64 {
        desired_bytes.max(0).min(self.remaining_bytes())
    }

    /// Charge up to `chunk_len` bytes and return the actually allowed count.
    pub fn consume_up_to(&mut self, chunk_len: i64) -> i64 {
        if chunk_len <= 0 {
            return 0;
        }
        let taken = chunk_len.min(self.remaining_bytes());
        self.used_bytes += taken;
        taken
    }

    /// Draw a seeded jitter value in `[min, max)` and record it (no sleeping).
    pub fn jitter_value(&mut self, min: f64, max: f64) -> f64 {
        let delay = self.rng.uniform(min, max);
        self.jitter_history.push(delay);
        delay
    }

    /// Reset the consecutive failure count after a successful probe.
    pub fn record_success(&mut self, target_id: &str) {
        if !self.dropped_targets.contains(target_id) {
            self.target_failures.insert(target_id.to_owned(), 0);
        }
    }

    /// Record a failure; return `true` if the target is now dropped.
    ///
    /// Returns the computed backoff in seconds alongside the drop flag, so the
    /// caller can sleep without this pure core touching a clock.
    pub fn record_failure(&mut self, target_id: &str, status_code: Option<i64>) -> (bool, f64) {
        let count = self.target_failures.get(target_id).copied().unwrap_or(0) + 1;
        self.target_failures.insert(target_id.to_owned(), count);
        let is_rate_or_server_err = matches!(status_code, Some(429 | 500 | 502 | 503 | 504) | None);
        let mut backoff = 0.0_f64;
        if is_rate_or_server_err {
            let mult = if status_code == Some(429) {
                2.0_f64
            } else {
                1.5_f64
            };
            let jitter = self.rng.uniform(0.05, 0.25);
            backoff = (self.base_backoff_sec * mult.powi(count as i32 - 1) + jitter).min(15.0);
            self.backoff_history
                .push((target_id.to_owned(), status_code.unwrap_or(0), backoff));
        }
        let dropped = count >= self.max_failures_per_target;
        if dropped {
            self.dropped_targets.insert(target_id.to_owned());
        }
        (dropped, backoff)
    }

    /// Whether a target has been dropped.
    pub fn is_dropped(&self, target_id: &str) -> bool {
        self.dropped_targets.contains(target_id)
    }

    /// Build a serializable budget summary.
    pub fn to_summary(&self) -> BudgetSummary {
        BudgetSummary {
            limit_bytes: self.limit_bytes,
            planned_worst_case_bytes: self.planned_worst_case_bytes,
            actual_used_bytes: self.used_bytes,
            exceeded: self.used_bytes > self.limit_bytes,
        }
    }
}
