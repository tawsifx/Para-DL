//! Android ADB device synchronisation (external `adb` process only).
//!
//! Port of `src/paradl/postproc/adb.py`. Device enumeration uses
//! `adb devices -l`; a successful push also triggers an Android media scan.
//! No Python is spawned.
//!
//! ```
//! use paradl_postproc::adb::{parse_devices, DEFAULT_MUSIC_DIR};
//! let output = "List of devices attached\nserial1\tdevice model:Pixel_7\n";
//! let devices = parse_devices(output);
//! assert_eq!(devices[0].model, "Pixel 7");
//! assert_eq!(DEFAULT_MUSIC_DIR, "/storage/emulated/0/Music");
//! ```

use std::path::Path;
use std::process::Command;

/// Default Android music directory (`config.py:14`, parity row G14).
pub const DEFAULT_MUSIC_DIR: &str = "/storage/emulated/0/Music";
/// Default Android download directory (`config.py:15`, parity row G15).
pub const DEFAULT_DOWNLOAD_DIR: &str = "/storage/emulated/0/Download";

/// A connected Android device reported by `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Device serial.
    pub serial: String,
    /// Connection status (`device`, `unauthorized`, ...).
    pub status: String,
    /// Human-readable model name.
    pub model: String,
}

/// Parse the stdout of `adb devices -l` (first line is the header).
pub fn parse_devices(output: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    for raw in output.lines().skip(1) {
        let line = raw.trim();
        if line.is_empty() || line.contains("offline") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let mut model = "Android Device".to_string();
        for part in &parts[2..] {
            if let Some(value) = part.strip_prefix("model:") {
                model = value.replace('_', " ");
            }
        }
        devices.push(Device {
            serial: parts[0].to_string(),
            status: parts[1].to_string(),
            model,
        });
    }
    devices
}

/// Enumerate connected devices, or an empty list when `adb` is unavailable.
pub fn connected_devices() -> Vec<Device> {
    match Command::new("adb").args(["devices", "-l"]).output() {
        Ok(output) => parse_devices(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => Vec::new(),
    }
}

/// Push `file` to the first connected device and trigger a media scan.
pub fn push_file(file: &Path, target_dir: &str) -> bool {
    let devices = connected_devices();
    let Some(device) = devices.first() else {
        return false;
    };
    if device.status != "device" {
        return false;
    }
    let destination = format!("{}/", target_dir.trim_end_matches('/'));
    let push = Command::new("adb")
        .args(["-s", &device.serial, "push"])
        .arg(file)
        .arg(&destination)
        .status();
    if !push.is_ok_and(|status| status.success()) {
        return false;
    }
    let remote = format!(
        "{}/{}",
        target_dir.trim_end_matches('/'),
        file.file_name().unwrap_or_default().to_string_lossy()
    );
    let _ = Command::new("adb")
        .args(["-s", &device.serial, "shell", "am", "broadcast", "-a"])
        .arg("android.intent.action.MEDIA_SCANNER_SCAN_FILE")
        .arg("-d")
        .arg(format!("file://{remote}"))
        .output();
    true
}
