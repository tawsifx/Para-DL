//! reqwest/rustls HTTP client, HEAD/Range prober, RFC 6266 filename parser,
//! ETag digest extraction, and jittered retry.
//!
//! The pure helpers ([`parse_content_disposition_filename`],
//! [`extract_etag_sha256`], [`extract_verified_sha256`], [`sha256_hex_of`])
//! mirror `src/paradl/net/client.py` byte-for-byte and are network-free.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::LazyLock;
use std::time::Duration;

use paradl_core::sanitize_filename;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Response};
use sha2::{Digest, Sha256};
use tracing::debug;
use url::Url;

use crate::dns::{IpFamily, resolve_dns};
use crate::{Error, Result};

/// Default User-Agent used by the shared client (`pdl-accelerator/{VERSION}`).
pub const DEFAULT_USER_AGENT: &str = "pdl-accelerator/0.1.0";
/// Browser-like User-Agent used by the HEAD/Range probers.
pub const PROBE_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

static FILENAME_STAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)filename\*\s*=\s*([A-Za-z0-9_\-]+)?'[^']*'(?:"([^"]+)"|([^;\s]+))"#,
    )
    .expect("filename* regex must compile")
});
static FILENAME_PLAIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)filename\s*=\s*(?:"([^"]+)"|'([^']+)'|([^;\s]+))"#)
        .expect("filename regex must compile")
});
static CONTENT_RANGE_TOTAL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)bytes\s+(?:[0-9]+-[0-9]+|\*)/([0-9]+)")
        .expect("content-range regex must compile")
});

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Find the first standalone 64-character hexadecimal SHA-256 digest in `text`.
///
/// Mirrors the Python regex `\b([0-9a-fA-F]{64})\b`: the 64-hex run must be
/// bounded by non-word characters. Returns the digest lower-cased.
pub fn find_sha256_hex(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 64 {
        return None;
    }
    for start in 0..=bytes.len() - 64 {
        let window = &bytes[start..start + 64];
        if !window.iter().all(|c| is_hex(*c)) {
            continue;
        }
        if start > 0 && is_word(bytes[start - 1]) {
            continue;
        }
        if start + 64 < bytes.len() && is_word(bytes[start + 64]) {
            continue;
        }
        return Some(
            std::str::from_utf8(window)
                .expect("hex window is ASCII")
                .to_ascii_lowercase(),
        );
    }
    None
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode a string into UTF-8 (RFC 5987 / `urllib.parse.unquote`).
///
/// The `charset` argument is accepted for parity with the Python oracle; UTF-8
/// is the only charset required by the captured goldens. Unknown bytes are
/// decoded lossily rather than failing the whole filename.
fn percent_decode(input: &str, _charset: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

/// Extract a filename from a `Content-Disposition` header with RFC 6266
/// `filename*` support. `filename*` takes priority over plain `filename`.
///
/// Query strings, path traversal, and Windows reserved names are neutralized by
/// delegating to `paradl_core::sanitize_filename`.
///
/// ```
/// use paradl_net::parse_content_disposition_filename;
/// assert_eq!(
///     parse_content_disposition_filename(Some("attachment; filename=\"a.pdf\"")).as_deref(),
///     Some("a.pdf"),
/// );
/// ```
pub fn parse_content_disposition_filename(header_val: Option<&str>) -> Option<String> {
    let header = header_val?;
    if header.is_empty() {
        return None;
    }

    if let Some(caps) = FILENAME_STAR_RE.captures(header) {
        let charset = caps
            .get(1)
            .map(|m| m.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("utf-8");
        let raw = caps
            .get(2)
            .or_else(|| caps.get(3))
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim();
        if !raw.is_empty() {
            let decoded = percent_decode(raw, charset);
            let base = basename(decoded.trim());
            if !base.is_empty() {
                return Some(sanitize_filename(base));
            }
        }
    }

    if let Some(caps) = FILENAME_PLAIN_RE.captures(header) {
        let raw = caps
            .get(1)
            .or_else(|| caps.get(2))
            .or_else(|| caps.get(3))
            .map(|m| m.as_str())
            .unwrap_or("")
            .trim();
        if !raw.is_empty() {
            let decoded = percent_decode(raw, "utf-8");
            let base = basename(decoded.trim());
            if !base.is_empty() {
                return Some(sanitize_filename(base));
            }
        }
    }
    None
}

/// Extract a 64-hex SHA-256 digest from Hugging Face `X-Linked-ETag`, `ETag`,
/// or `X-Checksum-SHA256` response headers. Header names are lower-cased.
///
/// ```
/// use std::collections::HashMap;
/// use paradl_net::extract_etag_sha256;
/// let mut h = HashMap::new();
/// h.insert("etag".into(), "\"sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789\"".into());
/// assert!(extract_etag_sha256(&h).is_some());
/// ```
pub fn extract_etag_sha256(headers: &HashMap<String, String>) -> Option<String> {
    for name in ["x-linked-etag", "etag", "x-checksum-sha256"] {
        if let Some(value) = headers.get(name)
            && let Some(found) = find_sha256_hex(value)
        {
            return Some(found);
        }
    }
    None
}

fn is_trusted_etag_sha256_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "huggingface.co"
        || host.ends_with(".huggingface.co")
        || host == "hf.co"
        || host.ends_with(".hf.co")
}

/// Extract a SHA-256 digest suitable for automatic integrity verification.
///
/// Explicit checksum headers (`x-linked-etag`, `x-checksum-sha256`) are always
/// trusted. A generic `etag` is trusted only when strong (not `W/...`) and
/// either prefixed with `sha256:` or served by a host whose ETags are file
/// SHA-256s (e.g. Hugging Face). Generic 64-hex ETags (GitHub codeload cache
/// keys) are ignored to avoid false verification failures.
pub fn extract_verified_sha256(
    headers: &HashMap<String, String>,
    url: &str,
) -> Option<String> {
    for name in ["x-linked-etag", "x-checksum-sha256"] {
        if let Some(value) = headers.get(name)
            && let Some(found) = find_sha256_hex(value)
        {
            return Some(found);
        }
    }
    if let Some(value) = headers.get("etag") {
        let raw = value.trim();
        if !raw.starts_with("W/")
            && (raw.to_ascii_lowercase().contains("sha256:")
                || is_trusted_etag_sha256_url(url))
        {
            return find_sha256_hex(raw);
        }
    }
    None
}

/// Compute the lowercase hex SHA-256 of an arbitrary string (ETag digest).
///
/// Useful for deriving a stable cache digest from a strong ETag.
///
/// ```
/// use paradl_net::client::sha256_hex_of;
/// assert_eq!(
///     sha256_hex_of("abc"),
///     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
/// );
/// ```
pub fn sha256_hex_of(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The result of an HTTP HEAD or Range probe, mirroring Python's
/// `HttpProbeResult` dataclass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpProbeResult {
    /// Whether the probe returned a usable 2xx response.
    pub ok: bool,
    /// The final URL after redirects.
    pub final_url: String,
    /// HTTP status code (`0` when no response was received).
    pub status_code: u16,
    /// Lower-cased `Content-Type` value.
    pub content_type: String,
    /// Total content length, when advertised or derivable from `Content-Range`.
    pub content_length: Option<u64>,
    /// Filename parsed from `Content-Disposition`, when present.
    pub filename: Option<String>,
    /// Whether the server confirmed byte-range support (HTTP 206 for Range GET,
    /// or `Accept-Ranges: bytes` on HEAD).
    pub accept_ranges: bool,
    /// A 64-hex digest extracted from ETag-like headers.
    pub etag_sha256: Option<String>,
    /// A SHA-256 digest trusted for automatic verification.
    pub verified_sha256: Option<String>,
    /// All response headers, lower-cased.
    pub headers: HashMap<String, String>,
}

/// Configuration for [`HttpClient`].
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Optional HTTP(S) proxy URL (e.g. `http://proxy.local:8080`).
    pub proxy: Option<String>,
    /// Preferred IP family for outbound connections.
    pub ip_family: IpFamily,
    /// Per-request timeout.
    pub timeout: Duration,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// User-Agent sent on every request.
    pub user_agent: String,
    /// Maximum number of retry attempts for transient failures.
    pub retries: u32,
    /// Optional DNS-over-HTTPS endpoint (`--doh-endpoint`), used when the
    /// `doh` feature is compiled in.
    pub doh_endpoint: Option<String>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            ip_family: IpFamily::Auto,
            timeout: Duration::from_secs_f64(4.5),
            connect_timeout: Duration::from_secs_f64(4.0),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            retries: 3,
            doh_endpoint: None,
        }
    }
}

