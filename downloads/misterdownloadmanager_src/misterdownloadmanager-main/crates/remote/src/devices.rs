//! The paired devices (`remote/devices.json`, readable by the user only). Each has a token of
//! its own (256 random bits) that its browser keeps and sends with every request; the file holds
//! only the token's SHA-256, so the file alone opens nothing. A device removed in Options is
//! refused from its next request on.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// One paired device.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Device {
    /// A short id of its own (Options' Remove names it).
    pub id: String,
    /// The name it was given when it paired ("Pixel 8", "Living room laptop").
    pub name: String,
    /// When it paired and when it was last seen, Unix seconds.
    pub added: i64,
    pub seen: i64,
    /// SHA-256 of its token, hex.
    hash: String,
}

pub struct Devices {
    path: Option<PathBuf>,
    list: Vec<Device>,
    /// When the last-seen times were last written (they change at every request).
    saved_seen: i64,
}

/// The longest device name kept (characters).
pub const MAX_NAME: usize = 40;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `n` random bytes from the system's generator.
pub fn random(n: usize) -> Vec<u8> {
    use aws_lc_rs::rand::SecureRandom;
    let mut buf = vec![0u8; n];
    // The system's generator doesn't fail on a working system; a zeroed token would, so stop.
    aws_lc_rs::rand::SystemRandom::new()
        .fill(&mut buf)
        .expect("the system's random number generator");
    buf
}

fn hash(token: &str) -> String {
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

/// A name as given, made safe to show: no control characters, trimmed, at most `MAX_NAME`.
pub fn clean_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    name.trim().to_owned()
}

impl Devices {
    /// The list kept at `path` (None: in memory only, for tests).
    pub fn load(path: Option<PathBuf>) -> Devices {
        let list = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Devices {
            path,
            list,
            saved_seen: now(),
        }
    }

    fn save(&mut self) {
        self.saved_seen = now();
        if let Some(path) = &self.path
            && let Ok(json) = serde_json::to_vec_pretty(&self.list)
        {
            if let Some(dir) = path.parent() {
                let _ = mdm_ipc::private_dir(dir);
            }
            if let Err(e) = mdm_ipc::private_write(path, &json) {
                tracing::warn!("remote control: the paired devices could not be saved: {e}");
            }
        }
    }

    pub fn list(&self) -> Vec<Device> {
        self.list.clone()
    }

    /// Pair a new device called `name`: its record and its token (given once, to the device).
    pub fn add(&mut self, name: &str) -> (Device, String) {
        let token = hex::encode(random(32));
        let mut id = hex::encode(random(4));
        while self.list.iter().any(|d| d.id == id) {
            id = hex::encode(random(4));
        }
        let name = match clean_name(name) {
            n if n.is_empty() => "?".to_owned(),
            n => n,
        };
        let device = Device {
            id,
            name,
            added: now(),
            seen: now(),
            hash: hash(&token),
        };
        self.list.push(device.clone());
        self.save();
        (device, token)
    }

    /// The device `token` belongs to (its last-seen time brought up to date), if any.
    pub fn check(&mut self, token: &str) -> Option<Device> {
        let h = hash(token);
        // Every entry is compared in full, whatever matches (no early exit on a prefix).
        let mut found = None;
        for (i, d) in self.list.iter().enumerate() {
            if same(&d.hash, &h) {
                found = Some(i);
            }
        }
        let i = found?;
        self.list[i].seen = now();
        // Last-seen times are written at most every ten minutes.
        if now() - self.saved_seen > 600 {
            self.save();
        }
        Some(self.list[i].clone())
    }

    /// Remove device `id`; true when it was there.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|d| d.id != id);
        let removed = self.list.len() != before;
        if removed {
            self.save();
        }
        removed
    }
}

/// Comparison in time independent of where the first difference is.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_check_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let mut d = Devices::load(Some(path.clone()));
        let (phone, token) = d.add("Phone\u{7}\n with a very long name that goes on and on and on");
        assert!(phone.name.chars().count() <= MAX_NAME);
        assert!(
            phone.name.starts_with("Phone with"),
            "no control characters"
        );
        assert_eq!(token.len(), 64);
        // The file holds the hash only.
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains(&token));
        // Read back by a new start.
        let mut d = Devices::load(Some(path));
        assert_eq!(d.check(&token).unwrap().id, phone.id);
        assert!(d.check("wrong").is_none());
        assert!(d.remove(&phone.id));
        assert!(d.check(&token).is_none(), "removed: refused");
    }
}
