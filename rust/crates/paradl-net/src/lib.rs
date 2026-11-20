//! Network layer for paraDL: reqwest/rustls client, HTTP HEAD/Range probing,
//! RFC 6266 filenames, ETag digests, DNS/DoH, proxy + IP-family selection,
//! pure routing rules, and the public [`TargetResolver`] trait.
//!
//! # Routing split (framework charter §2, ruling 3)
//!
//! Rust owns the **pure** routing rules: hostname-suffix matching against the
//! `paradl-core` single-source domain lists, direct-extension suffix matching,
//! `ytsearch1:` free-text search, and HTTP HEAD/Range heuristics (`text/html`
//! delegates to the media worker). All domain lists are re-exported from
//! `paradl-core` so there is exactly one constant and no drift.
//!
//! Rust **does not** implement yt-dlp extraction. For any URL that may need an
//! extractor, [`routing::ytdlp_extractor_claims`] returns a
//! [`routing::MediaClaim::DelegateToWorker`] decision; the live extractor
//! registry and full metadata live in the Python media worker
//! (`python -m paradl_media`, protocol `paradl-media/1`, owned by W3-4). When
//! `claims=false`, routing falls back to [`routing::MediaClaim::DirectFallback`].
//!
//! # Feature flags
//!
//! * `doh` (opt-in) — DNS-over-HTTPS via the hickory-dns project's resolver.
//!   Without it, [`dns::resolve_dns`] uses the system resolver only.
//!
//! HTTP/3 is **not** implemented. The legacy `--http3` CLI flag is kept
//! compatible by calling [`note_http3_drop`], which logs a reason; no Cargo
//! feature is exposed for that unimplemented capability.
//!
//! ```
//! use paradl_net::client::parse_content_disposition_filename;
//! use paradl_net::routing::{classify_target, PureClaimsChecker, RouteDecision};
//!
//! let name = parse_content_disposition_filename(Some(
//!     "attachment; filename*=UTF-8''report%20file.pdf",
//! ));
//! assert_eq!(name.as_deref(), Some("report file.pdf"));
//!
//! let decision = classify_target("https://youtu.be/abc", None, &PureClaimsChecker);
//! assert!(matches!(decision, RouteDecision::DelegateToWorker { .. }));
//! ```

#![warn(missing_docs)]

pub mod client;
pub mod compat;
pub mod dns;
pub mod isp;
pub mod routing;

pub use client::{
    DEFAULT_USER_AGENT, HttpClient, HttpClientConfig, HttpProbeResult,
    extract_etag_sha256, extract_verified_sha256, parse_content_disposition_filename,
    probe_http_url, probe_range_support, request_with_retry,
};
pub use compat::{HTTP3_DROP_REASON, note_http3_drop};
pub use dns::{DohEndpoint, IpFamily, filter_addresses, resolve_dns, resolve_dns_doh};
pub use isp::{
    AUTO_ISP_SENTINEL, UNKNOWN_ISP, clean_isp_name, detect_isp, reset_isp_detector,
    resolve_effective_isp, set_isp_detector,
};
// The domain/extension lists live in `paradl-core` and are re-exported here as
// the single source of truth for routing (one generated constant, no drift).
pub use paradl_core::{
    DIRECT_EXTENSIONS, IMAGE_DOMAINS, MEDIA_DOMAINS, has_direct_extension, is_image_domain,
    is_media_domain,
};
pub use routing::{
    DelegateReason, ExtractorClaims, MediaClaim, MediaOp, PureClaimsChecker, ResolveContext,
    ResolvedAsset, RouteDecision, TargetResolver, classify_target, resolve_target,
    ytdlp_extractor_claims,
};

/// The framework's single public error type, re-exported for convenience.
pub use paradl_core::Error;

/// Convenience result alias using the framework's single public error type.
pub type Result<T> = std::result::Result<T, paradl_core::Error>;
