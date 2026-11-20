//! Pure routing rules and the public [`TargetResolver`] trait.
//!
//! The pure rules mirror `src/paradl/net/client.py::probe_target` but contain
//! **no extractor logic**: domain lists, extension suffixes, `ytsearch1:`, and
//! HEAD/Range MIME heuristics are evaluated against [`paradl_core`] constants.
//! Anything that needs yt-dlp's live extractor registry is returned as a
//! [`RouteDecision::DelegateToWorker`] for the Python media worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use paradl_core::{TargetType, sanitize_filename};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::client::{HttpClient, HttpProbeResult, probe_http_url};
use crate::dns::IpFamily;
use crate::{Error, Result};

/// Why a target was handed to the Python media worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DelegateReason {
    /// Hostname matched `MEDIA_DOMAINS`.
    MediaDomain,
    /// Probe reported `text/html` (parity row L31).
    HtmlContentType,
    /// An injected extractor-claims checker matched (live registry delegated).
    ExtractorClaim,
    /// No direct-download signal was found; media worker gets the benefit of
    /// the doubt (Python's default `STREAM` fallthrough).
    DefaultStream,
    /// The target is a free-text or `ytsearch1:` search query.
    SearchQuery,
}

/// The result of `ytdlp_extractor_claims_url` (ruling 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaClaim {
    /// Delegate to the media worker's `claims`/`resolve` op.
    DelegateToWorker,
    /// `claims=false`: fall back to a direct Rust download.
    DirectFallback,
}

/// A check for whether a URL is claimed by a yt-dlp extractor.
///
/// Rust never loads yt-dlp. The default [`PureClaimsChecker`] uses only the
/// `MEDIA_DOMAINS` list; a W3-4 media bridge supplies an implementation that
/// calls the worker's `claims` op against the live extractor registry.
pub trait ExtractorClaims: Send + Sync {
    /// Return `true` when an extractor claims `url`.
    fn claims(&self, url: &str) -> bool;
}

/// Domain-list-only [`ExtractorClaims`] implementation (no Python, no yt-dlp).
#[derive(Debug, Default, Clone, Copy)]
pub struct PureClaimsChecker;

impl ExtractorClaims for PureClaimsChecker {
    fn claims(&self, url: &str) -> bool {
        paradl_core::is_media_domain(url)
    }
}

/// Decide whether a URL needs the media worker's live extractor registry.
///
/// Known media domains are delegated unconditionally; everything else is
/// offered to `claims`, and `claims=false` becomes [`MediaClaim::DirectFallback`].
pub fn ytdlp_extractor_claims(url: &str, checker: &dyn ExtractorClaims) -> MediaClaim {
    if paradl_core::is_media_domain(url) || checker.claims(url) {
        MediaClaim::DelegateToWorker
    } else {
        MediaClaim::DirectFallback
    }
}

/// The classification of a raw target string.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// A direct download handled entirely in Rust.
    Direct {
        /// Always [`TargetType::Direct`] (or a probe-error fallback).
        target_type: TargetType,
        /// Normalized URL used for the transfer.
        clean_url: String,
        /// Best-effort filename from URL or probe.
        filename_hint: Option<String>,
        /// Content length from the probe.
        content_length: Option<u64>,
        /// MIME type from the probe.
        mime_type: Option<String>,
        /// Whether byte ranges are supported.
        accept_ranges: Option<bool>,
        /// SHA-256 digest extracted from ETag-like headers.
        etag_sha256: Option<String>,
        /// Error text for a failed HTTP probe (halts the transfer).
        probe_error: Option<String>,
        /// HTTP status code from a failed probe.
        status_code: Option<u16>,
    },
    /// Hand the target to the Python media worker (`paradl-media/1`).
    DelegateToWorker {
        /// `Stream` or `Search`.
        target_type: TargetType,
        /// URL or `ytsearch1:` query passed to the worker.
        clean_url: String,
        /// Why the worker was chosen.
        reason: DelegateReason,
    },
    /// A site-specific resolver target (`gh:`, `hf:`, `pip:`, `brew:`, `ipsw:`).
    /// REST resolution is owned by W2-3 site engines.
    Site {
        /// The site-specific target type.
        target_type: TargetType,
        /// The original shortcut/URL.
        clean_url: String,
    },
    /// A magnet/torrent target.
    Torrent {
        /// The magnet URI or `.torrent` URL.
        clean_url: String,
    },
}

