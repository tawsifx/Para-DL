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

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use paradl_core::Error;

use crate::{Platform, Result};

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

/// Run `security -i` with `command` on stdin; return its trimmed stdout.
fn run_security(command: &str) -> Result<String> {
    let binary = tool_path("security", "/usr/bin/security");
    let mut child = Command::new(binary)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(command.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(Error::failure(format!(
            "security -i failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
        let status = Command::new(binary).args(args).output()?.status;
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
        match run_security(&command) {
            Ok(value) if !value.is_empty() => Ok(Some(value)),
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
