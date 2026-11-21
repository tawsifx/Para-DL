//! Byte-exact ports of the Python oracle's data models and helper functions.
//!
//! Every numeric constant (1024 multipliers, bit-to-byte `/8`, 200-character
//! filename cap) and every domain/extension list is mirrored from
//! `src/paradl/core/models.py`. The domain lists and extension set are each
//! defined exactly once and all helpers read from those single constants.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// Known direct-download file extensions (the single source of truth).
///
/// Mirrors `paradl.core.models.DIRECT_EXTENSIONS`. Use [`has_direct_extension`]
/// for URL/filename suffix matching rather than duplicating this list.
pub const DIRECT_EXTENSIONS: &[&str] = &[
    ".zip",
    ".tar",
    ".gz",
    ".tgz",
    ".bz2",
    ".tbz2",
    ".xz",
    ".txz",
    ".7z",
    ".rar",
    ".iso",
    ".img",
    ".dmg",
    ".pkg",
    ".deb",
    ".rpm",
    ".apk",
    ".ipa",
    ".ipsw",
    ".bin",
    ".exe",
    ".msi",
    ".appimage",
    ".safetensors",
    ".gguf",
    ".pt",
    ".pth",
    ".onnx",
    ".ckpt",
    ".parquet",
    ".csv",
    ".jsonl",
    ".mp4",
    ".mkv",
    ".avi",
    ".mov",
    ".webm",
    ".flv",
    ".mp3",
    ".m4a",
    ".flac",
    ".wav",
    ".opus",
    ".aac",
    ".ogg",
    ".pdf",
    ".epub",
    ".doc",
    ".docx",
    ".png",
    ".jpg",
    ".jpeg",
    ".webp",
    ".gif",
    ".svg",
    ".ttf",
    ".otf",
    ".woff",
    ".woff2",
    ".eot",
    ".whl",
    ".meta4",
    ".metalink",
];

/// Known streaming-media domains, matched by hostname suffix.
///
/// Mirrors `paradl.core.models.MEDIA_DOMAINS` in declaration order.
pub const MEDIA_DOMAINS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "soundcloud.com",
    "vimeo.com",
    "twitch.tv",
    "bandcamp.com",
    "bilibili.com",
    "dailymotion.com",
    "mixcloud.com",
    "facebook.com",
    "instagram.com",
    "tiktok.com",
    "twitter.com",
    "x.com",
];

/// Known image-generation / image-CDN domains, matched by hostname suffix.
///
/// Mirrors `paradl.core.models.IMAGE_DOMAINS` in declaration order.
pub const IMAGE_DOMAINS: &[&str] = &[
    "image.pollinations.ai",
    "images.unsplash.com",
    "i.imgur.com",
    "i.redd.it",
    "cdn.discordapp.com",
];

const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The kind of target a URL/search term was classified as.
///
/// Mirrors the string values of `paradl.core.models.TargetType` (a `StrEnum`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetType {
    /// A torrent/magnet target (`"torrent"`).
    #[default]
    Torrent,
    /// A direct file download (`"direct"`).
    Direct,
    /// A streaming-media target (`"stream"`).
    Stream,
    /// A search query (`"search"`).
    Search,
    /// A GitHub release or repository (`"github"`).
    Github,
    /// A Hugging Face model/dataset (`"huggingface"`).
    #[serde(rename = "huggingface")]
    Hf,
    /// An Apple IPSW firmware (`"ipsw"`).
    IpSw,
    /// A PyPI package (`"pypi"`).
    Pypi,
    /// A Homebrew formula/cask (`"brew"`).
    Brew,
}

impl TargetType {
    /// Return the Python `StrEnum` string value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Torrent => "torrent",
            Self::Direct => "direct",
            Self::Stream => "stream",
            Self::Search => "search",
            Self::Github => "github",
            Self::Hf => "huggingface",
            Self::IpSw => "ipsw",
            Self::Pypi => "pypi",
            Self::Brew => "brew",
        }
    }
}

