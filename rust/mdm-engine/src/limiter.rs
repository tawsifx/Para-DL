// Modifications: removed MDM's inline `#[cfg(test)] mod tests;` (tests are in src/tests.rs).
//! Per-download speed limiter (IDM: "Maximum download speed for one file", notes/ENGINE.md §4).
//! A token bucket shared by all of a download's connections; the limit can change while the
//! download runs, and a new limit applies to the waits already started too (#621). Throttling
//! the socket reads makes TCP slow the server down.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
// Tokio's clock: the system's, except in tests that pause it.
use tokio::time::Instant;

/// Bytes per second; 0 = unlimited. One per download.
#[derive(Debug)]
pub struct Limiter {
    /// Changed under the bucket's lock, read without it by [`Limiter::get`].
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
    /// Every byte that went through it (less what was given back), limited or not.
    taken: AtomicU64,
    /// Wakes the waits when the limit changes, so they count what is left at the new rate.
    changed: Notify,
}

#[derive(Debug)]
struct Bucket {
    /// Bytes that may be read right now (may go negative: debt to wait off).
    tokens: f64,
    last: Instant,
    /// The budget earned since the limiter was made, bytes, at whatever the rate was each
    /// moment: a wait ends when this reaches its mark, so a new rate applies to the rest of it.
    earned: f64,
    /// Goes up each time the limit is removed: the debt is forgiven then, and every wait from
    /// before ends, even if a limit is set again before that wait looks.
    era: u64,
}

impl Bucket {
    /// Add what `rate` earned since the last time.
    fn refill(&mut self, rate: u64, now: Instant) {
        let earned = now.saturating_duration_since(self.last).as_secs_f64() * rate as f64;
        self.last = now;
        if rate > 0 {
            self.earned += earned;
            self.tokens = (self.tokens + earned).min(rate as f64 * BURST_SECS);
        }
    }
}

/// Where a wait ends: once the bucket has earned `at` bytes, in the same `era`.
#[derive(Debug)]
struct Mark {
    at: f64,
    era: u64,
}

/// Burst allowance: up to one second's budget accumulates while no connection is reading.
const BURST_SECS: f64 = 1.0;

impl Limiter {
    pub fn new(rate: Option<u64>) -> Self {
        Limiter {
            rate: AtomicU64::new(rate.unwrap_or(0)),
            bucket: Mutex::new(Bucket {
                tokens: 0.0,
                last: Instant::now(),
                earned: 0.0,
                era: 0,
            }),
            taken: AtomicU64::new(0),
            changed: Notify::new(),
        }
    }

    /// The bytes that went through it so far (less those given back).
    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// A new limit (None = unlimited), for the waits already started too.
    pub fn set(&self, rate: Option<u64>) {
        let rate = rate.unwrap_or(0);
        {
            let mut b = self.bucket.lock().unwrap();
            let old = self.rate.load(Ordering::Relaxed);
            if old == rate {
                return;
            }
            b.refill(old, Instant::now());
            if rate == 0 {
                b.tokens = 0.0;
                b.era += 1;
            }
            self.rate.store(rate, Ordering::Relaxed);
        }
        self.changed.notify_waiters();
    }

    pub fn get(&self) -> Option<u64> {
        Some(self.rate.load(Ordering::Relaxed)).filter(|r| *r > 0)
    }

    /// Account for `n` received bytes; where the caller's wait ends, if it must wait.
    fn take(&self, n: u64) -> Option<Mark> {
        self.taken.fetch_add(n, Ordering::Relaxed);
        if self.rate.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let mut b = self.bucket.lock().unwrap();
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return None;
        }
        b.refill(rate, Instant::now());
        b.tokens -= n as f64;
        (b.tokens < 0.0).then(|| Mark {
            at: b.earned - b.tokens,
            era: b.era,
        })
    }

    /// How much longer the wait ending at `mark` lasts at the rate now; None once it is over.
    fn left(&self, mark: &Mark) -> Option<Duration> {
        let mut b = self.bucket.lock().unwrap();
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 || b.era != mark.era {
            return None;
        }
        b.refill(rate, Instant::now());
        let owed = mark.at - b.earned;
        (owed >= 1.0).then(|| Duration::from_secs_f64(owed / rate as f64))
    }

    /// Give back budget for `n` bytes that were paid for but not kept.
    pub fn refund(&self, n: u64) {
        let mut t = self.taken.load(Ordering::Relaxed);
        while let Err(now) = self.taken.compare_exchange_weak(
            t,
            t.saturating_sub(n),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            t = now;
        }
        if self.rate.load(Ordering::Relaxed) > 0 {
            self.bucket.lock().unwrap().tokens += n as f64;
        }
    }

    /// Wait until `n` more bytes fit under the limit: the limit at each moment of the wait, not
    /// only the one when it started (#621).
    pub async fn acquire(&self, n: u64) {
        let Some(mark) = self.take(n) else { return };
        loop {
            let mut changed = std::pin::pin!(self.changed.notified());
            changed.as_mut().enable();
            let Some(wait) = self.left(&mark) else { return };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = changed => {}
            }
        }
    }
}
