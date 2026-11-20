//! Android ADB device synchronisation (external `adb` process only).
//!
//! Port of `src/paradl/postproc/adb.py`. Device enumeration uses
//! `adb devices -l`; a successful push also triggers an Android media scan.
//! Every external call is bounded to 5 seconds (`adb.py:25`) — a timeout kills
//! the child and returns [`paradl_core::Error`] (exit code `1`), never a silent
//! "no devices" result. No Python is spawned.
//!
//! ```
//! use paradl_postproc::adb::{parse_devices, DEFAULT_MUSIC_DIR};
//! let output = "List of devices attached\nserial1\tdevice model:Pixel_7\n";
//! let devices = parse_devices(output);
//! assert_eq!(devices[0].model, "Pixel 7");
//! assert_eq!(DEFAULT_MUSIC_DIR, "/storage/emulated/0/Music");
//! ```

use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use paradl_core::Error;

use crate::Result;

/// Timeout for every `adb` invocation (`adb.py:25`: `timeout=5.0`).
pub const ADB_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Wait for `child` for at most `timeout`, killing it on expiry.
///
/// The status waiter polls `Child::try_wait` and releases the mutex between
/// polls, so the caller can kill the child promptly. stdout/stderr are drained
/// concurrently to avoid pipe-buffer deadlocks.
fn wait_with_timeout(child: Child, timeout: Duration) -> io::Result<Output> {
    let mut child = child;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    drop(child.stdin.take());

    let stdout_reader = stdout.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });
    let stderr_reader = stderr.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        })
    });

    let shared = Arc::new(Mutex::new(child));
    let (sender, receiver) = mpsc::channel();
    let waiter_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        loop {
            let polled = match waiter_shared.lock() {
                Ok(mut guard) => guard.try_wait(),
                Err(_) => Err(io::Error::other("child mutex poisoned")),
            };
            match polled {
                Ok(Some(status)) => {
                    let _ = sender.send(Ok(status));
                    return;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = sender.send(Err(error));
                    return;
                }
            }
        }
    });

    let status = match receiver.recv_timeout(timeout) {
        Ok(result) => result?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            if let Ok(mut guard) = shared.lock() {
                let _ = guard.kill();
                let _ = guard.wait();
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("subprocess timed out after {} ms", timeout.as_millis()),
            ));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(io::Error::other("subprocess status channel closed"));
        }
    };

    let stdout = stdout_reader
        .map(|handle| {
            handle
                .join()
                .unwrap_or(Err(io::Error::other("stdout reader panicked")))
        })
        .transpose()?
        .unwrap_or_default();
    let stderr = stderr_reader
        .map(|handle| {
            handle
                .join()
                .unwrap_or(Err(io::Error::other("stderr reader panicked")))
        })
        .transpose()?
        .unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Run `adb` with bounded waiting. `None` means the tool is unavailable; a
/// timeout is a real error and is propagated.
fn run_adb(args: &[&str]) -> Result<Option<Output>> {
    let spawn = Command::new("adb")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(child) = spawn else {
        return Ok(None);
    };
    match wait_with_timeout(child, ADB_TIMEOUT) {
        Ok(output) => Ok(Some(output)),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Err(Error::failure(format!(
            "adb {:?} timed out after {}s",
            args,
            ADB_TIMEOUT.as_secs()
        ))),
        Err(_) => Ok(None),
    }
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

/// Enumerate connected devices. An unavailable `adb` yields an empty list;
/// a timeout is returned as an error (exit code `1`).
pub fn connected_devices() -> Result<Vec<Device>> {
    let Some(output) = run_adb(&["devices", "-l"])? else {
        return Ok(Vec::new());
    };
    Ok(parse_devices(&String::from_utf8_lossy(&output.stdout)))
}

/// Push `file` to the first connected device and trigger a media scan.
///
/// Returns an error on timeout instead of reporting a silent failure.
pub fn push_file(file: &Path, target_dir: &str) -> Result<bool> {
    let devices = connected_devices()?;
    let Some(device) = devices.first() else {
        return Ok(false);
    };
    if device.status != "device" {
        return Ok(false);
    }
    let file_name = file.to_string_lossy();
    let destination = format!("{}/", target_dir.trim_end_matches('/'));
    let Some(push) = run_adb(&["-s", &device.serial, "push", &file_name, &destination])? else {
        return Ok(false);
    };
    if !push.status.success() {
        return Ok(false);
    }
    let remote = format!(
        "{}{}",
        destination,
        file.file_name().unwrap_or_default().to_string_lossy()
    );
    run_adb(&[
        "-s",
        &device.serial,
        "shell",
        "am",
        "broadcast",
        "-a",
        "android.intent.action.MEDIA_SCANNER_SCAN_FILE",
        "-d",
        &format!("file://{remote}"),
    ])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn adb_timeout_matches_oracle() {
        assert_eq!(ADB_TIMEOUT, Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_kills_a_hanging_child() {
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 10"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let error = wait_with_timeout(child, Duration::from_millis(200)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[test]
    fn wait_with_timeout_captures_fast_output() {
        let child = Command::new("/bin/sh")
            .args(["-c", "printf ok"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_with_timeout(child, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
    }
}
