//! macOS platform implementation.
//!
//! Mirrors `src/paradl/platform/macos.py` for origin metadata (`xattr -w -x`)
//! and keychain secrets (`security -i` over **stdin**, never argv). External
//! tools stay external processes; no Python is spawned. All helpers are safe
//! no-ops on non-macOS hosts, matching the Python `is_effective_macos()` fallback.
//!
//! ```
//! # #[cfg(feature = "platform-macos")]
//! # {
//! use paradl_platform::macos::{binary_plist_string_array, keychain_add_command};
//! let urls = vec!["https://example.com/file.zip".to_string()];
//! assert!(binary_plist_string_array(&urls).starts_with(b"bplist00"));
//! assert!(keychain_add_command("pdl", "hf", "tok", None).ends_with("-w \"tok\"\n"));
//! # }
//! ```

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use paradl_core::Error;

use crate::{Platform, Result};

/// Timeout for `security -i` keychain operations (`macos.py:672/718/755`).
const SECURITY_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout for `xattr -w -x` metadata writes (`macos.py:820/845`).
const XATTR_TIMEOUT: Duration = Duration::from_secs(3);

/// `com.apple.metadata:kMDItemWhereFroms` xattr name (parity row L32).
pub const WHERE_FROMS_ATTR: &str = "com.apple.metadata:kMDItemWhereFroms";
/// `com.apple.quarantine` xattr name (parity row L32).
pub const QUARANTINE_ATTR: &str = "com.apple.quarantine";
/// Default keychain service (parity row L35).
pub const KEYCHAIN_SERVICE: &str = "pdl";

/// Number of bytes `plistlib` uses for an integer of this magnitude: 1, 2, 4, or 8.
fn int_byte_width(value: usize) -> usize {
    if value <= u8::MAX as usize {
        1
    } else if value <= u16::MAX as usize {
        2
    } else if value <= u32::MAX as usize {
        4
    } else {
        8
    }
}

/// Encode a non-negative integer as a binary-plist integer object.
fn plist_integer(value: usize) -> Vec<u8> {
    if value <= u8::MAX as usize {
        vec![0x10, value as u8]
    } else if value <= u16::MAX as usize {
        vec![0x11, (value >> 8) as u8, value as u8]
    } else if value <= u32::MAX as usize {
        let mut out = vec![0x12];
        out.extend_from_slice(&(value as u32).to_be_bytes());
        out
    } else {
        let mut out = vec![0x13];
        out.extend_from_slice(&(value as u64).to_be_bytes());
        out
    }
}

/// Encode one string as a binary-plist object body (marker + length + bytes).
fn plist_string(value: &str) -> Vec<u8> {
    let (base, data, len) = if value.is_ascii() {
        (0x50u8, value.as_bytes().to_vec(), value.len())
    } else {
        let units: Vec<u16> = value.encode_utf16().collect();
        let mut data = Vec::with_capacity(units.len() * 2);
        for unit in &units {
            data.extend_from_slice(&unit.to_be_bytes());
        }
        (0x60u8, data, units.len())
    };
    let mut out = Vec::with_capacity(data.len() + 10);
    if len < 15 {
        out.push(base | len as u8);
    } else {
        out.push(base | 0x0f);
        out.extend_from_slice(&plist_integer(len));
    }
    out.extend_from_slice(&data);
    out
}

