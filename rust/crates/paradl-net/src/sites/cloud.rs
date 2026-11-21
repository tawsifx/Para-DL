//! Cloud-sharing URL resolvers (Google Drive, Dropbox, Pastebin).
//!
//! Pure URL rewrites ported from `src/paradl/engines/cloud.py`. No REST call is
//! needed: the rewritten URL is returned as a direct asset.

use std::sync::LazyLock;

use async_trait::async_trait;
use paradl_core::TargetType;
use regex::Regex;
use url::Url;

use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::Result;

static GDRIVE_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"drive\.google\.com/file/d/([a-zA-Z0-9_-]+)")
        .expect("Google Drive file regex must compile")
});
static GDRIVE_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[?&]id=([a-zA-Z0-9_-]+)").expect("Google Drive id regex must compile")
});
static PASTEBIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"pastebin\.com/([a-zA-Z0-9]+)$").expect("Pastebin regex must compile")
});

fn hostname(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(ToOwned::to_owned))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// Return `true` when a URL is a cloud-sharing link handled by this engine.
pub fn is_cloud_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("drive.google.com") || lower.contains("dropbox.com") || lower.contains("pastebin.com")
}

/// Rewrite a cloud preview/sharing URL into a direct stream URL.
///
/// Returns `(url, filename_hint)` exactly like `CloudResolver.resolve_url`.
pub fn resolve_cloud_url(url: &str) -> (String, Option<String>) {
    let original = url.trim();
    let host = hostname(original);

    if host == "drive.google.com"
        || host.ends_with(".drive.google.com")
        || original.contains("drive.google.com")
    {
        let file_id = GDRIVE_FILE_RE
            .captures(original)
            .or_else(|| GDRIVE_ID_RE.captures(original))
            .map(|caps| caps[1].to_owned());
        if let Some(file_id) = file_id {
            return (
                format!("https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"),
                None,
            );
        }
    }

    if host == "dropbox.com" || host.ends_with(".dropbox.com") || original.contains("dropbox.com") {
        if original.contains("dl=0") {
            return (original.replace("dl=0", "dl=1"), None);
        }
        if original.contains("dl=1") {
            return (original.to_owned(), None);
        }
        if original.contains('?') {
            return (format!("{original}&dl=1"), None);
        }
        return (format!("{original}?dl=1"), None);
    }

    if (host == "pastebin.com" || host.ends_with(".pastebin.com") || original.contains("pastebin.com"))
        && !original.contains("/raw/")
        && let Some(caps) = PASTEBIN_RE.captures(original)
    {
        let id = &caps[1];
        return (
            format!("https://pastebin.com/raw/{id}"),
            Some(format!("{id}.txt")),
        );
    }

    (original.to_owned(), None)
}

/// [`TargetResolver`] for cloud-sharing links.
#[derive(Debug, Default, Clone, Copy)]
pub struct CloudResolver;

impl CloudResolver {
    /// Create a cloud resolver.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TargetResolver for CloudResolver {
    fn scheme(&self) -> &'static str {
        "cloud"
    }

    async fn resolve(&self, target: &str, _ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let (url, filename) = resolve_cloud_url(target);
        Ok(vec![ResolvedAsset {
            url,
            filename,
            target_type: TargetType::Direct,
            ..Default::default()
        }])
    }
}
