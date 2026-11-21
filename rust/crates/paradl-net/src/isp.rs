//! ISP auto-detection and the W0-3 `"auto"` sentinel resolution handoff.
//!
//! `paradl_core::PDLConfig::get_effective_isp()` returns either the configured
//! ISP string or the literal `"auto"`. This module supplies the detection
//! capability that resolves that sentinel in `paradl-net`; the wiring is
//! performed at integration time (W4-1). `paradl-core` is never modified.
//!
//! Detection mirrors `src/paradl/net/client.py:detect_isp`: three tiers
//! (ip-api.com → ipinfo.io → Cloudflare trace + RDAP) with short timeouts, a
//! five-minute success cache, and a custom-detector override for tests.
//! Transient failures return [`UNKNOWN_ISP`] and are never cached.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::debug;

/// The configured value that requests dynamic ISP detection.
pub const AUTO_ISP_SENTINEL: &str = "auto";
/// Returned when every detection tier fails.
pub const UNKNOWN_ISP: &str = "Unknown ISP";
/// How long a successful detection is cached.
pub const ISP_CACHE_TTL: Duration = Duration::from_secs(300);

type IspDetector = Arc<dyn Fn() -> Option<String> + Send + Sync>;

static ISP_DETECTOR: OnceLock<Mutex<Option<IspDetector>>> = OnceLock::new();
static ISP_CACHE: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();

fn lock_detector() -> std::sync::MutexGuard<'static, Option<IspDetector>> {
    ISP_DETECTOR
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn lock_cache() -> std::sync::MutexGuard<'static, Option<(Instant, String)>> {
    ISP_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// Install a custom ISP detector (used by tests and deterministic overrides).
///
/// Passing `None` clears the override. The returned string is taken verbatim
/// after trimming; an override returning `None` yields [`UNKNOWN_ISP`].
pub fn set_isp_detector<F>(detector: Option<F>)
where
    F: Fn() -> Option<String> + Send + Sync + 'static,
{
    *lock_detector() = detector.map(|function| Arc::new(function) as IspDetector);
}

/// Remove any custom detector and clear the successful-detection cache.
pub fn reset_isp_detector() {
    *lock_detector() = None;
    *lock_cache() = None;
}

fn custom_detector() -> Option<IspDetector> {
    lock_detector().clone()
}

fn cache_result(name: &str) {
    *lock_cache() = Some((Instant::now(), name.to_owned()));
}

fn cached_name() -> Option<String> {
    let cache = lock_cache();
    let (timestamp, name) = cache.as_ref()?;
    if timestamp.elapsed() < ISP_CACHE_TTL && !name.is_empty() {
        Some(name.clone())
    } else {
        None
    }
}

/// Clean and normalize a raw ISP/organization name.
///
/// Strips leading AS numbers (`AS149048 -`, `AS13335:`) and surrounding
/// quotes/whitespace, mirroring `clean_isp_name` in the Python oracle.
pub fn clean_isp_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut rest = trimmed;
    if trimmed.to_ascii_lowercase().starts_with("as") {
        let bytes = trimmed.as_bytes();
        let mut index = 2;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index > 2 {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            while index < bytes.len() && matches!(bytes[index], b'-' | b'_' | b':') {
                index += 1;
            }
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            rest = &trimmed[index..];
        }
    }
    rest.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .to_owned()
}

fn plausible(name: &str) -> bool {
    !name.is_empty()
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "unknown" | "unknown isp" | "private" | "local"
        )
}

fn blocking_text(url: &str, timeout: Duration) -> Option<String> {
    let client = reqwest::blocking::Client::builder().timeout(timeout).build().ok()?;
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().ok()
}

fn blocking_json(url: &str, timeout: Duration) -> Option<Value> {
    let body = blocking_text(url, timeout)?;
    serde_json::from_str(&body).ok()
}