fn delegate(target_type: TargetType, clean_url: &str, reason: DelegateReason) -> RouteDecision {
    RouteDecision::DelegateToWorker {
        target_type,
        clean_url: clean_url.to_owned(),
        reason,
    }
}

fn url_basename(url: &str) -> String {
    let without_params = url.split(['?', '#']).next().unwrap_or(url);
    without_params.rsplit('/').next().unwrap_or(without_params).to_owned()
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn status_description(status: u16) -> &'static str {
    match status {
        401 => "Unauthorized (Authentication required)",
        402 => "Payment Required (Upstream credits/quota exhausted or payment required)",
        403 => "Forbidden (Access denied by server)",
        404 => "Not Found (Resource does not exist)",
        410 => "Gone (Resource has been permanently removed)",
        429 => "Too Many Requests (Rate limit exceeded)",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown HTTP Error",
    }
}

fn probe_error_message(probe: &HttpProbeResult, target: &str) -> Option<String> {
    if probe.ok || probe.status_code < 400 {
        return None;
    }
    Some(format!(
        "HTTP Error {}: {} for {}",
        probe.status_code,
        status_description(probe.status_code),
        target
    ))
}

fn is_probably_direct(probe: &HttpProbeResult) -> bool {
    let content_type = &probe.content_type;
    ["application/", "video/", "audio/", "image/", "octet-stream"]
        .iter()
        .any(|marker| content_type.contains(marker))
        || (!content_type.contains("text/html") && probe.content_length.is_some_and(|n| n > 1024))
        || (probe.accept_ranges && !content_type.contains("text/html"))
}

fn image_filename(url: &str) -> String {
    let raw = url_basename(url);
    let mut name = truncate_chars(&sanitize_filename(&raw), 160);
    if name.is_empty() {
        name = "image".to_owned();
    }
    let lower = name.to_ascii_lowercase();
    if ![".jpg", ".jpeg", ".png", ".webp", ".gif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        name.push_str(".jpg");
    }
    name
}

fn classify_image(url: &str, probe: Option<&HttpProbeResult>, fallback_name: String) -> RouteDecision {
    if let Some(probe) = probe {
        if probe.ok {
            let final_name = probe
                .filename
                .clone()
                .unwrap_or(fallback_name);
            return RouteDecision::Direct {
                target_type: TargetType::Direct,
                clean_url: probe.final_url.clone(),
                filename_hint: Some(truncate_chars(&sanitize_filename(&final_name), 180)),
                content_length: probe.content_length,
                mime_type: Some(
                    if probe.content_type.is_empty() {
                        "image/jpeg".to_owned()
                    } else {
                        probe.content_type.clone()
                    },
                ),
                accept_ranges: Some(probe.accept_ranges),
                etag_sha256: probe.etag_sha256.clone(),
                probe_error: None,
                status_code: None,
            };
        }
        if let Some(error) = probe_error_message(probe, url) {
            return RouteDecision::Direct {
                target_type: TargetType::Direct,
                clean_url: url.to_owned(),
                filename_hint: Some(fallback_name),
                content_length: None,
                mime_type: Some("image/jpeg".to_owned()),
                accept_ranges: None,
                etag_sha256: None,
                probe_error: Some(error),
                status_code: Some(probe.status_code),
            };
        }
    }
    RouteDecision::Direct {
        target_type: TargetType::Direct,
        clean_url: url.to_owned(),
        filename_hint: Some(fallback_name),
        content_length: None,
        mime_type: Some("image/jpeg".to_owned()),
        accept_ranges: None,
        etag_sha256: None,
        probe_error: None,
        status_code: None,
    }
}

fn shortcut_type(lower: &str) -> Option<TargetType> {
    if lower.starts_with("gh:") || lower.starts_with("github:") || lower.contains("github.com/") {
        Some(TargetType::Github)
    } else if lower.starts_with("hf:")
        || lower.starts_with("huggingface:")
        || lower.contains("huggingface.co/")
        || lower.contains("hf.co/")
    {
        Some(TargetType::Hf)
    } else if lower.starts_with("ipsw:") {
        Some(TargetType::IpSw)
    } else if lower.starts_with("pip:")
        || lower.starts_with("pypi:")
        || lower.contains("pypi.org/")
    {
        Some(TargetType::Pypi)
    } else if lower.starts_with("brew:")
        || lower.starts_with("homebrew:")
        || lower.contains("formulae.brew.sh/")
    {
        Some(TargetType::Brew)
    } else {
        None
    }
}

