//! Apple IPSW site engine: device aliases, API lookup and signed build selection.
//!
//! Ports `src/paradl/engines/ipsw.py` resolution. SHA-256 verification after
//! the transfer is owned by `paradl-postproc` (W2-2).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use paradl_core::{Error, TargetType};
use regex::Regex;
use serde_json::Value;

use super::token::SecretProvider;
use super::SiteHttp;
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::Result;

static FILENAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9,]+)_([0-9.]+)_([A-Za-z0-9]+)_Restore\.ipsw")
        .expect("IPSW filename regex must compile")
});

/// Device aliases from `IpswEngine.DEVICE_ALIASES`.
pub const DEVICE_ALIASES: &[(&str, &str)] = &[
    ("iphone 16", "iPhone17,3"),
    ("iphone 16 plus", "iPhone17,4"),
    ("iphone 16 pro", "iPhone17,1"),
    ("iphone 16 pro max", "iPhone17,2"),
    ("iphone 15", "iPhone15,4"),
    ("iphone 15 plus", "iPhone15,5"),
    ("iphone 15 pro", "iPhone16,1"),
    ("iphone 15 pro max", "iPhone16,2"),
    ("iphone 14", "iPhone14,7"),
    ("iphone 14 plus", "iPhone14,8"),
    ("iphone 14 pro", "iPhone15,2"),
    ("iphone 14 pro max", "iPhone15,3"),
    ("iphone 13", "iPhone14,5"),
    ("iphone 13 mini", "iPhone14,4"),
    ("iphone 13 pro", "iPhone14,2"),
    ("iphone 13 pro max", "iPhone14,3"),
    ("iphone 12", "iPhone13,2"),
    ("iphone 12 mini", "iPhone13,1"),
    ("iphone 12 pro", "iPhone13,3"),
    ("iphone 12 pro max", "iPhone13,4"),
    ("iphone 11", "iPhone12,1"),
    ("iphone 11 pro", "iPhone12,3"),
    ("iphone 11 pro max", "iPhone12,5"),
    ("iphone se 2", "iPhone12,8"),
    ("iphone se 3", "iPhone14,6"),
];

/// Return `true` when `target` looks like an IPSW URL or shortcut.
pub fn is_ipsw(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    if lower.starts_with("ipsw:") || lower.starts_with("ipsw/") {
        return true;
    }
    if lower.contains("ipsw.me") {
        return true;
    }
    lower.contains(".ipsw")
        && (lower.contains("updates.cdn-apple.com") || lower.starts_with("http"))
}

/// Resolve a device name/identifier to the Apple hardware identifier.
pub fn resolve_device_identifier(value: &str) -> String {
    let mut text = value.trim();
    if let Some(rest) = text
        .strip_prefix("ipsw:")
        .or_else(|| text.strip_prefix("ipsw/"))
    {
        text = rest.trim();
    }
    let cleaned = text.to_ascii_lowercase().replace(['-', '_'], " ");
    let cleaned = cleaned.trim();
    for (alias, identifier) in DEVICE_ALIASES {
        if cleaned == *alias || cleaned == identifier.to_ascii_lowercase() {
            return (*identifier).to_owned();
        }
    }
    text.to_owned()
}

/// Metadata parsed from an Apple IPSW filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpswFilenameMeta {
    /// Hardware identifier (e.g. `iPhone17,3`).
    pub identifier: String,
    /// iOS version (e.g. `18.0`).
    pub version: String,
    /// Build identifier (e.g. `22A5297f`).
    pub build: String,
}

/// Parse `<ident>_<version>_<build>_Restore.ipsw` from a filename.
pub fn parse_ipsw_filename(filename: &str) -> Option<IpswFilenameMeta> {
    let caps = FILENAME_RE.captures(filename)?;
    Some(IpswFilenameMeta {
        identifier: caps[1].to_owned(),
        version: caps[2].to_owned(),
        build: caps[3].to_owned(),
    })
}

fn basename(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url).split('#').next().unwrap_or(url);
    clean.rsplit('/').next().unwrap_or(clean).to_owned()
}

