//! Network-free demonstration of `paradl-net`'s routing and helper APIs.
//!
//! Run with:
//! ```bash
//! cargo run -p paradl-net --example resolve_target
//! cargo run -p paradl-net --example resolve_target --features doh
//! ```

use std::collections::HashMap;

use paradl_net::client::{
    extract_verified_sha256, parse_content_disposition_filename,
};
use paradl_net::dns::{DohEndpoint, IpFamily};
use paradl_net::routing::{
    PureClaimsChecker, ResolveContext, RouteDecision, classify_target, resolve_target,
};
use paradl_net::{
    HttpClient, HttpClientConfig, extract_etag_sha256, note_http3_drop, resolve_effective_isp,
};

#[tokio::main]
async fn main() -> Result<(), paradl_core::Error> {
    // 1. RFC 6266 filename parsing.
    let name = parse_content_disposition_filename(Some(
        "attachment; filename*=UTF-8''quarterly%20report.pdf",
    ));
    println!("filename: {name:?}");

    // 2. ETag digest extraction.
    let mut headers = HashMap::new();
    headers.insert(
        "x-linked-etag".to_owned(),
        "\"sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890\"".to_owned(),
    );
    println!("etag digest: {:?}", extract_etag_sha256(&headers));
    println!("verified digest: {:?}", extract_verified_sha256(&headers, "https://example.com/f"));

    // 3. Pure routing classifications (no network).
    for target in [
        "https://www.youtube.com/watch?v=abc",
        "https://example.com/archive.zip",
        "https://image.pollinations.ai/prompt/sunset",
        "lofi hip hop radio",
    ] {
        let decision = classify_target(target, None, &PureClaimsChecker);
        let label = match &decision {
            RouteDecision::Direct { .. } => "DIRECT",
            RouteDecision::DelegateToWorker { .. } => "DELEGATE",
            RouteDecision::Site { .. } => "SITE",
            RouteDecision::Torrent { .. } => "TORRENT",
        };
        println!("{target} -> {label} ({decision:?})");
    }

    // 4. TargetResolver dispatch (probing disabled => offline-safe).
    let client = HttpClient::new(HttpClientConfig {
        ip_family: IpFamily::Ipv4,
        ..Default::default()
    })?;
    let ctx = ResolveContext {
        allow_network_probe: false,
        ..Default::default()
    };
    let assets = resolve_target(
        "https://example.com/file.pdf",
        &ctx,
        &client,
        std::sync::Arc::new(PureClaimsChecker),
    )
    .await?;
    println!("resolved assets: {assets:?}");

    // 5. ISP handoff + dropped-flag compatibility.
    println!("explicit ISP: {}", resolve_effective_isp("Comcast Cable"));
    let endpoint = DohEndpoint::parse("https://1.1.1.1/dns-query")?;
    println!("DoH endpoint: {} {}", endpoint.authority(), endpoint.path());
    note_http3_drop();

    Ok(())
}
