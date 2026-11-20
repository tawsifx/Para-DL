//! Network-free demonstration of the W2-3 site resolvers.
//!
//! Run with:
//! ```bash
//! cargo run -p paradl-net --example site_resolvers
//! ```
//!
//! No REST call is made: the example exercises shortcut parsing, asset/wheel
//! scoring, archive acceleration, cloud rewrites and token precedence only.

use std::collections::HashMap;
use std::sync::Arc;

use paradl_net::sites::accelerate::{accelerate_url_with_mirror, is_github_archive};
use paradl_net::sites::brew::{fallback_bottle_tags, parse_brew_target};
use paradl_net::sites::cloud::resolve_cloud_url;
use paradl_net::sites::github::{GithubTarget, score_asset_for_current_host};
use paradl_net::sites::hf::{HfRepoType, HfTarget, build_resolve_url};
use paradl_net::sites::ipsw::{is_ipsw, resolve_device_identifier};
use paradl_net::sites::pypi::{WheelTarget, parse_pypi_target, score_wheel};
use paradl_net::sites::token::{EnvSecretProvider, SecretProvider, SiteHeaders, SiteService};

fn main() {
    // 1. Shortcut parsing (parity row C51).
    let gh = GithubTarget::parse("https://github.com/astral-sh/uv?tab=readme-ov-file");
    println!("github: {} {}/{}", gh.kind(), gh.owner().unwrap_or("?"), gh.repo().unwrap_or("?"));
    let hf = HfTarget::parse("https://huggingface.co/meta-llama/Llama-3.1-8B/tree/main/onnx");
    println!("hf: {} @ {} ({:?})", hf.repo_id, hf.revision, hf.repo_type);
    println!("pypi: {:?}", parse_pypi_target("pip:requests==2.32.3"));
    println!("brew: {}", parse_brew_target("https://formulae.brew.sh/formula/ffmpeg"));
    println!("ipsw: {}", resolve_device_identifier("iphone 16"));
    assert!(is_ipsw("ipsw:iphone 16"));

    // 2. Asset / wheel scoring.
    let asset = "tool-1.0.0-aarch64-apple-darwin.tar.gz";
    println!("asset score: {}", score_asset_for_current_host(asset, false));
    let target = WheelTarget::new((3, 12), "linux", "x86_64");
    let wheel = "numpy-1.26.4-cp312-cp312-manylinux_2_17_x86_64.whl";
    println!("wheel score: {}", score_wheel(wheel, "bdist_wheel", &target));
    println!("bottle tags: {:?}", fallback_bottle_tags("darwin", "arm64", 15));

    // 3. Direct-URL rewrite / acceleration.
    let archive = "https://github.com/python/cpython/archive/refs/heads/main.zip";
    assert!(is_github_archive(archive));
    println!(
        "accelerated: {:?}",
        accelerate_url_with_mirror(archive, Some("https://gh-proxy.com/"))
    );
    println!("cloud: {:?}", resolve_cloud_url("https://pastebin.com/AbC123"));
    println!(
        "hf resolve: {}",
        build_resolve_url(&hf.repo_id, &hf.revision, "model.bin", HfRepoType::Model)
    );

    // 4. Token flows: env wins, then keychain; no secret is put in argv.
    let env = HashMap::from([
        ("GITHUB_TOKEN".to_owned(), "demo-token".to_owned()),
        ("GH_TOKEN".to_owned(), "ignored".to_owned()),
    ]);
    let secrets = Arc::new(EnvSecretProvider::with_env(env, None));
    assert_eq!(secrets.token(SiteService::Github).as_deref(), Some("demo-token"));
    println!("redaction registrations: {:?}", secrets.registered_secrets());
    println!(
        "github REST headers: {:?}",
        SiteHeaders::rest(SiteService::Github, secrets.as_ref())
    );
}
