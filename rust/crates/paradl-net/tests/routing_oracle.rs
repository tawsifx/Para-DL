//! Oracle-mirrored routing and network-helper tests for `paradl-net`.
//!
//! Every expected value below is captured from the READ-ONLY Python oracle:
//! `tests/unit/test_net.py`, `tests/integration/test_engine_batch.py`,
//! `tests/unit/test_bug_fixes.py`, and `paradl-oracle/fixtures/core_models.json`.
//! This file is authored FIRST (RED), then the crate is implemented to make it
//! pass (GREEN).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use paradl_core::TargetType;
use paradl_net::client::{
    HttpProbeResult, extract_etag_sha256, extract_verified_sha256,
    parse_content_disposition_filename,
};
use paradl_net::dns::{DohEndpoint, IpFamily, filter_addresses};
use paradl_net::isp::{
    UNKNOWN_ISP, clean_isp_name, reset_isp_detector, resolve_effective_isp, set_isp_detector,
};
use paradl_net::routing::{
    DelegateReason, MediaClaim, PureClaimsChecker, RouteDecision, classify_target,
    ytdlp_extractor_claims,
};
use paradl_net::{HTTP3_DROP_REASON, note_http3_drop};

/// Serializes tests that touch the process-global ISP detector override.
mod test_support {
    use std::sync::{Mutex, MutexGuard};

    static ISP_LOCK: Mutex<()> = Mutex::new(());