impl fmt::Display for TargetType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TargetType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "torrent" => Ok(Self::Torrent),
            "direct" => Ok(Self::Direct),
            "stream" => Ok(Self::Stream),
            "search" => Ok(Self::Search),
            "github" => Ok(Self::Github),
            "huggingface" | "hf" => Ok(Self::Hf),
            "ipsw" => Ok(Self::IpSw),
            "pypi" => Ok(Self::Pypi),
            "brew" => Ok(Self::Brew),
            _ => Err(Error::usage(format!("Unknown target type: {s:?}"))),
        }
    }
}

/// Adaptive watchdog mode.
///
/// Mirrors `paradl.core.models.WatchdogMode` (a `StrEnum`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WatchdogMode {
    /// Version 1 watchdog (`"v1"`).
    V1,
    /// Version 2 watchdog (`"v2"`, the default).
    #[default]
    V2,
    /// Watchdog disabled (`"off"`).
    Off,
}

impl WatchdogMode {
    /// Return the Python `StrEnum` string value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
            Self::Off => "off",
        }
    }
}

impl fmt::Display for WatchdogMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WatchdogMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "v1" => Ok(Self::V1),
            "v2" => Ok(Self::V2),
            "off" => Ok(Self::Off),
            _ => Err(Error::usage(format!("Invalid watchdog mode: {s:?}"))),
        }
    }
}

/// Input accepted by [`parse_rate`], mirroring Python's `str | int | float | None`.
#[derive(Debug, Clone, PartialEq)]
pub enum RateInput {
    /// Python `None` — always parses to `0.0`.
    None,
    /// An integer count of bytes per second.
    Int(i64),
    /// A floating-point count of bytes per second.
    Float(f64),
    /// A human-readable rate string (e.g. `"4MBps"`, `"1Mbps"`).
    Text(String),
}

impl From<&str> for RateInput {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}

