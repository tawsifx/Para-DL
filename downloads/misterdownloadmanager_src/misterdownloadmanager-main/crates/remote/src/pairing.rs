//! Pairing codes. Options → Remote → Pair a device shows an 8-digit code, valid for ten minutes
//! and for one device; closing that window ends it. Each device (by its address) gets five wrong
//! tries, one a second at most; after twenty wrong tries in all the code is void (a new one must
//! be shown). A guesser gets at most twenty chances in a hundred million per code shown, and a
//! device that keeps guessing can't use up the tries of the phone being paired.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// How long a code shown stays valid.
pub const VALID: Duration = Duration::from_secs(10 * 60);
/// Wrong tries one device (address) gets for a code.
pub const TRIES: u8 = 5;
/// Wrong tries in all, from every device, before the code is void.
pub const TOTAL: u8 = 20;
/// The least time between two tries from one device.
pub const PACE: Duration = Duration::from_secs(1);
/// Addresses remembered for pacing (more are forgotten once they're past their pace).
const PEERS: usize = 256;

struct Active {
    code: String,
    until: Instant,
    /// Wrong tries so far, in all and by address.
    wrong: u8,
    wrong_by: HashMap<IpAddr, u8>,
}

#[derive(Default)]
pub struct Pairing {
    active: Option<Active>,
    last_try: HashMap<IpAddr, Instant>,
    /// The device the code shown paired (the window says so and closes).
    paired: Option<String>,
}

/// A try's outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Right,
    /// Wrong; `left` tries remain for this device (0: it can't try this code again).
    Wrong {
        left: u8,
    },
    /// No code is shown (none asked for, timed out, or used up).
    NoCode,
    /// Too soon after this device's last try.
    TooSoon,
}

impl Pairing {
    /// Show a new code (the one before is void).
    pub fn start(&mut self) -> String {
        let n = u64::from_le_bytes(crate::devices::random(8).try_into().unwrap_or([0; 8]))
            % 100_000_000;
        let code = format!("{n:08}");
        self.active = Some(Active {
            code: code.clone(),
            until: Instant::now() + VALID,
            wrong: 0,
            wrong_by: HashMap::new(),
        });
        self.paired = None;
        code
    }

    /// The pairing window closed: its code is void.
    pub fn stop(&mut self) {
        self.active = None;
        self.paired = None;
    }

    /// The code shown now and the time it has left, if any.
    pub fn shown(&self) -> Option<(String, Duration)> {
        let a = self.active.as_ref()?;
        let left = a.until.checked_duration_since(Instant::now())?;
        Some((a.code.clone(), left))
    }

    /// The device the code shown paired, once it has.
    pub fn paired(&self) -> Option<String> {
        self.paired.clone()
    }

    /// The device at `peer` tries `given` (spaces and dashes ignored).
    pub fn check(&mut self, given: &str, peer: IpAddr) -> Check {
        let now = Instant::now();
        if self
            .last_try
            .get(&peer)
            .is_some_and(|t| now.duration_since(*t) < PACE)
        {
            return Check::TooSoon;
        }
        if self.last_try.len() >= PEERS {
            self.last_try.retain(|_, t| now.duration_since(*t) < PACE);
            if self.last_try.len() >= PEERS {
                // Hundreds of addresses trying within one second: none more until they slow.
                return Check::TooSoon;
            }
        }
        self.last_try.insert(peer, now);
        let Some(a) = self
            .active
            .as_mut()
            .filter(|a| a.until > now && a.wrong < TOTAL)
        else {
            self.active = None;
            return Check::NoCode;
        };
        let spent = a.wrong_by.get(&peer).copied().unwrap_or(0);
        if spent >= TRIES {
            return Check::Wrong { left: 0 };
        }
        let given: String = given.chars().filter(char::is_ascii_digit).collect();
        let right = given.len() == a.code.len()
            && given
                .bytes()
                .zip(a.code.bytes())
                .fold(0u8, |acc, (x, y)| acc | (x ^ y))
                == 0;
        if right {
            // One device per code.
            self.active = None;
            return Check::Right;
        }
        a.wrong += 1;
        let spent = spent + 1;
        a.wrong_by.insert(peer, spent);
        if a.wrong >= TOTAL {
            self.active = None;
        }
        Check::Wrong {
            left: TRIES - spent,
        }
    }

    /// The code shown paired `name`.
    pub fn done(&mut self, name: &str) {
        self.paired = Some(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait() {
        std::thread::sleep(PACE + Duration::from_millis(20));
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, last])
    }

    fn wrong_for(code: &str) -> &'static str {
        if code == "00000000" {
            "11111111"
        } else {
            "00000000"
        }
    }

    #[test]
    fn a_code_pairs_once() {
        let mut p = Pairing::default();
        assert_eq!(p.check("12345678", ip(2)), Check::NoCode, "none shown");
        let code = p.start();
        assert_eq!(code.len(), 8);
        wait();
        let spaced = format!("{} {}", &code[..4], &code[4..]);
        assert_eq!(p.check(&spaced, ip(2)), Check::Right);
        wait();
        assert_eq!(p.check(&code, ip(2)), Check::NoCode, "used");
    }

    #[test]
    fn one_device_gets_five_paced_tries() {
        let mut p = Pairing::default();
        let code = p.start();
        let wrong = wrong_for(&code);
        assert_eq!(p.check(wrong, ip(9)), Check::Wrong { left: TRIES - 1 });
        assert_eq!(p.check(&code, ip(9)), Check::TooSoon, "one try a second");
        for left in (0..TRIES - 1).rev() {
            wait();
            assert_eq!(p.check(wrong, ip(9)), Check::Wrong { left });
        }
        wait();
        assert_eq!(
            p.check(&code, ip(9)),
            Check::Wrong { left: 0 },
            "this device is out of tries, even with the right code"
        );
    }

    /// A device guessing on the network neither slows the phone being paired nor uses up its
    /// tries: the phone pairs with the code.
    #[test]
    fn a_guessing_device_does_not_block_the_phone() {
        let mut p = Pairing::default();
        let code = p.start();
        let wrong = wrong_for(&code);
        for _ in 0..TRIES {
            assert!(matches!(p.check(wrong, ip(66)), Check::Wrong { .. }));
            wait();
        }
        assert_eq!(p.check(wrong, ip(66)), Check::Wrong { left: 0 });
        // Right after the guesser's try: the phone isn't paced by it.
        assert_eq!(p.check(&code, ip(2)), Check::Right);
    }

    #[test]
    fn twenty_wrong_tries_in_all_void_the_code() {
        let mut p = Pairing::default();
        let code = p.start();
        let wrong = wrong_for(&code);
        // Four devices, five tries each, at the same moments (each paced on its own).
        for round in 0..TRIES {
            for d in 0..(TOTAL / TRIES) {
                let got = p.check(wrong, ip(100 + d));
                assert!(matches!(got, Check::Wrong { .. }), "round {round}: {got:?}");
            }
            wait();
        }
        assert_eq!(
            p.check(&code, ip(2)),
            Check::NoCode,
            "void after {TOTAL} wrong tries"
        );
    }
}