/// Classify a raw target using only pure rules and an optional HTTP probe.
///
/// This function performs no network I/O. `probe` should be supplied by the
/// caller via [`probe_http_url`] when network probing is allowed. Domain and
/// extension checks read the single-source constants re-exported from
/// `paradl-core`.
pub fn classify_target(
    target: &str,
    probe: Option<&HttpProbeResult>,
    claims: &dyn ExtractorClaims,
) -> RouteDecision {
    let target = target.trim();
    let lower = target.to_ascii_lowercase();

    // 1. Magnet / torrent.
    if lower.starts_with("magnet:?") || lower.ends_with(".torrent") {
        return RouteDecision::Torrent {
            clean_url: target.to_owned(),
        };
    }

    // 2. YouTube search prefix (idempotent).
    if lower.starts_with("ytsearch1:") {
        return delegate(TargetType::Search, target, DelegateReason::SearchQuery);
    }

    // 3. Site shortcuts — REST resolution is owned by W2-3 site engines.
    if let Some(target_type) = shortcut_type(&lower) {
        return RouteDecision::Site {
            target_type,
            clean_url: target.to_owned(),
        };
    }

    // 4. Local metalink/metadata files.
    if (lower.ends_with(".meta4") || lower.ends_with(".metalink"))
        && !lower.starts_with("http://")
        && !lower.starts_with("https://")
        && !lower.starts_with("ftp://")
    {
        return RouteDecision::Direct {
            target_type: TargetType::Direct,
            clean_url: target.to_owned(),
            filename_hint: Some(url_basename(target)),
            content_length: None,
            mime_type: None,
            accept_ranges: None,
            etag_sha256: None,
            probe_error: None,
            status_code: None,
        };
    }

    // 5. Generic network URLs.
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("ftp://") {
        // 5a. Streaming-media domains win over extensions.
        if paradl_core::is_media_domain(target) {
            return delegate(TargetType::Stream, target, DelegateReason::MediaDomain);
        }
        // 5b. Known direct-download extensions.
        if paradl_core::has_direct_extension(target) {
            return RouteDecision::Direct {
                target_type: TargetType::Direct,
                clean_url: target.to_owned(),
                filename_hint: Some(url_basename(target)),
                content_length: None,
                mime_type: None,
                accept_ranges: None,
                etag_sha256: None,
                probe_error: None,
                status_code: None,
            };
        }
        // 5c. Dedicated image services.
        if paradl_core::is_image_domain(target) {
            return classify_image(target, probe, image_filename(target));
        }
        // 5d. MIME/Range heuristics for HTTP(S) only.
        if let Some(probe) = probe {
            if let Some(error) = probe_error_message(probe, target) {
                return RouteDecision::Direct {
                    target_type: TargetType::Direct,
                    clean_url: target.to_owned(),
                    filename_hint: None,
                    content_length: None,
                    mime_type: None,
                    accept_ranges: None,
                    etag_sha256: None,
                    probe_error: Some(error),
                    status_code: Some(probe.status_code),
                };
            }
            if probe.ok {
                if is_probably_direct(probe) {
                    let filename = probe
                        .filename
                        .clone()
                        .or_else(|| {
                            let base = url_basename(&probe.final_url);
                            (!base.is_empty()).then_some(base)
                        })
                        .unwrap_or_else(|| "downloaded_file".to_owned());
                    return RouteDecision::Direct {
                        target_type: TargetType::Direct,
                        clean_url: probe.final_url.clone(),
                        filename_hint: Some(truncate_chars(&sanitize_filename(&filename), 180)),
                        content_length: probe.content_length,
                        mime_type: Some(probe.content_type.clone()),
                        accept_ranges: Some(probe.accept_ranges),
                        etag_sha256: probe.etag_sha256.clone(),
                        probe_error: None,
                        status_code: None,
                    };
                }
                if probe.content_type.contains("text/html") {
                    return delegate(TargetType::Stream, target, DelegateReason::HtmlContentType);
                }
                if claims.claims(target) {
                    return delegate(TargetType::Stream, target, DelegateReason::ExtractorClaim);
                }
            }
        }
        // No probe or no direct signal: Python defaults to STREAM.
        return delegate(TargetType::Stream, target, DelegateReason::DefaultStream);
    }

    // 6. Free text -> `ytsearch1:` search.
    delegate(
        TargetType::Search,
        &format!("ytsearch1:{target}"),
        DelegateReason::SearchQuery,
    )
}