impl From<String> for RateInput {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<i32> for RateInput {
    fn from(n: i32) -> Self {
        Self::Int(i64::from(n))
    }
}

impl From<i64> for RateInput {
    fn from(n: i64) -> Self {
        Self::Int(n)
    }
}

impl From<f64> for RateInput {
    fn from(n: f64) -> Self {
        Self::Float(n)
    }
}

impl<T> From<Option<T>> for RateInput
where
    T: Into<RateInput>,
{
    fn from(opt: Option<T>) -> Self {
        opt.map_or(Self::None, Into::into)
    }
}

/// Parse a human-readable transfer rate into bytes per second.
///
/// Uses binary multipliers (`1 K = 1024`) consistent with aria2, and divides
/// bit units by 8. Mirrors `paradl.core.models.parse_rate` exactly; unparseable
/// or negative specifications return [`Error::Usage`].
///
/// ```
/// use paradl_core::parse_rate;
/// assert_eq!(parse_rate("1M").unwrap(), 1024.0 * 1024.0);
/// assert_eq!(parse_rate("1Mbps").unwrap(), 1024.0 * 1024.0 / 8.0);
/// assert_eq!(parse_rate("off").unwrap(), 0.0);
/// ```
pub fn parse_rate(value: impl Into<RateInput>) -> Result<f64, Error> {
    match value.into() {
        RateInput::None => Ok(0.0),
        RateInput::Int(n) if n < 0 => Err(Error::usage(format!("Rate cannot be negative: {n}"))),
        RateInput::Int(n) => Ok(n as f64),
        RateInput::Float(n) if n < 0.0 => Err(Error::usage(format!(
            "Rate cannot be negative: {}",
            py_float_str(n)
        ))),
        RateInput::Float(n) => Ok(n),
        RateInput::Text(s) => parse_rate_str(&s),
    }
}

fn py_float_str(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn parse_rate_str(input: &str) -> Result<f64, Error> {
    let s = input.trim();
    if s.is_empty() {
        return Err(Error::usage("Empty rate string"));
    }
    if matches!(
        s.to_ascii_lowercase().as_str(),
        "0" | "off" | "none" | "false" | "disable" | "disabled"
    ) {
        return Ok(0.0);
    }

    let unparseable = || Error::usage(format!("Unparseable rate specification: '{input}'"));
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    let digits_start = i;
    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_start {
        return Err(unparseable());
    }
    if i < len && bytes[i] == b'.' {
        i += 1;
        let fraction_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == fraction_start {
            return Err(unparseable());
        }
    }
    let value: f64 = s[..i].parse().map_err(|_| unparseable())?;

    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    let mut multiplier = 1.0;
    if i < len
        && matches!(
            bytes[i],
            b'k' | b'K' | b'm' | b'M' | b'g' | b'G' | b't' | b'T'
        )
    {
        multiplier = match bytes[i].to_ascii_uppercase() {
            b'K' => 1024.0,
            b'M' => 1024.0_f64.powi(2),
            b'G' => 1024.0_f64.powi(3),
            b'T' => 1024.0_f64.powi(4),
            _ => unreachable!(),
        };
        i += 1;
    }
    // The Python regex `([kKmMgGtT])?(i)?` allows the IEC marker on its own
    // (e.g. "1ib" -> 0.125 B/s), so consume it outside the magnitude branch.
    if i < len && bytes[i] == b'i' {
        i += 1;
    }

    let rest = &s[i..];
    let unit = ["bytes", "byte", "bits", "bit", "B", "b"]
        .into_iter()
        .find(|token| rest.starts_with(token));
    if let Some(u) = unit {
        i += u.len();
    }

    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let remaining = &s[i..];
    if remaining.starts_with("ps") {
        i += 2;
    } else if remaining.starts_with("/sec") {
        i += 4;
    } else if remaining.starts_with("/s") {
        i += 2;
    }
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i != len {
        return Err(unparseable());
    }

    let is_bits = matches!(unit, Some("b" | "bit" | "bits"));
    let mut bps = value * multiplier;
    if is_bits {
        bps /= 8.0;
    }
    if value == 0.0 { Ok(0.0) } else { Ok(bps) }
}

/// Validate a rate string and convert it to aria2's integer byte-rate syntax.
///
/// Returns `Ok(None)` for zero/disabled rates and `Err` for invalid input,
/// mirroring `paradl.core.models.format_rate_for_aria2`.
pub fn format_rate_for_aria2(rate: &str) -> Result<Option<String>, Error> {
    let bps = parse_rate(rate)?;
    if bps <= 0.0 {
        return Ok(None);
    }
    Ok(Some((bps as u64).to_string()))
}

/// Sanitize a filename: replace unsafe characters, cap at 200 characters, and
/// neutralize dot-only and Windows reserved device names.
///
/// Byte-exact port of `paradl.core.models.sanitize_filename` (parity row L02).
///
/// ```
/// use paradl_core::sanitize_filename;
/// assert_eq!(sanitize_filename("a/b:c"), "a_b_c");
/// assert_eq!(sanitize_filename("CON"), "_CON");
/// assert_eq!(sanitize_filename("..."), "download");
/// ```
pub fn sanitize_filename(name: &str) -> String {
    const UNSAFE: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
    let translated: String = name
        .chars()
        .map(|c| if UNSAFE.contains(&c) { '_' } else { c })
        .collect();
    let cleaned: String = translated.trim().chars().take(200).collect();
    let stripped_dots = cleaned.trim_matches(|c| c == '.' || c == ' ');
    if stripped_dots.is_empty() || stripped_dots.chars().all(|c| c == '.') {
        return "download".to_owned();
    }
    let base_stem = stripped_dots
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if WINDOWS_RESERVED_NAMES.contains(&base_stem.as_str()) {
        return format!("_{cleaned}");
    }
    if cleaned.is_empty() {
        "download".to_owned()
    } else {
        cleaned
    }
}

/// Format a byte count the way `paradl.core.models.format_size` does.
pub fn format_size(bytes: Option<f64>) -> String {
    let Some(bytes) = bytes else {
        return "Unknown size".to_owned();
    };
    if bytes <= 0.0 {
        return "0 B".to_owned();
    }
    let mut value = bytes;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if value < 1024.0 {
            return format!("{value:.1} {unit}");
        }
        value /= 1024.0;
    }
    format!("{value:.1} PB")
}

/// Format a speed in bytes per second the way `paradl.core.models.format_speed` does.
pub fn format_speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / (1024.0 * 1024.0))
    } else if bps >= 1024.0 {
        format!("{:.1} KB/s", bps / 1024.0)
    } else {
        format!("{bps:.0} B/s")
    }
}

