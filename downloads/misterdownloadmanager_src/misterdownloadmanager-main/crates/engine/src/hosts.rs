//! Learned per-host connection limits (beyond IDM, D-012): when a host refuses extra connections
//! while others to it are receiving data, it allows at least those — the most ever seen working
//! at a refusal is the cap (counting connections still connecting, or keeping the smallest
//! count, learned 1 for a host that allows 2: benchmark 2026-10-30). After two such observations the
//! cap applies — to this download and to later ones — instead of every extra connection using
//! up its retries (wasted time, and a risk of being banned). Forgotten after `DECAY`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a learned cap lasts without being seen again.
pub const DECAY: Duration = Duration::from_secs(10 * 60);
/// Observations before a cap applies (one refusal can be a hiccup).
const NEEDED: u32 = 2;

#[derive(Debug, Default)]
pub struct HostLimits {
    hosts: Mutex<HashMap<String, Learned>>,
}

#[derive(Debug, Clone, Copy)]
struct Learned {
    cap: u32,
    seen: u32,
    at: Instant,
}

impl HostLimits {
    /// The connections `host` is known to allow, if learned.
    pub fn cap(&self, host: &str) -> Option<u32> {
        let mut hosts = self.hosts.lock().unwrap();
        match hosts.get(host) {
            Some(l) if l.at.elapsed() >= DECAY => {
                hosts.remove(host);
                None
            }
            Some(l) if l.seen >= NEEDED => Some(l.cap),
            _ => None,
        }
    }

    /// An extra connection to `host` was refused while `working` others were receiving data.
    /// Returns the cap once it applies.
    pub fn refused(&self, host: &str, working: u32) -> Option<u32> {
        let working = working.max(1);
        let mut hosts = self.hosts.lock().unwrap();
        let l = hosts.entry(host.to_owned()).or_insert(Learned {
            cap: working,
            seen: 0,
            at: Instant::now(),
        });
        if l.at.elapsed() >= DECAY {
            *l = Learned {
                cap: working,
                seen: 0,
                at: Instant::now(),
            };
        }
        l.cap = l.cap.max(working);
        l.seen += 1;
        l.at = Instant::now();
        (l.seen >= NEEDED).then_some(l.cap)
    }

    /// `working` connections to `host` are receiving data at once: a learned cap below that
    /// was too low (refusals can arrive before an accepted connection's first byte) — raise it.
    pub fn working(&self, host: &str, working: u32) {
        let mut hosts = self.hosts.lock().unwrap();
        if let Some(l) = hosts.get_mut(host)
            && working > l.cap
        {
            l.cap = working;
        }
    }

    /// Everything learned, for display: (host, cap).
    pub fn learned(&self) -> Vec<(String, u32)> {
        let hosts = self.hosts.lock().unwrap();
        let mut v: Vec<(String, u32)> = hosts
            .iter()
            .filter(|(_, l)| l.seen >= NEEDED && l.at.elapsed() < DECAY)
            .map(|(h, l)| (h.clone(), l.cap))
            .collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_teach_the_most_that_worked() {
        let h = HostLimits::default();
        assert_eq!(h.refused("x.org", 1), None, "one refusal can be a hiccup");
        assert_eq!(h.cap("x.org"), None);
        // A refusal while only one other was receiving doesn't hide that two worked before.
        assert_eq!(h.refused("x.org", 2), Some(2));
        assert_eq!(h.refused("x.org", 1), Some(2));
        assert_eq!(h.cap("x.org"), Some(2));
        // More seen working later raises it; fewer never lowers it.
        h.working("x.org", 3);
        h.working("x.org", 1);
        assert_eq!(h.cap("x.org"), Some(3));
        h.working("y.org", 5);
        assert_eq!(h.cap("y.org"), None, "nothing refused there: no cap");
        assert_eq!(h.cap("other.org"), None);
        assert_eq!(h.learned(), vec![("x.org".to_owned(), 3)]);
    }
}
