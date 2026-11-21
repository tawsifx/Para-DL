//! Range-capable acceleration for GitHub source archives.
//!
//! Mirrors `src/paradl/net/accelerate.py`: `codeload.github.com` does not honour
//! HTTP Range requests, so a source archive is rewritten through a mirror. The
//! mirror prefix is read at call time from `PARADL_GITHUB_ARCHIVE_MIRROR`; an
//! empty value disables acceleration.

use url::Url;

/// Environment variable holding the archive mirror prefix.
pub const MIRROR_ENV: &str = "PARADL_GITHUB_ARCHIVE_MIRROR";

/// Default Range-capable mirror (`gh-proxy.com`).
pub const DEFAULT_MIRROR: &str = "https://gh-proxy.com/";

/// Return `true` for GitHub `/archive/` source-archive URLs (zip/tar.gz).
///
/// ```
/// use paradl_net::sites::accelerate::is_github_archive;
/// assert!(is_github_archive("https://github.com/python/cpython/archive/refs/heads/main.zip"));
/// assert!(!is_github_archive("https://example.com/archive/x.zip"));
/// ```
pub fn is_github_archive(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "github.com" | "www.github.com" | "codeload.github.com"
    ) {
        return false;
    }
    parsed.path().contains("/archive/")
}

/// Rewrite a GitHub archive URL through `mirror` (or the default mirror when
/// `None`). Returns `None` for non-archive URLs or an empty mirror prefix.
///
/// The original URL is passed through unchanged, exactly as the Python oracle
/// does (`prefix.rstrip("/") + "/" + url.lstrip("/")`).
pub fn accelerate_url_with_mirror(url: &str, mirror: Option<&str>) -> Option<String> {
    if !is_github_archive(url) {
        return None;
    }
    let prefix = mirror.unwrap_or(DEFAULT_MIRROR);
    if prefix.trim().is_empty() {
        return None;
    }
    Some(format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        url.trim_start_matches('/')
    ))
}

/// Rewrite a GitHub archive URL using `PARADL_GITHUB_ARCHIVE_MIRROR` (read at
/// call time, so toggling the variable needs no restart).
pub fn accelerate_url(url: &str) -> Option<String> {
    let prefix = std::env::var(MIRROR_ENV).unwrap_or_else(|_| DEFAULT_MIRROR.to_owned());
    accelerate_url_with_mirror(url, Some(&prefix))
}