/// A reusable reqwest/rustls client with proxy, IP-family, and DoH settings.
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    no_redirect: reqwest::Client,
    config: HttpClientConfig,
}

fn build_client(
    config: &HttpClientConfig,
    redirect: reqwest::redirect::Policy,
) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(config.timeout)
        .connect_timeout(config.connect_timeout)
        .user_agent(config.user_agent.clone())
        .redirect(redirect);
    if let Some(proxy) = &config.proxy {
        let proxy = reqwest::Proxy::all(proxy)
            .map_err(|e| Error::usage(format!("Invalid proxy {proxy:?}: {e}")))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| Error::failure(format!("Failed to build HTTP client: {e}")))
}

impl HttpClient {
    /// Build a client from `config`.
    pub fn new(config: HttpClientConfig) -> Result<Self> {
        let inner = build_client(&config, reqwest::redirect::Policy::limited(10))?;
        let no_redirect = build_client(&config, reqwest::redirect::Policy::none())?;
        Ok(Self {
            inner,
            no_redirect,
            config,
        })
    }

    /// Return this client's configuration.
    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }

    /// Borrow the default (redirect-following) reqwest client.
    pub fn inner(&self) -> &reqwest::Client {
        &self.inner
    }

    /// Borrow the client that ignores redirects.
    pub fn client_for(&self, follow_redirects: bool) -> &reqwest::Client {
        if follow_redirects {
            &self.inner
        } else {
            &self.no_redirect
        }
    }

    /// Resolve `host` using the configured proxy/IP-family/DoH settings.
    pub async fn resolve_addresses(&self, host: &str) -> Vec<IpAddr> {
        resolve_dns(host, self.config.doh_endpoint.as_deref(), self.config.ip_family).await
    }

    /// Build a client that pins `host` to the first address of the selected IP
    /// family. With [`IpFamily::Auto`] this simply returns the shared client.
    pub async fn pinned_client(&self, host: &str, follow_redirects: bool) -> Result<reqwest::Client> {
        if self.config.ip_family == IpFamily::Auto {
            return Ok(self.client_for(follow_redirects).clone());
        }
        let addresses = self.resolve_addresses(host).await;
        let Some(address) = addresses.first() else {
            return Ok(self.client_for(follow_redirects).clone());
        };
        let redirect = if follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        };
        let mut builder = reqwest::Client::builder()
            .timeout(self.config.timeout)
            .connect_timeout(self.config.connect_timeout)
            .user_agent(self.config.user_agent.clone())
            .redirect(redirect)
            // The port in the override is ignored; the URL's port is used.
            .resolve(host, SocketAddr::new(*address, 0));
        if let Some(proxy) = &self.config.proxy {
            let proxy = reqwest::Proxy::all(proxy)
                .map_err(|e| Error::usage(format!("Invalid proxy {proxy:?}: {e}")))?;
            builder = builder.proxy(proxy);
        }
        builder
            .build()
            .map_err(|e| Error::failure(format!("Failed to build pinned HTTP client: {e}")))
    }
}

