// SPDX-License-Identifier: GPL-3.0-or-later
// Engine probe adapted from Mister Download Manager (c) Ahmed Nefzaoui
//   https://github.com/anefzaoui/misterdownloadmanager — crates/engine/src/download.rs (`probe`)
// Original engine is GPL-3.0-or-later. Written for paradl by Tawsif Rahman, 2026-11-17.
// Modifications: a focused GET-only probe that records the *final* (post-redirect) URL.
// This is the control-plane fix for docs/diagnostics/watchdog-cycle-aria2-bug.md: the
// redirected URI is captured once and kept in the URI pool for the whole download, including
// every socket recycle — unlike aria2's pause/unpause, which lost it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, ClientBuilder};
use url::Url;

use crate::error::{Error, Result};
use crate::state::Validators;
use crate::verify::Hash;

/// Bound the probe's response-header wait. Without it, a server that accepts the
/// TCP connection but never sends headers hangs `Download::start` forever — the
/// global "all waits have timeouts" constraint is otherwise violated here.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Build the shared HTTP client. rustls only (no OpenSSL / native TLS), optional SOCKS proxy,
/// optional IP-family pinning via a local bind. All timeouts are bounded.
pub fn build_client(
    proxy: Option<&str>,
    ip_family: Option<&str>,
    connect_timeout: Duration,
) -> Result<Client> {
    // F15: do not force HTTP/1.1 — negotiate HTTP/2 when the server offers it.
    let mut b = ClientBuilder::new()
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::limited(10));

    if let Some(p) = proxy.filter(|p| !p.is_empty()) {
        b = b.proxy(reqwest::Proxy::all(p).map_err(|e| Error::Network(e.to_string()))?);
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
    b.build().map_err(Error::from)
}

/// Turn a Python `dict[str, str]` into a HeaderMap, dropping invalid entries.
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
    pub size: Option<u64>,
    /// `Accept-Ranges: bytes` — whether segmented Range requests are supported.
    pub resumable: bool,
    pub file_name: String,
    pub content_type: Option<String>,
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
        .map_err(|_| Error::Timeout)??;
    if !resp.status().is_success() {
        return Err(Error::Status(resp.status().as_u16()));
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