/// Extract a lower-cased hostname from a URL, mirroring `urlparse(url).hostname`.
fn hostname(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let candidate = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else {
        trimmed.to_owned()
    };
    let parsed = url::Url::parse(&candidate).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

fn matches_domain(host: &str, domains: &[&str]) -> bool {
    domains.iter().any(|domain| {
        let d = domain.to_ascii_lowercase();
        host == d || host.ends_with(&format!(".{d}"))
    })
}

/// Return `true` if `url` belongs to a known streaming-media domain.
///
/// Uses [`MEDIA_DOMAINS`] as the single source of truth.
pub fn is_media_domain(url: &str) -> bool {
    let Some(host) = hostname(url) else {
        return false;
    };
    matches_domain(&host, MEDIA_DOMAINS)
}

/// Return `true` if `url` belongs to an image-generation / image-CDN service.
///
/// Uses [`IMAGE_DOMAINS`] as the single source of truth and also reproduces the
/// special pollinations.ai `/prompt/` rule from the Python oracle.
pub fn is_image_domain(url: &str) -> bool {
    let Some(host) = hostname(url) else {
        return false;
    };
    if (host == "pollinations.ai" || host.ends_with(".pollinations.ai"))
        && (host.contains("image") || url.contains("/prompt/"))
    {
        return true;
    }
    matches_domain(&host, IMAGE_DOMAINS)
}

/// Return the matching [`DIRECT_EXTENSIONS`] entry for a URL or filename.
///
/// Query strings and fragments are stripped, the last path component's final
/// extension is lower-cased, and the result is looked up in the single constant.
pub fn direct_extension(path: &str) -> Option<&'static str> {
    let without_params = path.split(['?', '#']).next()?;
    let name = without_params.rsplit('/').next()?;
    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    let candidate = format!(".{extension}");
    DIRECT_EXTENSIONS
        .iter()
        .copied()
        .find(|ext| *ext == candidate.as_str())
}

/// Return `true` when a URL/filename ends in a known direct-download extension.
pub fn has_direct_extension(path: &str) -> bool {
    direct_extension(path).is_some()
}

/// Classified target metadata, mirroring `paradl.core.models.TargetInfo`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetInfo {
    /// The original user-supplied target string.
    pub raw: String,
    /// How the target was classified.
    pub target_type: TargetType,
    /// The normalized URL used for the transfer.
    pub clean_url: String,
    /// Optional filename hint from probing.
    pub filename_hint: Option<String>,
    /// Content-Length from probing.
    pub content_length: Option<u64>,
    /// MIME type from probing.
    pub mime_type: Option<String>,
    /// Whether the server advertises byte ranges.
    pub accept_ranges: Option<bool>,
    /// SHA-256 digest of the ETag, if present.
    pub etag_sha256: Option<String>,
    /// Expected digest supplied by the user.
    pub expected_digest: Option<String>,
    /// Extra HTTP headers.
    pub headers: HashMap<String, String>,
    /// Mirror URLs.
    pub mirrors: Vec<String>,
    /// Path to a metalink/metadata file.
    pub metalink_path: Option<PathBuf>,
    /// Path to a cookie file.
    pub cookie_file: Option<String>,
    /// Referer URL.
    pub referer: Option<String>,
    /// Override User-Agent.
    pub user_agent: Option<String>,
    /// GitHub site-specific metadata.
    pub github_meta: Option<Value>,
    /// Hugging Face site-specific metadata.
    pub hf_meta: Option<Value>,
    /// IPSW site-specific metadata.
    pub ipsw_meta: Option<Value>,
    /// PyPI site-specific metadata.
    pub pypi_meta: Option<Value>,
    /// Homebrew site-specific metadata.
    pub brew_meta: Option<Value>,
    /// Error message from a failed probe.
    pub probe_error: Option<String>,
    /// HTTP status code from probing.
    pub status_code: Option<u16>,
}