/// Which media-worker operation an asset needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaOp {
    /// Fetch full metadata / download a stream or search result.
    Resolve,
    /// Download a site-managed (yt-dlp) transfer.
    DownloadManaged,
    /// Ask the worker whether its live extractor registry claims the URL.
    Claims,
}

impl MediaOp {
    /// Return the `paradl-media/1` operation string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::DownloadManaged => "download_managed",
            Self::Claims => "claims",
        }
    }
}

/// A single resolved download asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolvedAsset {
    /// The URL (or `ytsearch1:` query) to fetch.
    pub url: String,
    /// Best-effort output filename.
    pub filename: Option<String>,
    /// Expected byte length.
    pub content_length: Option<u64>,
    /// Trusted SHA-256 digest, if any.
    pub digest: Option<String>,
    /// How the asset was classified.
    pub target_type: TargetType,
    /// Extra HTTP headers (includes probed `content-type` / `accept-ranges`).
    pub headers: HashMap<String, String>,
    /// `Some` when the Python media worker must handle this asset.
    pub media_op: Option<MediaOp>,
}

impl Default for ResolvedAsset {
    fn default() -> Self {
        Self {
            url: String::new(),
            filename: None,
            content_length: None,
            digest: None,
            target_type: TargetType::Direct,
            headers: HashMap::new(),
            media_op: None,
        }
    }
}

/// Context passed to [`TargetResolver::resolve`].
#[derive(Debug, Clone)]
pub struct ResolveContext {
    /// Extra HTTP headers (cookies, referer, authorization).
    pub headers: HashMap<String, String>,
    /// Whether HTTP HEAD/Range probing is allowed.
    pub allow_network_probe: bool,
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Preferred IP family.
    pub ip_family: IpFamily,
    /// Optional DNS-over-HTTPS endpoint.
    pub doh_endpoint: Option<String>,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Python version tuple used by PyPI wheel scoring (W2-3).
    pub py_version: Option<(u32, u32)>,
}

impl Default for ResolveContext {
    fn default() -> Self {
        Self {
            headers: HashMap::new(),
            allow_network_probe: true,
            proxy: None,
            ip_family: IpFamily::Auto,
            doh_endpoint: None,
            timeout: Duration::from_secs_f64(4.0),
            py_version: None,
        }
    }
}

/// Resolve a raw target into one or more downloadable assets.
///
/// Implementors are keyed by URL scheme (see [`HttpResolver`],
/// [`SearchResolver`], [`MagnetResolver`]).
#[async_trait]
pub trait TargetResolver: Send + Sync {
    /// The URI scheme this resolver handles (e.g. `"https"`).
    fn scheme(&self) -> &'static str;

    /// Resolve `target` into downloadable assets using `ctx`.
    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>>;
}

fn decision_to_assets(decision: RouteDecision) -> Result<Vec<ResolvedAsset>> {
    let asset = match decision {
        RouteDecision::Direct {
            target_type,
            clean_url,
            filename_hint,
            content_length,
            mime_type,
            accept_ranges,
            etag_sha256,
            probe_error,
            ..
        } => {
            // Python halts early on probe errors (never falls back to media).
            if let Some(error) = probe_error {
                return Err(Error::failure(error));
            }
            let mut headers = HashMap::new();
            if let Some(mime) = mime_type {
                headers.insert("content-type".to_owned(), mime);
            }
            if let Some(ranges) = accept_ranges {
                headers.insert(
                    "accept-ranges".to_owned(),
                    if ranges { "bytes" } else { "none" }.to_owned(),
                );
            }
            ResolvedAsset {
                url: clean_url,
                filename: filename_hint,
                content_length,
                digest: etag_sha256,
                target_type,
                headers,
                media_op: None,
            }
        }
        RouteDecision::DelegateToWorker {
            target_type,
            clean_url,
            reason,
        } => {
            debug!("delegating {clean_url} to media worker ({reason:?})");
            ResolvedAsset {
                url: clean_url,
                filename: None,
                content_length: None,
                digest: None,
                target_type,
                headers: HashMap::new(),
                media_op: Some(MediaOp::Resolve),
            }
        }
        RouteDecision::Site {
            target_type,
            clean_url,
        } => ResolvedAsset {
            url: clean_url,
            target_type,
            media_op: None,
            ..Default::default()
        },
        RouteDecision::Torrent { clean_url } => ResolvedAsset {
            url: clean_url,
            target_type: TargetType::Torrent,
            media_op: None,
            ..Default::default()
        },
    };
    Ok(vec![asset])
}