fn header_map(headers: &HashMap<String, String>) -> HeaderMap {
    let mut map = HeaderMap::with_capacity(headers.len());
    for (key, value) in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            map.insert(name, value);
        }
    }
    map
}

fn response_headers(response: &Response) -> HashMap<String, String> {
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

fn retry_delay(attempt: u32) -> Duration {
    // Jittered exponential backoff: (0.15 * 2^attempt) + random(0..0.15).
    // A time-derived jitter avoids pulling in a RNG dependency.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as f64
        / 1_000_000_000.0;
    let seconds = 0.15 * 2_f64.powi(attempt as i32) + nanos * 0.15;
    Duration::from_secs_f64(seconds)
}

/// Perform an HTTP request with jittered exponential backoff on transient
/// failures (connection errors and HTTP 502/503/504).
pub async fn request_with_retry(
    client: &HttpClient,
    method: Method,
    url: &str,
    headers: &HashMap<String, String>,
    retries: u32,
    timeout: Duration,
    follow_redirects: bool,
) -> Result<Response> {
    let parsed = Url::parse(url).map_err(|e| Error::usage(format!("Invalid URL {url:?}: {e}")))?;
    let host = parsed.host_str().unwrap_or("");
    let attempts = retries.max(1);
    let http_client = client.pinned_client(host, follow_redirects).await?;
    let mut last_error: Option<reqwest::Error> = None;

    for attempt in 0..attempts {
        let builder = http_client
            .request(method.clone(), url)
            .timeout(timeout)
            .headers(header_map(headers));

        match builder.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if matches!(status, 502..=504) && attempt + 1 < attempts {
                    debug!("HTTP {status} for {url}; retrying (attempt {})", attempt + 1);
                    tokio::time::sleep(retry_delay(attempt)).await;
                    continue;
                }
                return Ok(response);
            }
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < attempts {
                    tokio::time::sleep(retry_delay(attempt)).await;
                }
            }
        }
    }

    match last_error {
        Some(error) => Err(Error::failure(format!(
            "Request failed for {url}: {error}"
        ))),
        None => Err(Error::failure(format!("Request failed for {url}"))),
    }
}

fn probe_request_headers(extra: &HashMap<String, String>) -> HashMap<String, String> {
    let mut headers = HashMap::with_capacity(extra.len() + 2);
    headers.insert("User-Agent".to_owned(), PROBE_USER_AGENT.to_owned());
    headers.insert("Accept-Encoding".to_owned(), "identity".to_owned());
    for (key, value) in extra {
        headers.insert(key.clone(), value.clone());
    }
    headers
}

