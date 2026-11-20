//! Windows platform implementation.
//!
//! Mirrors `src/paradl/platform/windows.py` for the `Zone.Identifier` alternate
//! data stream (byte-exact CRLF INI, parity row L10) and the user-scoped secret
//! store (parity row L11). All helpers are pure and cross-platform testable;
//! the trait methods degrade to safe no-ops when Windows is unavailable.
//!
//! ```
//! # #[cfg(feature = "platform-windows")]
//! # {
//! use paradl_platform::windows::format_zone_identifier_text;
//! assert_eq!(
//!     format_zone_identifier_text(Some("https://example.com/a.exe"), None),
//!     "[ZoneTransfer]\r\nHostUrl=https://example.com/a.exe\r\n",
//! );
//! # }
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use paradl_core::Error;

use crate::{Platform, Result};

/// Name of the Zone.Identifier alternate data stream.
pub const ZONE_IDENTIFIER_STREAM: &str = "Zone.Identifier";
/// Section header of the Zone.Identifier INI body.
pub const ZONE_TRANSFER_SECTION: &str = "[ZoneTransfer]";

/// Format the CRLF-delimited Zone.Identifier INI body (windows.py:376-383).
///
/// Byte-exact golden: `"[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=<url>\r\n"`.
pub fn format_zone_identifier_text(host_url: Option<&str>, zone_id: Option<i64>) -> String {
    let mut lines = vec![ZONE_TRANSFER_SECTION.to_string()];
    if let Some(zone) = zone_id {
        lines.push(format!("ZoneId={zone}"));
    }
    if let Some(url) = host_url {
        lines.push(format!("HostUrl={url}"));
    }
    format!("{}\r\n", lines.join("\r\n"))
}

/// Parse a Zone.Identifier INI body into a case-preserving map.
///
/// Keys inside `[ZoneTransfer]` keep their original case, matching
/// `configparser` with `optionxform = str` (windows.py:354-373).
pub fn parse_zone_identifier_text(text: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == ZONE_TRANSFER_SECTION;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            result.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    result
}

/// Strip `ZoneId` while preserving `HostUrl` (windows.py:386-396).
pub fn unquarantine_zone_identifier_text(text: &str) -> String {
    let mut kept: Vec<(String, String)> = Vec::new();
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == ZONE_TRANSFER_SECTION;
            continue;
        }
        if !in_section || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && !key.trim().eq_ignore_ascii_case("zoneid")
        {
            kept.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    if kept.is_empty() {
        return String::new();
    }
    let mut lines = vec![ZONE_TRANSFER_SECTION.to_string()];
    for (key, value) in kept {
        let name = if key.eq_ignore_ascii_case("hosturl") {
            "HostUrl".to_string()
        } else {
            key
        };
        lines.push(format!("{name}={value}"));
    }
    format!("{}\r\n", lines.join("\r\n"))
}

/// Alternate-data-stream path string (`<file>:Zone.Identifier`).
pub fn ads_path(path: &Path) -> String {
    format!("{}:{ZONE_IDENTIFIER_STREAM}", path.display())
}

/// Return the user-scoped credential store path (windows.py:613-620).
fn default_store_path() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA")
        && !local.is_empty()
    {
        return PathBuf::from(local).join("pdl").join("credentials.json");
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config").join("pdl").join("credentials.json")
}

fn store_key(service: &str, account: &str) -> String {
    format!("{service}:{account}")
}

fn read_store(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        },
        Err(_) => serde_json::Map::new(),
    }
}

fn write_store(path: &Path, map: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(map)
        .map_err(|e| Error::failure(format!("credential store serialization failed: {e}")))?;
    fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Windows [`Platform`] implementation.
#[derive(Debug, Clone, Default)]
pub struct WindowsPlatform {
    /// Override for the credential store path (used in tests).
    pub store_path: Option<PathBuf>,
}

impl WindowsPlatform {
    fn credentials_path(&self) -> PathBuf {
        self.store_path.clone().unwrap_or_else(default_store_path)
    }

    fn write_ads(&self, path: &Path, text: &str) -> Result<()> {
        let ads = ads_path(path);
        fs::write(&ads, text.as_bytes()).map_err(|e| {
            Error::failure(format!(
                "failed to write :Zone.Identifier stream to {}: {e}",
                path.display()
            ))
        })
    }

    fn read_ads(&self, path: &Path) -> Option<String> {
        fs::read_to_string(ads_path(path)).ok()
    }
}

impl Platform for WindowsPlatform {
    fn set_origin_metadata(&self, path: &Path, urls: &[String]) -> Result<()> {
        let Some(primary) = urls.first() else {
            return Ok(());
        };
        if primary.is_empty() || !path.exists() {
            return Ok(());
        }
        let zone_id = self.read_ads(path).and_then(|text| {
            parse_zone_identifier_text(&text)
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("zoneid"))
                .and_then(|(_, value)| value.parse::<i64>().ok())
        });
        self.write_ads(path, &format_zone_identifier_text(Some(primary), zone_id))
    }

    fn store_secret(&self, service: &str, account: &str, secret: &str) -> Result<()> {
        if service.is_empty() || account.is_empty() || secret.is_empty() {
            return Err(Error::usage("service/account/secret must be non-empty"));
        }
        let path = self.credentials_path();
        let mut map = read_store(&path);
        map.insert(
            store_key(service, account),
            serde_json::Value::String(secret.to_string()),
        );
        write_store(&path, &map)
    }

    fn load_secret(&self, service: &str, account: &str) -> Result<Option<String>> {
        let path = self.credentials_path();
        Ok(read_store(&path)
            .get(&store_key(service, account))
            .and_then(|value| value.as_str())
            .map(str::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_preserves_case_and_section_scope() {
        let parsed = parse_zone_identifier_text(
            "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/x\r\n[Other]\r\nKey=v\r\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.get("ZoneId").map(String::as_str), Some("3"));
        assert!(!parsed.contains_key("Key"));
    }

    #[test]
    fn unquarantine_with_only_zone_id_is_empty() {
        assert_eq!(
            unquarantine_zone_identifier_text("[ZoneTransfer]\r\nZoneId=3\r\n"),
            ""
        );
    }
}