fn rdap_name(value: &Value) -> String {
    if let Some(entities) = value.get("entities").and_then(Value::as_array) {
        for entity in entities {
            if let Some(vcard) = entity.get("vcardArray").and_then(Value::as_array)
                && let Some(entries) = vcard.get(1).and_then(Value::as_array)
            {
                for entry in entries {
                    if let Some(parts) = entry.as_array()
                        && parts.first().and_then(Value::as_str) == Some("fn")
                        && let Some(name) = parts.get(3).and_then(Value::as_str)
                    {
                        return name.trim().to_owned();
                    }
                }
            }
        }
    }
    if let Some(remarks) = value.get("remarks").and_then(Value::as_array)
        && let Some(description) = remarks.first().and_then(|r| r.get("description"))
        && let Some(name) = description.as_array().and_then(|a| a.first())
        && let Some(name) = name.as_str()
    {
        return name.trim().to_owned();
    }
    value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// Auto-detect the active ISP with the default 2.5-second timeout.
///
/// ```
/// use paradl_net::detect_isp;
/// let isp = detect_isp(); // "Unknown ISP" offline; never panics
/// assert!(!isp.is_empty());
/// ```
pub fn detect_isp() -> String {
    detect_isp_with(Duration::from_millis(2500), false)
}

/// Auto-detect the ISP with an explicit timeout and optional cache bypass.
pub fn detect_isp_with(timeout: Duration, force_refresh: bool) -> String {
    if let Some(detector) = custom_detector() {
        return match detector() {
            Some(name) if !name.trim().is_empty() => name.trim().to_owned(),
            _ => UNKNOWN_ISP.to_owned(),
        };
    }

    if !force_refresh
        && let Some(name) = cached_name()
    {
        return name;
    }
    let short = timeout.min(Duration::from_secs(2));

    // Tier 1: ip-api.com (explicit `isp` / `org` fields).
    if let Some(data) = blocking_json("http://ip-api.com/json", short)
        && data.get("status").and_then(Value::as_str) == Some("success")
    {
        let raw = data
            .get("isp")
            .or_else(|| data.get("org"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let name = clean_isp_name(raw);
        if plausible(&name) {
            cache_result(&name);
            return name;
        }
    }
    debug!("ISP detection tier 1 (ip-api) did not yield a name");

    // Tier 2: ipinfo.io (`org` contains the AS number and organization).
    if let Some(data) = blocking_json("https://ipinfo.io/json", short) {
        let raw = data.get("org").and_then(Value::as_str).unwrap_or("");
        let name = clean_isp_name(raw);
        if plausible(&name) {
            cache_result(&name);
            return name;
        }
    }
    debug!("ISP detection tier 2 (ipinfo) did not yield a name");

    // Tier 3: Cloudflare trace -> public IP -> RDAP organization.
    if let Some(trace) = blocking_text("https://1.1.1.1/cdn-cgi/trace", short)
        && let Some(ip) = trace
            .lines()
            .find_map(|line| line.strip_prefix("ip=").map(str::trim))
            .filter(|ip| !ip.is_empty())
    {
        let url = format!("https://rdap.org/ip/{ip}");
        if let Some(data) = blocking_json(&url, short) {
            let name = clean_isp_name(&rdap_name(&data));
            if plausible(&name) {
                cache_result(&name);
                return name;
            }
        }
    }

    UNKNOWN_ISP.to_owned()
}

/// Resolve the configured ISP value: pass through explicit names, detect when
/// the value is the `"auto"` sentinel (case-insensitive).
///
/// ```
/// use paradl_net::resolve_effective_isp;
/// assert_eq!(resolve_effective_isp("Comcast Cable"), "Comcast Cable");
/// ```
pub fn resolve_effective_isp(configured: &str) -> String {
    let configured = configured.trim();
    if !configured.eq_ignore_ascii_case(AUTO_ISP_SENTINEL) {
        configured.to_owned()
    } else {
        detect_isp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_isp_name_handles_variants() {
        assert_eq!(clean_isp_name("AS13335 - Cloudflare"), "Cloudflare");
        assert_eq!(clean_isp_name("AS149048: Example"), "Example");
        assert_eq!(clean_isp_name("'Telco'"), "Telco");
    }
}