/// [`TargetResolver`] for `http://` and `https://` targets.
pub struct HttpResolver {
    client: HttpClient,
    scheme: &'static str,
    claims: Arc<dyn ExtractorClaims>,
}

impl HttpResolver {
    /// Build an HTTPS resolver with the domain-list-only claims checker.
    pub fn new(client: HttpClient) -> Self {
        Self::with_scheme(client, "https")
    }

    /// Build a resolver for the given scheme (`"http"` or `"https"`).
    pub fn with_scheme(client: HttpClient, scheme: &'static str) -> Self {
        Self {
            client,
            scheme,
            claims: Arc::new(PureClaimsChecker),
        }
    }

    /// Override the extractor-claims checker (used by the W3-4 media bridge).
    pub fn with_claims(mut self, claims: Arc<dyn ExtractorClaims>) -> Self {
        self.claims = claims;
        self
    }
}

#[async_trait]
impl TargetResolver for HttpResolver {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let target = target.trim();
        let is_http = {
            let lower = target.to_ascii_lowercase();
            lower.starts_with("http://") || lower.starts_with("https://")
        };
        let probe = if ctx.allow_network_probe && is_http {
            match probe_http_url(&self.client, target, &ctx.headers, ctx.timeout).await {
                Ok(result) => Some(result),
                Err(error) => {
                    debug!("probe failed for {target}: {error}");
                    None
                }
            }
        } else {
            None
        };
        decision_to_assets(classify_target(target, probe.as_ref(), self.claims.as_ref()))
    }
}

/// [`TargetResolver`] for `ytsearch1:` free-text search targets.
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchResolver;

impl SearchResolver {
    /// Create a search resolver.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TargetResolver for SearchResolver {
    fn scheme(&self) -> &'static str {
        "ytsearch1"
    }

    async fn resolve(&self, target: &str, _ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let query = target.trim();
        let url = if query.to_ascii_lowercase().starts_with("ytsearch1:") {
            query.to_owned()
        } else {
            format!("ytsearch1:{query}")
        };
        decision_to_assets(RouteDecision::DelegateToWorker {
            target_type: TargetType::Search,
            clean_url: url,
            reason: DelegateReason::SearchQuery,
        })
    }
}

/// [`TargetResolver`] for `magnet:` torrent targets.
#[derive(Debug, Default, Clone, Copy)]
pub struct MagnetResolver;

impl MagnetResolver {
    /// Create a magnet resolver.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TargetResolver for MagnetResolver {
    fn scheme(&self) -> &'static str {
        "magnet"
    }

    async fn resolve(&self, target: &str, _ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        decision_to_assets(RouteDecision::Torrent {
            clean_url: target.trim().to_owned(),
        })
    }
}

/// Convenience dispatcher that selects a resolver from `target`'s scheme and
/// runs it. Free text is routed to [`SearchResolver`].
pub async fn resolve_target(
    target: &str,
    ctx: &ResolveContext,
    client: &HttpClient,
    claims: Arc<dyn ExtractorClaims>,
) -> Result<Vec<ResolvedAsset>> {
    let trimmed = target.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        return MagnetResolver::new().resolve(trimmed, ctx).await;
    }
    if lower.starts_with("ytsearch1:") {
        return SearchResolver::new().resolve(trimmed, ctx).await;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let scheme = if lower.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        return HttpResolver::with_scheme(client.clone(), scheme)
            .with_claims(claims)
            .resolve(trimmed, ctx)
            .await;
    }
    decision_to_assets(classify_target(trimmed, None, claims.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_op_strings_match_protocol() {
        assert_eq!(MediaOp::Resolve.as_str(), "resolve");
        assert_eq!(MediaOp::DownloadManaged.as_str(), "download_managed");
        assert_eq!(MediaOp::Claims.as_str(), "claims");
    }

    #[test]
    fn probe_error_status_descriptions() {
        assert!(status_description(402).contains("Payment Required"));
        assert!(status_description(404).contains("Not Found"));
    }
}