/// [`TargetResolver`] for Apple IPSW firmware targets.
#[derive(Clone)]
pub struct IpswResolver {
    http: Arc<dyn SiteHttp>,
    offline: bool,
}

impl IpswResolver {
    /// Build a resolver with production HTTP I/O.
    pub fn new<H: SiteHttp + 'static>(
        http: H,
        secrets: Arc<dyn SecretProvider>,
    ) -> Self {
        Self::from_shared(Arc::new(http), secrets)
    }

    /// Build a resolver sharing a [`SiteHttp`] instance.
    pub fn from_shared(http: Arc<dyn SiteHttp>, _secrets: Arc<dyn SecretProvider>) -> Self {
        Self {
            http,
            offline: false,
        }
    }

    /// Emit the offline synthetic metadata used by classification tests.
    pub fn with_offline(mut self, enabled: bool) -> Self {
        self.offline = enabled;
        self
    }

    fn asset(&self, url: &str, filename: &str, digest: Option<&str>, size: Option<u64>) -> ResolvedAsset {
        ResolvedAsset {
            url: url.to_owned(),
            filename: Some(filename.to_owned()),
            content_length: size,
            digest: digest.filter(|value| !value.is_empty()).map(|value| format!("sha256:{value}")),
            target_type: TargetType::IpSw,
            ..Default::default()
        }
    }
}

#[async_trait]
impl TargetResolver for IpswResolver {
    fn scheme(&self) -> &'static str {
        "ipsw"
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let target = target.trim();
        if target.to_ascii_lowercase().starts_with("http") {
            let filename = basename(target);
            let parsed = parse_ipsw_filename(&filename);
            if self.offline || parsed.is_none() {
                return Ok(vec![self.asset(target, &filename, None, None)]);
            }
            let meta = parsed.expect("parsed above");
            let api_url = format!("https://api.ipsw.me/v4/device/{}?type=ipsw", meta.identifier);
            let response = self
                .http
                .fetch_json(&api_url, &HashMap::new(), ctx.timeout)
                .await?;
            if response.status == 200
                && response.body.is_object()
                && let Some(firmwares) = response.body.get("firmwares").and_then(Value::as_array)
            {
                for firmware in firmwares {
                    let build = firmware.get("buildid").and_then(Value::as_str);
                    let url = firmware.get("url").and_then(Value::as_str);
                    if build == Some(meta.build.as_str()) || url == Some(target) {
                        let digest = firmware.get("sha256sum").and_then(Value::as_str);
                        let size = firmware.get("filesize").and_then(Value::as_u64);
                        return Ok(vec![self.asset(
                            url.unwrap_or(target),
                            &filename,
                            digest,
                            size,
                        )]);
                    }
                }
            }
            return Ok(vec![self.asset(target, &filename, None, None)]);
        }

        let identifier = resolve_device_identifier(target);
        if self.offline {
            return Ok(vec![self.asset(
                &format!("https://api.ipsw.me/v4/ipsw/download/{identifier}/latest"),
                &format!("{identifier}_Restore.ipsw"),
                None,
                None,
            )]);
        }
        let api_url = format!("https://api.ipsw.me/v4/device/{identifier}?type=ipsw");
        let response = self
            .http
            .fetch_json(&api_url, &HashMap::new(), ctx.timeout)
            .await?;
        if response.status != 200 || !response.body.is_object() {
            return Err(Error::failure(format!(
                "No signed firmware found for {identifier}"
            )));
        }
        let Some(firmware) = response
            .body
            .get("firmwares")
            .and_then(Value::as_array)
            .and_then(|firmwares| firmwares.iter().find(|fw| fw.get("signed") == Some(&Value::Bool(true))))
        else {
            return Err(Error::failure(format!(
                "No signed firmware found for {identifier}"
            )));
        };
        let url = firmware
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::failure("IPSW firmware has no URL"))?;
        let digest = firmware.get("sha256sum").and_then(Value::as_str);
        let size = firmware.get("filesize").and_then(Value::as_u64);
        Ok(vec![self.asset(url, &basename(url), digest, size)])
    }
}