/// Serialize `urls` into a **CPython `plistlib` byte-exact** binary plist.
///
/// The output equals `plistlib.dumps(list(urls), fmt=plistlib.FMT_BINARY)` from
/// `macos.py:814`. Verified against goldens for empty, single, multiple,
/// length-boundary (14/15/255/256), and UTF-16 (non-ASCII) strings.
pub fn binary_plist_string_array(urls: &[String]) -> Vec<u8> {
    let count = urls.len();
    let ref_width = int_byte_width(count);
    let mut objects: Vec<Vec<u8>> = Vec::with_capacity(count + 1);

    // Object 0: the root array.
    let mut root = Vec::new();
    if count < 15 {
        root.push(0xa0 | count as u8);
    } else {
        root.push(0xaf);
        root.extend_from_slice(&plist_integer(count));
    }
    for index in 1..=count {
        let mut bytes = vec![0u8; ref_width];
        for shift in (0..ref_width).rev() {
            bytes[shift] = ((index >> ((ref_width - 1 - shift) * 8)) & 0xff) as u8;
        }
        root.extend_from_slice(&bytes);
    }
    objects.push(root);
    for url in urls {
        objects.push(plist_string(url));
    }

    // Compute object offsets relative to the 8-byte bplist00 header.
    let mut offsets = Vec::with_capacity(objects.len());
    let mut cursor = 8usize;
    for object in &objects {
        offsets.push(cursor);
        cursor += object.len();
    }
    let offset_table_offset = cursor;
    let offset_width = int_byte_width(offset_table_offset);

    let mut out = Vec::with_capacity(offset_table_offset + offset_width + 32);
    out.extend_from_slice(b"bplist00");
    for object in &objects {
        out.extend_from_slice(object);
    }
    for offset in &offsets {
        for shift in (0..offset_width).rev() {
            out.push((offset >> (shift * 8)) as u8);
        }
    }

    // 32-byte trailer: 6 unused, offsetIntSize, objectRefSize, numObjects,
    // topObject, offsetTableOffset (all big-endian).
    out.extend_from_slice(&[0u8; 6]);
    out.push(offset_width as u8);
    out.push(ref_width as u8);
    out.extend_from_slice(&(objects.len() as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&(offset_table_offset as u64).to_be_bytes());
    out
}

/// Build argv (excluding the binary) for `xattr -w -x <attr> <hex> <path>`.
pub fn where_froms_xattr_args(path: &Path, urls: &[String]) -> Vec<String> {
    let hex = hex_encode(&binary_plist_string_array(urls));
    vec![
        "-w".to_string(),
        "-x".to_string(),
        WHERE_FROMS_ATTR.to_string(),
        hex,
        path.to_string_lossy().into_owned(),
    ]
}

fn hex_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Escape a token for the `security -i` interactive command grammar.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn keychain_suffix(keychain: Option<&str>) -> String {
    keychain
        .map(|k| format!(" \"{}\"", escape(k)))
        .unwrap_or_default()
}

/// Build the `add-generic-password` line fed to `security -i` stdin.
///
/// The secret appears **only** in this stdin payload — argv is just `["-i"]`.
pub fn keychain_add_command(
    service: &str,
    account: &str,
    secret: &str,
    keychain: Option<&str>,
) -> String {
    format!(
        "add-generic-password -a \"{}\" -s \"{}\" -U -w \"{}\"{}\n",
        escape(account),
        escape(service),
        escape(secret),
        keychain_suffix(keychain),
    )
}

/// Build the `find-generic-password` line fed to `security -i` stdin.
pub fn keychain_find_command(service: &str, account: &str, keychain: Option<&str>) -> String {
    format!(
        "find-generic-password -a \"{}\" -s \"{}\" -w{}\n",
        escape(account),
        escape(service),
        keychain_suffix(keychain),
    )
}

/// Build the `delete-generic-password` line fed to `security -i` stdin.
pub fn keychain_delete_command(service: &str, account: &str, keychain: Option<&str>) -> String {
    format!(
        "delete-generic-password -a \"{}\" -s \"{}\"{}\n",
        escape(account),
        escape(service),
        keychain_suffix(keychain),
    )
}

/// argv for `codesign --verify --verbose <path>` (parity row L33).
pub fn codesign_verify_args(path: &Path) -> Vec<String> {
    vec![
        "--verify".into(),
        "--verbose".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// argv for `spctl --assess --type execute --verbose <path>` (parity row L33).
pub fn spctl_assess_args(path: &Path) -> Vec<String> {
    vec![
        "--assess".into(),
        "--type".into(),
        "execute".into(),
        "--verbose".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// argv for ad-hoc `codesign --force --sign - <path>` (parity row L33).
pub fn codesign_adhoc_sign_args(path: &Path) -> Vec<String> {
    vec![
        "--force".into(),
        "--sign".into(),
        "-".into(),
        path.to_string_lossy().into_owned(),
    ]
}

fn tool_path(name: &str, absolute: &str) -> String {
    if Path::new(absolute).exists() {
        absolute.to_string()
    } else {
        name.to_string()
    }
}

/// Wait for `child` for at most `timeout`, killing it on expiry.
///
/// A status-waiter thread polls `Child::try_wait` in short intervals while the
/// caller uses `mpsc::recv_timeout`. Releasing the mutex between polls lets the
/// caller kill the child promptly on expiry. stdout/stderr are drained in their
/// own threads so a chatty child cannot deadlock on a full pipe.
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

/// Why a `security -i` invocation did not yield a token.
#[derive(Debug)]
enum SecurityFailure {
    Io(io::Error),
    Timeout(Duration),
    NonZero(String),
}

impl From<SecurityFailure> for Error {
    fn from(failure: SecurityFailure) -> Self {
        match failure {
            SecurityFailure::Io(error) => Error::Io(error),
            SecurityFailure::Timeout(timeout) => Error::failure(format!(
                "security -i timed out after {}s",
                timeout.as_secs()
            )),
            SecurityFailure::NonZero(stderr) => {
                Error::failure(format!("security -i failed: {stderr}"))
            }
        }
    }
}

/// Run `security -i` with `command` on stdin; return its trimmed stdout.
fn run_security_raw(command: &str) -> std::result::Result<String, SecurityFailure> {
    let binary = tool_path("security", "/usr/bin/security");
    let mut child = Command::new(binary)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(SecurityFailure::Io)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(command.as_bytes())
            .map_err(SecurityFailure::Io)?;
    }
    let output = wait_with_timeout(child, SECURITY_TIMEOUT)
        .map_err(|_| SecurityFailure::Timeout(SECURITY_TIMEOUT))?;
    if !output.status.success() {
        return Err(SecurityFailure::NonZero(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `security -i`, mapping every failure to `paradl_core::Error`.
fn run_security(command: &str) -> Result<String> {
    run_security_raw(command).map_err(Error::from)
}

/// macOS [`Platform`] implementation using only external processes.
#[derive(Debug, Clone, Default)]
pub struct MacOSPlatform {
    /// Optional keychain path appended to `security` commands.
    pub keychain: Option<String>,
}

impl Platform for MacOSPlatform {
    fn set_origin_metadata(&self, path: &Path, urls: &[String]) -> Result<()> {
        if !cfg!(target_os = "macos") || urls.is_empty() || !path.exists() {
            return Ok(());
        }
        let binary = tool_path("xattr", "/usr/bin/xattr");
        let args = where_froms_xattr_args(path, urls);
        let child = Command::new(binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let output = wait_with_timeout(child, XATTR_TIMEOUT).map_err(|error| {
            Error::failure(format!(
                "xattr -w -x {WHERE_FROMS_ATTR} timed out after {}s: {error}",
                XATTR_TIMEOUT.as_secs()
            ))
        })?;
        let status = output.status;
        if status.success() {
            Ok(())
        } else {
            Err(Error::failure(format!(
                "xattr -w -x {WHERE_FROMS_ATTR} failed for {}",
                path.display()
            )))
        }
    }

    fn store_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        if !cfg!(target_os = "macos") {
            return Ok(());
        }
        if service.is_empty() || account.is_empty() || secret.is_empty() {
            return Err(Error::usage(
                "keychain service/account/secret must be non-empty",
            ));
        }
        let command = keychain_add_command(service, account, secret, self.keychain.as_deref());
        run_security(&command).map(|_| ())
    }

    fn load_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
        if !cfg!(target_os = "macos") || account.is_empty() {
            return Ok(None);
        }
        let command = keychain_find_command(service, account, self.keychain.as_deref());
        match run_security_raw(&command) {
            Ok(value) => Ok((!value.is_empty()).then_some(value)),
            // A missing keychain item is a normal "not found"; a timeout is a
            // real failure and must not silently report `None`.
            Err(SecurityFailure::Timeout(timeout)) => {
                Err(Error::from(SecurityFailure::Timeout(timeout)))
            }
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn int_byte_width_matches_plistlib() {
        assert_eq!(int_byte_width(0), 1);
        assert_eq!(int_byte_width(255), 1);
        assert_eq!(int_byte_width(256), 2);
        assert_eq!(int_byte_width(65535), 2);
        assert_eq!(int_byte_width(65536), 4);
    }

    #[test]
    fn codesign_args_are_exact() {
        let path = Path::new("/Applications/App.app");
        assert_eq!(
            codesign_verify_args(path),
            vec!["--verify", "--verbose", "/Applications/App.app"]
        );
        assert_eq!(
            codesign_adhoc_sign_args(path),
            vec!["--force", "--sign", "-", "/Applications/App.app"]
        );
    }

    #[test]
    fn oracle_timeout_constants_are_exact() {
        assert_eq!(SECURITY_TIMEOUT, Duration::from_secs(5));
        assert_eq!(XATTR_TIMEOUT, Duration::from_secs(3));
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
            .args(["-c", "printf hello"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_with_timeout(child, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hello");
    }
}
