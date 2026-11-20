//! REST site resolvers: GitHub, Hugging Face, Homebrew, PyPI, IPSW and cloud links.
//!
//! These are **pure REST resolvers** (charter section 5, ruling 3). Each one
//! implements the public [`TargetResolver`]
//! trait and turns a shortcut/URL into one or more [`ResolvedAsset`]s. They do
//! **not** contain yt-dlp/extractor logic, archive extraction, or subprocess
//! installation — those belong to the media worker (W3-4) and post-processing
//! (W2-2).
//!
//! Resolved digests use the Python oracle's prefixed form (`sha256:<hex>`), so
//! a downstream verifier can consume them without guessing the algorithm.
//! Secrets are supplied by a [`token::SecretProvider`] and never placed in a
//! command line: the platform keychain implementation must pass them over
//! stdin (charter row L11).
//!
//! ```
//! use std::collections::HashMap;
//! use std::sync::Arc;
//! use paradl_net::HttpClient;
//! use paradl_net::sites::{
//!     SiteRouter, accelerate::{accelerate_url_with_mirror, is_github_archive},
//!     token::{EnvSecretProvider, Keychain},
//! };
//!
//! let url = "https://github.com/python/cpython/archive/refs/heads/main.zip";
//! assert!(is_github_archive(url));
//! assert_eq!(
//!     accelerate_url_with_mirror(url, Some("https://gh-proxy.com/")).as_deref(),
//!     Some("https://gh-proxy.com/https://github.com/python/cpython/archive/refs/heads/main.zip"),
//! );
//!
//! let client = HttpClient::new(Default::default()).unwrap();
//! let secrets = Arc::new(EnvSecretProvider::with_env(
//!     HashMap::new(), None::<Arc<dyn Keychain>>,
//! ));
//! let router = SiteRouter::new(client, secrets);
//! assert_eq!(router.kind_of("gh:owner/repo"), Some("github"));
//! assert_eq!(router.kind_of("hf:owner/repo"), Some("huggingface"));
//! ```

pub mod accelerate;
pub mod brew;
pub mod cloud;
pub mod github;
pub mod hf;
pub mod ipsw;
pub mod pypi;
pub mod token;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use paradl_core::TargetType;
use reqwest::{Method, Response};
use serde_json::Value;
use tracing::debug;

use crate::client::{HttpClient, request_with_retry};
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::{Error, Result};

/// A decoded REST JSON response, mirroring Python's
/// `(data, headers, status)` tuple from `fetch_json_with_etag`.
#[derive(Debug, Clone)]
pub struct JsonResponse {
    /// HTTP status code returned by the server.
    pub status: u16,
    /// Lower-cased response headers.
    pub headers: HashMap<String, String>,
    /// Parsed JSON body (`Value::Null` on an empty/non-JSON body).
    pub body: Value,
}

/// The result of following a non-redirecting HTTP request (GHCR blob flow).
#[derive(Debug, Clone)]
pub struct RedirectInfo {
    /// Final URL after the 3xx redirect (or the original blob URL on 200/206).
    pub url: String,
    /// Headers that must accompany a request to `url` (empty for a signed CDN URL).
    pub headers: HashMap<String, String>,
    /// Advertised content length, when known.
    pub content_length: Option<u64>,
}

