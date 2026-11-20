//! Pure routing rules (final master prompt ruling 3).
//!
//! Rust keeps the cheap, side-effect-free classification from the single
//! `paradl-core` constants. The expensive/live decision (yt-dlp extractor
//! claims, full metadata, direct-vs-managed handoff) is delegated to the
//! worker; `claims=false` always falls back to a direct download.

use paradl_core::{has_direct_extension, is_image_domain, is_media_domain};

/// Outcome of a media routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRoute {
    /// The worker claimed and handled the URL.
    Managed,
    /// The caller should hand the URL to `paradl-engine` as a direct download.
    Direct,
}

impl MediaRoute {
    /// Whether this route requires the direct-download fallback.
    pub fn is_direct(self) -> bool {
        self == Self::Direct
    }

    /// Whether this route is handled by the Python worker.
    pub fn is_managed(self) -> bool {
        self == Self::Managed
    }
}

/// Classify a target using only pure rules from `paradl-core`.
///
/// `ytsearch1:` searches and media/image domains are managed; a recognized
/// direct-download extension is direct. Everything else is provisionally
/// managed because the worker owns the authoritative `claims` decision.
///
/// ```
/// use paradl_media::{MediaRoute, classify_route};
/// assert_eq!(classify_route("ytsearch1:lofi"), MediaRoute::Managed);
/// assert_eq!(classify_route("https://example.com/file.zip"), MediaRoute::Direct);
/// ```
pub fn classify_route(target: &str) -> MediaRoute {
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("ytsearch") || is_media_domain(target) || is_image_domain(target) {
        return MediaRoute::Managed;
    }
    if has_direct_extension(target) {
        return MediaRoute::Direct;
    }
    MediaRoute::Managed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_rules_use_core_domain_constants() {
        assert_eq!(classify_route("ytsearch1:hello"), MediaRoute::Managed);
        assert_eq!(classify_route("https://youtube.com/watch?v=x"), MediaRoute::Managed);
        assert_eq!(classify_route("https://cdn.example/a.zip"), MediaRoute::Direct);
        assert_eq!(classify_route("https://example.com/page"), MediaRoute::Managed);
    }
}