/// A single download work item, mirroring `paradl.core.models.DownloadItem`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadItem {
    /// Source URL.
    pub url: String,
    /// Destination directory.
    pub out_dir: PathBuf,
    /// Override filename.
    pub filename: Option<String>,
    /// Extra HTTP headers.
    pub headers: HashMap<String, String>,
    /// Expected byte length.
    pub expected_length: Option<u64>,
    /// Expected digest string.
    pub expected_digest: Option<String>,
    /// SHA-256 digest of the ETag.
    pub etag_sha256: Option<String>,
    /// Mirror URLs.
    pub mirrors: Vec<String>,
    /// Path to a metalink/metadata file.
    pub metalink_path: Option<PathBuf>,
    /// Path to a cookie file.
    pub cookie_file: Option<String>,
    /// Referer URL.
    pub referer: Option<String>,
    /// Override User-Agent.
    pub user_agent: Option<String>,
}

impl Default for DownloadItem {
    fn default() -> Self {
        Self {
            url: String::new(),
            out_dir: PathBuf::new(),
            filename: None,
            headers: HashMap::new(),
            expected_length: None,
            expected_digest: None,
            etag_sha256: None,
            mirrors: Vec::new(),
            metalink_path: None,
            cookie_file: None,
            referer: None,
            user_agent: None,
        }
    }
}

/// Result of a completed transfer, mirroring `paradl.core.models.TransferResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TransferResult {
    /// Whether the transfer succeeded.
    pub success: bool,
    /// Primary downloaded file.
    pub file_path: Option<PathBuf>,
    /// All files produced.
    pub files: Vec<PathBuf>,
    /// Number of bytes transferred.
    pub bytes_downloaded: u64,
    /// Transfer duration in seconds.
    pub duration: f64,
    /// Average speed in bytes per second.
    pub avg_speed_bps: f64,
    /// 10th-percentile speed in bytes per second.
    pub p10_speed_bps: f64,
    /// Number of connection recycles.
    pub recycle_count: u64,
    /// Exact audio production string from the media worker.
    pub audio_format_produced: Option<String>,
    /// Whether checksum verification passed.
    pub checksum_verified: Option<bool>,
    /// Checksum algorithm name.
    pub checksum_algo: Option<String>,
    /// Checksum digest value.
    pub checksum_digest: Option<String>,
    /// Error message on failure.
    pub error: Option<String>,
    /// Process exit code (defaults to `0`).
    pub exit_code: i32,
}

impl Default for TransferResult {
    fn default() -> Self {
        Self {
            success: false,
            file_path: None,
            files: Vec::new(),
            bytes_downloaded: 0,
            duration: 0.0,
            avg_speed_bps: 0.0,
            p10_speed_bps: 0.0,
            recycle_count: 0,
            audio_format_produced: None,
            checksum_verified: None,
            checksum_algo: None,
            checksum_digest: None,
            error: None,
            exit_code: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_multiplier_precision() {
        assert_eq!(parse_rate("1T").unwrap(), 1024.0_f64.powi(4));
        assert_eq!(parse_rate("2.5Gbps").unwrap(), 335_544_320.0);
    }

    #[test]
    fn direct_extension_comes_from_single_constant() {
        assert_eq!(direct_extension("a/b.tar.gz?sig=1"), Some(".gz"));
        assert_eq!(direct_extension("no-ext"), None);
    }

    #[test]
    fn from_str_for_enums() {
        assert_eq!("huggingface".parse::<TargetType>().unwrap(), TargetType::Hf);
        assert_eq!("v2".parse::<WatchdogMode>().unwrap(), WatchdogMode::V2);
    }
}
