//! Range probe, HTTP client construction, host-scoped headers, and opt-in DoH.
//!
// Modifications: a focused GET-only probe that records the *final* (post-redirect) URL.
// This is the control-plane fix for docs/diagnostics/watchdog-cycle-aria2-bug.md: the
// redirected URI is captured once and kept in the URI pool for the whole download, including
// every socket recycle — unlike aria2's pause/unpause, which lost it.
//
// W1-2 addition: optional DNS-over-HTTPS behind the `doh` feature.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

#[cfg(feature = "doh")]
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(feature = "doh")]
use std::sync::Arc;
use paradl_core::Error;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, ClientBuilder};
use url::Url;

use crate::error::Result;
use crate::state::Validators;
use crate::verify::Hash;

/// Bound the probe's response-header wait. Without it, a server that accepts the
/// TCP connection but never sends headers hangs `Download::start` forever — the
/// global "all waits have timeouts" constraint is otherwise violated here.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Build the shared HTTP client. rustls only (no OpenSSL / native TLS), optional SOCKS proxy,
/// optional IP-family pinning via a local bind, and optional DNS-over-HTTPS. All timeouts
/// are bounded.
///
/// `doh_endpoint` is accepted only when the `doh` feature is compiled in; otherwise a
/// diagnostic is logged and the operating-system resolver is used (charter §5).
pub fn build_client(
    proxy: Option<&str>,
    ip_family: Option<&str>,
    connect_timeout: Duration,
    doh_endpoint: Option<&str>,
) -> Result<Client> {
    // F15: do not force HTTP/1.1 — negotiate HTTP/2 when the server offers it.
    let mut b = ClientBuilder::new()
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::limited(10));

    if let Some(p) = proxy.filter(|p| !p.is_empty()) {
        b = b.proxy(reqwest::Proxy::all(p).map_err(|e| Error::failure(e.to_string()))?);
    }
    match ip_family.unwrap_or("auto").to_ascii_lowercase().as_str() {
        "ipv4" | "v4" => {
            b = b.local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        }
        "ipv6" | "v6" => {
            b = b.local_address(IpAddr::V6(Ipv6Addr::UNSPECIFIED));
        }
        _ => {}
    }

    if let Some(endpoint) = doh_endpoint.filter(|e| !e.is_empty()) {
        #[cfg(feature = "doh")]
        {
            b = b.dns_resolver(Arc::new(build_doh_resolver(endpoint)?));
        }
        #[cfg(not(feature = "doh"))]
        {
            tracing::warn!(
                endpoint = endpoint,
                "DoH endpoint ignored: paradl-engine was compiled without the `doh` feature; \
                 using the operating-system resolver"
            );
        }
    }
    b.build().map_err(|e| Error::failure(e.to_string()))
}

/// Turn a Python `dict[str, str]` into a HeaderMap, dropping invalid entries.
/// A raw `Cookie` entry is ignored: cookies must go through the scoped field.
pub fn headers_from_pairs(pairs: Option<&std::collections::HashMap<String, String>>) -> HeaderMap {
    let mut out = HeaderMap::new();
    if let Some(pairs) = pairs {
        for (k, v) in pairs {
            // F14: a raw Cookie entry in the headers dict would be sent to every host,
            // including cross-domain mirrors. Cookies must go through the scoped field.
            if k.eq_ignore_ascii_case("cookie") {
                continue;
            }
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                out.insert(name, value);
            }
        }
    }
    out
}

/// Sensitive request headers must never cross to a different host. reqwest strips
/// these during the probe's redirect chain, but workers issue *direct* requests to
/// the redirect-derived final URI (and to mirrors), where reqwest has no redirect
/// to policy over, so the same rule must be applied here explicitly. This mirrors
/// reqwest's own sensitive-header set; benign headers (User-Agent, X-Trace-Id, …)
/// pass through untouched.
pub fn scope_headers(
    headers: &HeaderMap,
    allowed_host: Option<&str>,
    target_host: Option<&str>,
) -> HeaderMap {
    let same_host = matches!((allowed_host, target_host), (Some(a), Some(t)) if a.eq_ignore_ascii_case(t));
    if same_host {
        return headers.clone();
    }
    let mut out = headers.clone();
    let _ = out.remove(reqwest::header::AUTHORIZATION);
    let _ = out.remove(reqwest::header::COOKIE);
    let _ = out.remove(reqwest::header::PROXY_AUTHORIZATION);
    out
}

/// What a one-request look at a URL tells us before downloading.
#[derive(Clone, Debug)]
pub struct ProbeInfo {
    /// The URL the user asked for.
    pub url: Url,
    /// The URL after redirects. **This is what every worker and every recycle uses.**
    pub final_url: Url,
    /// `Content-Length`, when the server announced one.
    pub size: Option<u64>,
    /// `Accept-Ranges: bytes` — whether segmented Range requests are supported.
    pub resumable: bool,
    /// The `Content-Disposition` (or URL-derived) filename.
    pub file_name: String,
    /// The response `Content-Type`, when present.
    pub content_type: Option<String>,
    /// Strong validators for conditional requests.
    pub validators: Validators,
    /// A server-announced hash, if any (strongest of Digest / Content-MD5 / x-goog-hash).
    pub checksum: Option<Hash>,
}