    pub fn isp_test_lock() -> MutexGuard<'static, ()> {
        ISP_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn probe_with(fields: HttpProbeResult) -> HttpProbeResult {
    fields
}

// ---------------------------------------------------------------------------
// test_net.py :: TestParseContentDispositionFilename (RFC 6266)
// ---------------------------------------------------------------------------

#[test]
fn rfc6266_plain_filename() {
    assert_eq!(
        parse_content_disposition_filename(Some("attachment; filename=\"report.pdf\"")),
        Some("report.pdf".to_owned())
    );
}

#[test]
fn rfc6266_filename_star_rfc5987() {
    assert_eq!(
        parse_content_disposition_filename(Some(
            "attachment; filename*=UTF-8''report%20file.pdf"
        )),
        Some("report file.pdf".to_owned())
    );
}

#[test]
fn rfc6266_filename_star_takes_priority() {
    assert_eq!(
        parse_content_disposition_filename(Some(
            "filename=\"old.pdf\"; filename*=UTF-8''new%20file.pdf"
        )),
        Some("new file.pdf".to_owned())
    );
}

#[test]
fn rfc6266_none_and_empty_return_none() {
    assert_eq!(parse_content_disposition_filename(None), None);
    assert_eq!(parse_content_disposition_filename(Some("")), None);
}

#[test]
fn rfc6266_path_traversal_blocked() {
    assert_eq!(
        parse_content_disposition_filename(Some("filename=\"../../../etc/passwd\"")),
        Some("passwd".to_owned())
    );
}

// ---------------------------------------------------------------------------
// test_net.py :: TestExtractETagSHA256
// ---------------------------------------------------------------------------

const DIGEST: &str = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";

fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn etag_x_linked_etag() {
    let h = headers(&[("x-linked-etag", &format!("\"sha256:{DIGEST}\""))]);
    assert_eq!(extract_etag_sha256(&h), Some(DIGEST.to_owned()));
}

#[test]
fn etag_plain_weak_etag() {
    let h = headers(&[("etag", &format!("\"W/{DIGEST}\""))]);
    assert_eq!(extract_etag_sha256(&h), Some(DIGEST.to_owned()));
}

#[test]
fn etag_none_and_invalid() {
    assert_eq!(extract_etag_sha256(&HashMap::new()), None);
    let h = headers(&[("etag", "\"not-a-sha256\"")]);
    assert_eq!(extract_etag_sha256(&h), None);
}

#[test]
fn verified_sha256_ignores_generic_and_weak_etags() {
    let generic = headers(&[("etag", &format!("\"{DIGEST}\""))]);
    assert_eq!(
        extract_verified_sha256(
            &generic,
            "https://codeload.github.com/goshitsarch-eng/gosh-dl/zip/refs/heads/main"
        ),
        None
    );
    let weak = headers(&[("etag", &format!("W/\"{DIGEST}\""))]);
    assert_eq!(
        extract_verified_sha256(
            &weak,
            "https://huggingface.co/org/repo/resolve/main/model.bin"
        ),
        None
    );
}

#[test]
fn verified_sha256_accepts_explicit_and_hf_headers() {
    let cases = [
        (
            headers(&[("x-linked-etag", &format!("\"{DIGEST}\""))]),
            "https://example.com/file.bin",
        ),
        (
            headers(&[("x-checksum-sha256", DIGEST)]),
            "https://example.com/file.bin",
        ),
        (
            headers(&[("etag", &format!("\"sha256:{DIGEST}\""))]),
            "https://example.com/file.bin",
        ),
        (
            headers(&[("etag", &format!("\"{DIGEST}\""))]),
            "https://huggingface.co/org/repo/resolve/main/model.bin",
        ),
    ];
    for (h, url) in cases {
        assert_eq!(extract_verified_sha256(&h, url), Some(DIGEST.to_owned()));
    }
}

// ---------------------------------------------------------------------------
// test_engine_batch.py :: TestImageProbeAndHttpErrors (exact classifications)
// ---------------------------------------------------------------------------

#[test]
fn image_domain_classified_as_direct() {
    let url = "https://image.pollinations.ai/prompt/vintage%20painting?width=800&height=450";
    let p = probe_with(HttpProbeResult {
        ok: true,
        status_code: 200,
        content_type: "image/jpeg".to_owned(),
        filename: Some("vintage-painting.jpg".to_owned()),
        final_url: url.to_owned(),
        ..Default::default()
    });
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    match decision {
        RouteDecision::Direct {
            target_type,
            filename_hint,
            mime_type,
            ..
        } => {
            assert_eq!(target_type, TargetType::Direct);
            assert_eq!(filename_hint.as_deref(), Some("vintage-painting.jpg"));
            assert_eq!(mime_type.as_deref(), Some("image/jpeg"));
        }
        other => panic!("expected Direct, got {other:?}"),
    }
}

#[test]
fn image_domain_402_surfaces_error_without_falling_to_stream() {
    let url = "https://image.pollinations.ai/prompt/expensive%20prompt";
    let p = probe_with(HttpProbeResult {
        ok: false,
        status_code: 402,
        final_url: url.to_owned(),
        ..Default::default()
    });
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    match decision {
        RouteDecision::Direct {
            target_type,
            status_code,
            probe_error,
            ..
        } => {
            assert_eq!(target_type, TargetType::Direct);
            assert_eq!(status_code, Some(402));
            let err = probe_error.expect("probe_error present");
            assert!(err.contains("402"), "{err}");
            assert!(err.contains("Payment Required"), "{err}");
        }
        other => panic!("expected Direct, got {other:?}"),
    }
}

#[test]
fn generic_url_404_surfaces_error() {
    let url = "https://example.com/api/missing_resource";
    let p = probe_with(HttpProbeResult {
        ok: false,
        status_code: 404,
        final_url: url.to_owned(),
        ..Default::default()
    });
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    match decision {
        RouteDecision::Direct {
            status_code,
            probe_error,
            ..
        } => {
            assert_eq!(status_code, Some(404));
            let err = probe_error.expect("probe_error present");
            assert!(err.contains("404"), "{err}");
            assert!(err.contains("Not Found"), "{err}");
        }
        other => panic!("expected Direct, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// core_models.json :: is_media_domain / is_image_domain / extension routing
// ---------------------------------------------------------------------------

#[test]
fn media_domain_urls_delegate_to_worker() {
    for url in [
        "https://www.youtube.com/watch?v=x",
        "https://youtu.be/x",
        "https://soundcloud.com/artist/track",
        "https://music.youtube.com/x",
    ] {
        let decision = classify_target(url, None, &PureClaimsChecker);
        match decision {
            RouteDecision::DelegateToWorker {
                target_type,
                reason,
                ..
            } => {
                assert_eq!(target_type, TargetType::Stream, "{url}");
                assert_eq!(reason, DelegateReason::MediaDomain, "{url}");
            }
            other => panic!("expected Stream delegate for {url}, got {other:?}"),
        }
    }
}

#[test]
fn direct_extension_urls_are_direct() {
    let decision = classify_target("https://example.com/file.zip", None, &PureClaimsChecker);
    match decision {
        RouteDecision::Direct {
            target_type,
            filename_hint,
            ..
        } => {
            assert_eq!(target_type, TargetType::Direct);
            assert_eq!(filename_hint.as_deref(), Some("file.zip"));
        }
        other => panic!("expected Direct, got {other:?}"),
    }
}

#[test]
fn image_domain_recognition_matches_oracle() {
    for url in [
        "https://image.pollinations.ai/prompt/sunset",
        "https://pollinations.ai/prompt/cat",
        "https://images.unsplash.com/photo-12345",
        "https://i.imgur.com/abcde",
        "https://i.redd.it/xyz123",
        "https://cdn.discordapp.com/attachments/123",
    ] {
        assert!(paradl_net::is_image_domain(url), "{url}");
    }
    for url in [
        "https://youtube.com/watch?v=12345",
        "https://github.com/tawsifx/pdl",
        "https://example.com/image.png",
    ] {
        assert!(!paradl_net::is_image_domain(url), "{url}");
    }
}

// ---------------------------------------------------------------------------
// client.py:787-850 :: HEAD/Range heuristics
// ---------------------------------------------------------------------------

#[test]
fn text_html_probe_delegates_to_worker() {
    let url = "https://example.com/page";
    let p = HttpProbeResult {
        ok: true,
        status_code: 200,
        content_type: "text/html; charset=utf-8".to_owned(),
        ..Default::default()
    };
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    match decision {
        RouteDecision::DelegateToWorker {
            target_type,
            reason,
            ..
        } => {
            assert_eq!(target_type, TargetType::Stream);
            assert_eq!(reason, DelegateReason::HtmlContentType);
        }
        other => panic!("expected Stream delegate, got {other:?}"),
    }
}

#[test]
fn application_octet_stream_probe_is_direct() {
    let url = "https://example.com/blob";
    let p = HttpProbeResult {
        ok: true,
        status_code: 200,
        content_type: "application/octet-stream".to_owned(),
        content_length: Some(5000),
        ..Default::default()
    };
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    assert!(matches!(decision, RouteDecision::Direct { .. }), "{decision:?}");
}

#[test]
fn range_capable_non_html_probe_is_direct() {
    let url = "https://example.com/data.bin";
    let p = HttpProbeResult {
        ok: true,
        status_code: 206,
        content_type: "application/octet-stream".to_owned(),
        accept_ranges: true,
        ..Default::default()
    };
    let decision = classify_target(url, Some(&p), &PureClaimsChecker);
    assert!(matches!(decision, RouteDecision::Direct { .. }), "{decision:?}");
}

// ---------------------------------------------------------------------------
// client.py:852-853 :: free text -> SEARCH
// ---------------------------------------------------------------------------

#[test]
fn free_text_is_search_with_ytsearch_prefix() {
    let decision = classify_target("some movie 2024", None, &PureClaimsChecker);
    match decision {
        RouteDecision::DelegateToWorker {
            target_type,
            clean_url,
            ..
        } => {
            assert_eq!(target_type, TargetType::Search);
            assert_eq!(clean_url, "ytsearch1:some movie 2024");
        }
        other => panic!("expected Search delegate, got {other:?}"),
    }
}

#[test]
fn ytsearch_prefix_is_search() {
    let decision = classify_target("ytsearch1:lofi mix", None, &PureClaimsChecker);
    assert!(matches!(
        decision,
        RouteDecision::DelegateToWorker {
            target_type: TargetType::Search,
            ..
        }
    ));
}

#[test]
fn magnet_is_torrent() {
    let decision = classify_target("magnet:?xt=urn:btih:abc", None, &PureClaimsChecker);
    assert!(matches!(decision, RouteDecision::Torrent { .. }));
}

// ---------------------------------------------------------------------------
// ytdlp_extractor_claims_url handoff (ruling 3)
// ---------------------------------------------------------------------------

#[test]
fn claims_delegates_media_domain_and_falls_back_direct() {
    assert_eq!(
        ytdlp_extractor_claims("https://youtube.com/watch?v=x", &PureClaimsChecker),
        MediaClaim::DelegateToWorker
    );
    assert_eq!(
        ytdlp_extractor_claims("https://example.com/file.bin", &PureClaimsChecker),
        MediaClaim::DirectFallback
    );
}

// ---------------------------------------------------------------------------
// bug_fixes.py :: L4 ISP detection + W0-3 "auto" handoff
// ---------------------------------------------------------------------------

#[test]
fn isp_explicit_config_passthrough() {
    assert_eq!(resolve_effective_isp("Comcast Cable"), "Comcast Cable");
}

#[test]
fn isp_auto_sentinel_resolves_via_detector() {
    let gate = test_support::isp_test_lock();
    set_isp_detector(Some(|| Some("Detected Fiber ISP".to_owned())));
    assert_eq!(resolve_effective_isp("auto"), "Detected Fiber ISP");
    reset_isp_detector();
    drop(gate);
}

#[test]
fn isp_detection_transient_failure_is_not_cached() {
    let gate = test_support::isp_test_lock();
    reset_isp_detector();
    set_isp_detector(Some(|| None));
    assert_eq!(resolve_effective_isp("auto"), UNKNOWN_ISP);
    // A second call still reaches the detector (no "Unknown ISP" cache).
    set_isp_detector(Some(|| Some("Recovered ISP".to_owned())));
    assert_eq!(resolve_effective_isp("auto"), "Recovered ISP");
    reset_isp_detector();
    drop(gate);
}

#[test]
fn clean_isp_name_strips_as_number() {
    assert_eq!(clean_isp_name("AS149048 - Comcast Cable"), "Comcast Cable");
    assert_eq!(clean_isp_name("  \"Example Telecom\"  "), "Example Telecom");
}

// ---------------------------------------------------------------------------
// DNS / proxy / IP-family / dropped-flag plumbing
// ---------------------------------------------------------------------------

#[test]
fn ip_family_filters_addresses() {
    let addrs = [
        IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    ];
    assert_eq!(filter_addresses(&addrs, IpFamily::Auto), addrs.to_vec());
    assert_eq!(
        filter_addresses(&addrs, IpFamily::Ipv4),
        vec![addrs[0], addrs[2]]
    );
    assert_eq!(filter_addresses(&addrs, IpFamily::Ipv6), vec![addrs[1]]);
}

#[test]
fn doh_endpoint_parse_and_defaults() {
    let e = DohEndpoint::parse("https://1.1.1.1/dns-query").unwrap();
    assert_eq!(e.host(), "1.1.1.1");
    assert_eq!(e.path(), "/dns-query");
    assert_eq!(e.port(), 443);

    let e2 = DohEndpoint::parse("https://cloudflare-dns.com").unwrap();
    assert_eq!(e2.path(), "/dns-query");

    assert!(DohEndpoint::parse("ftp://example.com/dns-query").is_err());
    assert!(DohEndpoint::parse("not a url").is_err());
}

#[test]
fn http3_flag_is_log_dropped_with_reason() {
    assert!(!HTTP3_DROP_REASON.is_empty());
    // Calling the dropped-flag helper must be infallible and never enable HTTP/3.
    note_http3_drop();
}

#[test]
fn http3_compat_shim_is_still_public_api() {
    let _: fn() = paradl_net::note_http3_drop;
    let _: &str = paradl_net::HTTP3_DROP_REASON;
}