/// Send a streaming `GET` with `Range: bytes=0-0` to verify 206 support without
/// downloading the whole body.
pub async fn probe_range_support(
    client: &HttpClient,
    url: &str,
    headers: &HashMap<String, String>,
    timeout: Duration,
) -> Result<HttpProbeResult> {
    let mut request_headers = probe_request_headers(headers);
    request_headers.insert("Range".to_owned(), "bytes=0-0".to_owned());
    let response = request_with_retry(
        client,
        Method::GET,
        url,
        &request_headers,
        2,
        timeout,
        true,
    )
    .await?;

    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let all_headers = response_headers(&response);
    let content_type = all_headers
        .get("content-type")
        .cloned()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let filename = all_headers
        .get("content-disposition")
        .and_then(|value| parse_content_disposition_filename(Some(value)));

    let mut content_length = None;
    if let Some(range) = all_headers.get("content-range")
        && let Some(caps) = CONTENT_RANGE_TOTAL_RE.captures(range)
        && let Some(total) = caps.get(1).and_then(|m| m.as_str().parse::<u64>().ok())
    {
        content_length = Some(total);
    }
    if content_length.is_none() && status == 200 {
        content_length = all_headers
            .get("content-length")
            .and_then(|value| value.parse::<u64>().ok());
    }

    let accept_ranges = status == 206;
    let etag_sha256 = extract_etag_sha256(&all_headers);
    let verified_sha256 = extract_verified_sha256(&all_headers, &final_url);

    Ok(HttpProbeResult {
        ok: status == 200 || status == 206,
        final_url,
        status_code: status,
        content_type,
        content_length,
        filename,
        accept_ranges,
        etag_sha256,
        verified_sha256,
        headers: all_headers,
    })
}

/// Probe a URL via `HEAD`, falling back to `Range: bytes=0-0` on 403/405/501,
/// transport errors, or missing range confirmation.
pub async fn probe_http_url(
    client: &HttpClient,
    url: &str,
    headers: &HashMap<String, String>,
    timeout: Duration,
) -> Result<HttpProbeResult> {
    let request_headers = probe_request_headers(headers);
    let mut head_res: Option<HttpProbeResult> = None;

    match request_with_retry(
        client,
        Method::HEAD,
        url,
        &request_headers,
        2,
        timeout,
        true,
    )
    .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if !matches!(status, 400 | 403 | 405 | 501) {
                let all_headers = response_headers(&response);
                let final_url = response.url().to_string();
                let content_type = all_headers
                    .get("content-type")
                    .cloned()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let filename = all_headers
                    .get("content-disposition")
                    .and_then(|value| parse_content_disposition_filename(Some(value)));
                let content_length = all_headers
                    .get("content-length")
                    .and_then(|value| value.parse::<u64>().ok());
                let accept_ranges = all_headers
                    .get("accept-ranges")
                    .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
                let etag_sha256 = extract_etag_sha256(&all_headers);
                let verified_sha256 = extract_verified_sha256(&all_headers, &final_url);
                head_res = Some(HttpProbeResult {
                    ok: (200..400).contains(&status),
                    final_url,
                    status_code: status,
                    content_type,
                    content_length,
                    filename,
                    accept_ranges,
                    etag_sha256,
                    verified_sha256,
                    headers: all_headers,
                });
            }
        }
        Err(error) => debug!("HEAD probe failed for {url}: {error}"),
    }

    let need_range = head_res
        .as_ref()
        .is_none_or(|result| !result.ok || !result.accept_ranges);
    if need_range
        && let Ok(mut range_res) = probe_range_support(client, url, headers, timeout).await
        && range_res.ok
    {
        if let Some(head) = &head_res
            && head.ok
        {
            if range_res.filename.is_none() {
                range_res.filename = head.filename.clone();
            }
            if range_res.content_length.is_none() {
                range_res.content_length = head.content_length;
            }
            if range_res.etag_sha256.is_none() {
                range_res.etag_sha256 = head.etag_sha256.clone();
            }
            if range_res.verified_sha256.is_none() {
                range_res.verified_sha256 = head.verified_sha256.clone();
            }
        }
        return Ok(range_res);
    }

    Ok(head_res.unwrap_or(HttpProbeResult {
        final_url: url.to_owned(),
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_digest_matches_oracle() {
        assert_eq!(
            sha256_hex_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn find_digest_respects_word_boundaries() {
        assert_eq!(find_sha256_hex("xxabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"), None);
        assert!(find_sha256_hex("sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789").is_some());
    }
}