/// One plain GET to learn the final URL, size, name and validators. The body is not read:
/// dropping the response closes the connection. Redirects are followed by reqwest.
pub async fn probe(
    client: &Client,
    url: Url,
    mut headers: HeaderMap,
    cookie: Option<&str>,
) -> Result<ProbeInfo> {
    headers.remove(reqwest::header::RANGE);
    // F9: attach the browser's cookie to the look at the original URL. reqwest strips
    // Cookie/Authorization headers on a cross-origin redirect, so it cannot leak to the
    // final host of a different domain.
    if let Some(c) = cookie
        && let Ok(v) = HeaderValue::from_str(c)
    {
        headers.insert(reqwest::header::COOKIE, v);
    }
    let resp = tokio::time::timeout(PROBE_TIMEOUT, client.get(url.clone()).headers(headers).send())
        .await
        .map_err(|_| crate::error::timeout())?
        .map_err(|e| Error::failure(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(crate::error::status(resp.status().as_u16()));
    }

    let final_url = resp.url().clone();
    let h = resp.headers();
    let size = h
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let resumable = h
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("bytes"));
    let content_type = h
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let validators = Validators::from_response(
        h.get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()),
        h.get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok()),
        h.get(reqwest::header::DATE).and_then(|v| v.to_str().ok()),
        final_url.host_str(),
    );
    let checksum = Hash::from_headers(h);
    let file_name = file_name_from(h.get(reqwest::header::CONTENT_DISPOSITION).and_then(|v| v.to_str().ok()), &final_url);
    drop(resp);
    Ok(ProbeInfo {
        url,
        final_url,
        size,
        resumable,
        file_name,
        content_type,
        validators,
        checksum,
    })
}

/// `Content-Disposition` filename, else the final URL's last path segment (redirects included).
fn file_name_from(disposition: Option<&str>, final_url: &Url) -> String {
    if let Some(d) = disposition {
        if let Some(start) = d.find("filename*=") {
            let rest = &d[start + "filename*=".len()..];
            let value = rest.split(';').next().unwrap_or("").trim().trim_matches('"');
            if let Some((_, encoded)) = value.split_once("''") {
                return percent_decode(encoded);
            }
        }
        if let Some(start) = d.find("filename=") {
            let rest = &d[start + "filename=".len()..];
            let value = rest.split(';').next().unwrap_or("").trim().trim_matches('"');
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    final_url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .unwrap_or_else(|| "download".to_owned())
}

/// Minimal percent decoding for file names (`%XX` -> byte). UTF-8 lossy is acceptable here.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let (Some(a), Some(b)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(a * 16 + b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Opt-in DNS-over-HTTPS. Only compiled with `--features doh`; with the feature
// off, `build_client` above keeps the operating-system resolver and logs why.
// ---------------------------------------------------------------------------

#[cfg(feature = "doh")]
struct DohResolver {
    inner: hickory_resolver::TokioResolver,
}

#[cfg(feature = "doh")]
impl reqwest::dns::Resolve for DohResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let resolver = self.inner.clone();
        Box::pin(async move {
            let lookup = resolver
                .lookup_ip(name.as_str())
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let owned: Vec<SocketAddr> = lookup.iter().map(|ip| SocketAddr::new(ip, 0)).collect();
            let addrs: reqwest::dns::Addrs = Box::new(owned.into_iter());
            Ok(addrs)
        })
    }
}

/// Build a hickory-dns resolver that sends DNS queries over HTTPS to `endpoint`.
///
/// The endpoint is an http(s) URL such as `https://cloudflare-dns.com/dns-query`.
/// Its host name is bootstrapped once through the system resolver, then every
/// query for transfer hosts goes over the encrypted HTTPS transport.
#[cfg(feature = "doh")]
fn build_doh_resolver(endpoint: &str) -> Result<DohResolver> {
    use hickory_resolver::config::{NameServerConfig, ResolverConfig};
    use hickory_resolver::name_server::TokioConnectionProvider;
    use hickory_resolver::proto::xfer::Protocol;
    use hickory_resolver::Resolver;

    let url = Url::parse(endpoint).map_err(crate::error::url)?;
    if !matches!(url.scheme(), "https" | "http") {
        return Err(Error::failure("DoH endpoint must be an http(s) URL"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::failure("DoH endpoint has no host"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let socket_addr = (host, port)
        .to_socket_addrs()
        .map_err(Error::Io)?
        .next()
        .ok_or_else(|| Error::failure("DoH endpoint host did not resolve"))?;

    let mut ns = NameServerConfig::new(socket_addr, Protocol::Https);
    ns.http_endpoint = Some(url.path().to_owned());
    let mut config = ResolverConfig::new();
    config.add_name_server(ns);
    let inner = Resolver::builder_with_config(config, TokioConnectionProvider::default()).build();
    Ok(DohResolver { inner })
}