/// Pluggable HTTP seam for site resolvers.
///
/// The production implementation is [`HttpSiteHttp`], which reuses the shared
/// [`HttpClient`] (retries, proxy, IP family, DoH). Tests inject a scripted
/// implementation — this is the Rust equivalent of the Python oracle
/// monkeypatching `fetch_json_with_etag`.
#[async_trait]
pub trait SiteHttp: Send + Sync {
    /// Fetch and decode JSON from `url` with `headers` and `timeout`.
    async fn fetch_json(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<JsonResponse>;

    /// Issue a non-redirecting GET and report a 3xx target, or a 200/206 blob
    /// response. Returns `None` when the request did not produce a usable URL.
    async fn redirect(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Option<RedirectInfo>>;
}

fn lower_headers(response: &Response) -> HashMap<String, String> {
    response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

/// Production [`SiteHttp`] backed by the shared reqwest [`HttpClient`].
#[derive(Clone)]
pub struct HttpSiteHttp {
    client: HttpClient,
}

impl HttpSiteHttp {
    /// Build an HTTP seam from a shared client.
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl SiteHttp for HttpSiteHttp {
    async fn fetch_json(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<JsonResponse> {
        let response = request_with_retry(
            &self.client,
            Method::GET,
            url,
            headers,
            self.client.config().retries,
            timeout,
            true,
        )
        .await?;
        let status = response.status().as_u16();
        let response_headers = lower_headers(&response);
        let body = match response.bytes().await {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            Err(error) => {
                debug!("Failed to read JSON body from {url}: {error}");
                Value::Null
            }
        };
        Ok(JsonResponse {
            status,
            headers: response_headers,
            body,
        })
    }

    async fn redirect(
        &self,
        url: &str,
        headers: &HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Option<RedirectInfo>> {
        let response = request_with_retry(
            &self.client,
            Method::GET,
            url,
            headers,
            self.client.config().retries,
            timeout,
            false,
        )
        .await?;
        let status = response.status().as_u16();
        let all = lower_headers(&response);
        if matches!(status, 301 | 302 | 307 | 308) {
            let Some(location) = all.get("location").cloned() else {
                return Ok(None);
            };
            // A signed CDN URL needs no Authorization header. Its length is
            // best-effort (a failed HEAD must not fail resolution).
            let content_length = match request_with_retry(
                &self.client,
                Method::HEAD,
                &location,
                &HashMap::new(),
                1,
                timeout,
                true,
            )
            .await
            {
                Ok(head) => lower_headers(&head)
                    .get("content-length")
                    .and_then(|value| value.parse::<u64>().ok()),
                Err(error) => {
                    debug!("GHCR HEAD probe failed for {location}: {error}");
                    None
                }
            };
            return Ok(Some(RedirectInfo {
                url: location,
                headers: HashMap::new(),
                content_length,
            }));
        }
        if matches!(status, 200 | 206) {
            let content_length = all
                .get("content-length")
                .and_then(|value| value.parse::<u64>().ok());
            return Ok(Some(RedirectInfo {
                url: url.to_owned(),
                headers: headers.clone(),
                content_length,
            }));
        }
        Ok(None)
    }
}

/// Dispatch a raw target to the matching site engine.
///
/// This is the W2-3 entry point that W4-1 wires ahead of
/// [`crate::routing::resolve_target`] for shortcut prefixes (parity row C51).
#[derive(Clone)]
pub struct SiteRouter {
    secrets: Arc<dyn token::SecretProvider>,
    http: Arc<dyn SiteHttp>,
}

impl SiteRouter {
    /// Build a router with production HTTP I/O.
    pub fn new(client: HttpClient, secrets: Arc<dyn token::SecretProvider>) -> Self {
        let http = Arc::new(HttpSiteHttp::new(client));
        Self { secrets, http }
    }

    /// Build a router that shares an existing HTTP seam (used by tests and by
    /// callers that wrap `SiteHttp` with a cache/metrics layer).
    pub fn with_http(secrets: Arc<dyn token::SecretProvider>, http: Arc<dyn SiteHttp>) -> Self {
        Self { secrets, http }
    }

    /// Return the site key for `target`, or `None` when no site engine claims it.
    ///
    /// Keys are the `TargetType::as_str()` values: `github`, `huggingface`,
    /// `pypi`, `brew`, `ipsw`, plus `cloud` for pure cloud-link rewrites.
    pub fn kind_of(&self, target: &str) -> Option<&'static str> {
        let lower = target.trim().to_ascii_lowercase();
        if cloud::is_cloud_url(&lower) {
            return Some("cloud");
        }
        if lower.starts_with("gh:") || lower.starts_with("github:") || lower.contains("github.com") {
            return Some(TargetType::Github.as_str());
        }
        if lower.starts_with("hf:")
            || lower.starts_with("huggingface:")
            || lower.contains("huggingface.co")
            || lower.contains("hf.co/")
        {
            return Some(TargetType::Hf.as_str());
        }
        if lower.starts_with("pip:")
            || lower.starts_with("pypi:")
            || lower.contains("pypi.org/")
        {
            return Some(TargetType::Pypi.as_str());
        }
        if lower.starts_with("brew:")
            || lower.starts_with("homebrew:")
            || lower.contains("formulae.brew.sh/")
        {
            return Some(TargetType::Brew.as_str());
        }
        if lower.starts_with("ipsw:") || lower.starts_with("ipsw/") || ipsw::is_ipsw(&lower) {
            return Some(TargetType::IpSw.as_str());
        }
        None
    }

    /// Resolve `target` with the engine selected by [`SiteRouter::kind_of`].
    pub async fn resolve(
        &self,
        target: &str,
        ctx: &ResolveContext,
    ) -> Result<Vec<ResolvedAsset>> {
        match self.kind_of(target) {
            Some("cloud") => cloud::CloudResolver::new().resolve(target, ctx).await,
            Some("github") => {
                github::GithubResolver::from_shared(self.http.clone(), self.secrets.clone())
                    .resolve(target, ctx)
                    .await
            }
            Some("huggingface") => {
                hf::HfResolver::from_shared(self.http.clone(), self.secrets.clone())
                    .resolve(target, ctx)
                    .await
            }
            Some("pypi") => {
                pypi::PyPIResolver::from_shared(self.http.clone(), self.secrets.clone())
                    .resolve(target, ctx)
                    .await
            }
            Some("brew") => {
                brew::BrewResolver::from_shared(self.http.clone())
                    .resolve(target, ctx)
                    .await
            }
            Some("ipsw") => {
                ipsw::IpswResolver::from_shared(self.http.clone(), self.secrets.clone())
                    .resolve(target, ctx)
                    .await
            }
            _ => Err(Error::usage(format!("No site resolver handles {target:?}"))),
        }
    }
}

pub use accelerate::{accelerate_url, accelerate_url_with_mirror, is_github_archive};

/// Match a filename against a glob pattern, substring, or (fallback) regex.
///
/// Mirrors `GitHubEngine._matches_pattern` and `hf._matches_asset_pattern`:
/// `fnmatch` case-insensitive first, then a substring test, then a regex.
pub(crate) fn matches_pattern(name: &str, pattern: &str) -> bool {
    let lower_name = name.to_ascii_lowercase();
    let lower_pattern = pattern.to_ascii_lowercase();
    if glob_match(&lower_name, &lower_pattern) || lower_pattern.len() <= lower_name.len()
        && lower_name.contains(&lower_pattern)
    {
        return true;
    }
    regex::Regex::new(pattern).is_ok_and(|re| re.is_match(name))
}

fn glob_match(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let mut previous = vec![false; name.len() + 1];
    previous[0] = true;
    for ch in &pattern {
        let mut current = vec![false; name.len() + 1];
        if *ch == '*' {
            current[0] = previous[0];
            for i in 1..=name.len() {
                current[i] = previous[i] || current[i - 1];
            }
        } else {
            for i in 1..=name.len() {
                current[i] = previous[i - 1] && (*ch == '?' || *ch == name[i - 1]);
            }
        }
        previous = current;
    }
    previous[name.len()]
}
