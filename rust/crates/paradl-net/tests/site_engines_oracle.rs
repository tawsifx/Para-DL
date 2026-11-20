//! Oracle-mirrored tests for the W2-3 site engines.
//!
//! Every expected value below is captured VERBATIM from the READ-ONLY Python
//! oracle: `tests/unit/test_bug_fixes.py`, `tests/test_github_acceleration.py`,
//! `tests/unit/test_pypi_transitive.py`, and the site engines in
//! `src/paradl/engines/{github,hf,brew,pypi,ipsw,cloud}.py`.
//!
//! This file is authored FIRST (RED), then `paradl_net::sites` is implemented
//! to make it pass (GREEN). W1-4 routing contracts are untouched.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use async_trait::async_trait;
use paradl_core::TargetType;
use paradl_net::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use paradl_net::sites::brew::{BrewResolver, fallback_bottle_tags};
use paradl_net::sites::cloud::{CloudResolver, resolve_cloud_url};
use paradl_net::sites::github::{
    GithubResolver, GithubTarget, extract_asset_digest, score_asset_for_host,
};
use paradl_net::sites::hf::{HfRepoType, HfResolver, HfTarget, build_resolve_url, filter_siblings};
use paradl_net::sites::ipsw::{
    IpswResolver, is_ipsw, parse_ipsw_filename, resolve_device_identifier,
};
use paradl_net::sites::pypi::{
    PyPIResolver, WheelTarget, extract_wheel_dependencies, parse_pypi_target, score_wheel,
};
use paradl_net::sites::token::{EnvSecretProvider, Keychain, SecretProvider, SiteHeaders, SiteService};
use paradl_net::sites::{
    JsonResponse, RedirectInfo, SiteHttp, SiteRouter, glob_match, accelerate::{
        accelerate_url_with_mirror, is_github_archive,
    },
};

// ---------------------------------------------------------------------------
// Test-only transport: scripted responses so REST resolution is deterministic.
// `SiteHttp` is the seam the Python oracle monkeypatches (`fetch_json_with_etag`).
// ---------------------------------------------------------------------------

struct Scripted {
    routes: HashMap<String, (u16, serde_json::Value)>,
    redirects: HashMap<String, Option<RedirectInfo>>,
}

impl Scripted {
    fn new() -> Self {
        Self {
            routes: HashMap::new(),
            redirects: HashMap::new(),
        }
    }

    fn json(mut self, url: &str, status: u16, body: serde_json::Value) -> Self {
        self.routes.insert(url.to_owned(), (status, body));
        self
    }

    fn redirect(mut self, url: &str, info: Option<(&str, Option<u64>)>) -> Self {
        self.redirects.insert(
            url.to_owned(),
            info.map(|(u, n)| RedirectInfo {
                url: u.to_owned(),
                headers: HashMap::new(),
                content_length: n,
            }),
        );
        self
    }
}

#[async_trait]
impl SiteHttp for Scripted {
    async fn fetch_json(
        &self,
        url: &str,
        _headers: &HashMap<String, String>,
        _timeout: std::time::Duration,
    ) -> paradl_net::Result<JsonResponse> {
        let (status, body) = self
            .routes
            .get(url)
            .cloned()
            .unwrap_or((404, serde_json::Value::Null));
        Ok(JsonResponse {
            status,
            headers: HashMap::new(),
            body,
        })
    }

    async fn redirect(
        &self,
        url: &str,
        _headers: &HashMap<String, String>,
        _timeout: std::time::Duration,
    ) -> paradl_net::Result<Option<RedirectInfo>> {
        Ok(self.redirects.get(url).cloned().flatten())
    }
}

fn context() -> ResolveContext {
    ResolveContext {
        allow_network_probe: false,
        ..Default::default()
    }
}

struct TestKeychain {
    token: String,
}
impl Keychain for TestKeychain {
    fn load_secret(&self, _service: &str, _account: &str) -> Option<String> {
        Some(self.token.clone())
    }
}

// ===========================================================================
// GitHub — tests/unit/test_bug_fixes.py L1 + engines/github.py
// ===========================================================================

#[test]
fn github_repo_url_with_query_string_matches_oracle() {
    let p = GithubTarget::parse("https://github.com/astral-sh/uv?tab=readme-ov-file");
    assert_eq!(p.kind(), "repo");
    assert_eq!(p.owner().unwrap(), "astral-sh");
    assert_eq!(p.repo().unwrap(), "uv");

    let rel = GithubTarget::parse("https://github.com/astral-sh/uv/releases?after=0.1.0");
    assert_eq!(rel.kind(), "release");
    assert_eq!(rel.owner().unwrap(), "astral-sh");
    assert_eq!(rel.repo().unwrap(), "uv");
    assert_eq!(rel.tag().unwrap(), "latest");
}

#[test]
fn github_asset_blob_tree_gist_and_shortcut_parsing() {
    let asset = GithubTarget::parse(
        "https://github.com/owner/repo/releases/download/v1.2.3/tool.zip?x=1",
    );
    assert_eq!(asset.kind(), "asset");
    assert_eq!(asset.owner().unwrap(), "owner");
    assert_eq!(asset.repo().unwrap(), "repo");
    assert_eq!(asset.tag().unwrap(), "v1.2.3");
    assert_eq!(asset.filename().unwrap(), "tool.zip");

    let blob = GithubTarget::parse("https://github.com/owner/repo/blob/main/src/main.rs");
    assert_eq!(blob.kind(), "blob");
    assert_eq!(blob.url(), "https://raw.githubusercontent.com/owner/repo/main/src/main.rs");
    assert_eq!(blob.filename().unwrap(), "main.rs");

    let tree = GithubTarget::parse("https://github.com/owner/repo/tree/main/docs");
    assert_eq!(tree.kind(), "tree");
    assert_eq!(tree.path().unwrap(), "docs");

    let gist = GithubTarget::parse("https://gist.github.com/user/a1b2c3d4");
    assert_eq!(gist.kind(), "gist");
    assert_eq!(gist.user().unwrap(), "user");
    assert_eq!(gist.gist_id().unwrap(), "a1b2c3d4");

    let short = GithubTarget::parse("gh:astral-sh/uv");
    assert_eq!(short.kind(), "repo");
    assert_eq!(short.url(), "https://github.com/astral-sh/uv");
}

#[test]
fn github_asset_scoring_is_byte_exact_for_macos_arm64() {
    // The oracle's first ARM branch also matches the literal "apple-darwin",
    // so even x86_64 / universal macOS names score +35 on an ARM host.
    assert_eq!(score_asset_for_host("tool-1.0.0-aarch64-apple-darwin.tar.gz", false, "darwin", true), 90);
    assert_eq!(score_asset_for_host("tool-1.0.0-x86_64-apple-darwin.tar.gz", false, "darwin", true), 90);
    assert_eq!(score_asset_for_host("tool-1.0.0-universal-apple-darwin.dmg", false, "darwin", true), 100);
    assert_eq!(score_asset_for_host("tool-1.0.0.sha256", false, "darwin", true), 0);
    assert_eq!(score_asset_for_host("tool-1.0.0-linux-x86_64.tar.gz", false, "darwin", true), 0);
    assert_eq!(score_asset_for_host("tool-1.0.0-any.zip", false, "darwin", true), 5);
}

#[test]
fn github_asset_scoring_is_byte_exact_for_windows_x86_64() {
    assert_eq!(score_asset_for_host("tool-1.0.0-windows-x86_64.zip", false, "windows", false), 100);
    assert_eq!(score_asset_for_host("tool-1.0.0-windows-x86_64.exe", false, "windows", false), 95);
    assert_eq!(score_asset_for_host("tool-1.0.0-windows-x86_64.exe", true, "windows", false), 90);
    assert_eq!(score_asset_for_host("tool-1.0.0-windows-x86_64.msi", false, "windows", false), 90);
    assert_eq!(score_asset_for_host("tool-1.0.0-windows-x86_64.msi", true, "windows", false), 75);
    assert_eq!(score_asset_for_host("tool-1.0.0-macos-arm64.zip", false, "windows", false), 0);
    assert_eq!(score_asset_for_host("tool-1.0.0.sha256", false, "windows", false), 0);
    assert_eq!(score_asset_for_host("tool-1.0.0-any.zip", false, "windows", false), 10);
}

#[test]
fn github_asset_digest_keeps_sha256_prefix_verbatim() {
    let asset: serde_json::Value = serde_json::json!({ "digest": "  sha256:abcdef  " });
    assert_eq!(extract_asset_digest(&asset).as_deref(), Some("sha256:abcdef"));
    assert_eq!(extract_asset_digest(&serde_json::json!({})), None);
}

#[tokio::test]
async fn github_release_asset_selection_and_digest_matches_oracle() {
    let release = serde_json::json!({
        "tag_name": "v1.0.0",
        "assets": [
            {"name": "tool-1.0.0.sha256", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool.sha256", "size": 100, "digest": "sha256:0000"},
            {"name": "tool-1.0.0-x86_64-macos.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-mac.tar.gz", "size": 1000, "digest": "sha256:1111"},
            {"name": "tool-1.0.0-aarch64-macos.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-arm.tar.gz", "size": 1100, "digest": "sha256:2222"}
        ]
    });
    let api = "https://api.github.com/repos/o/r/releases/latest";
    let http = Scripted::new().json(api, 200, release);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = GithubResolver::new(http, secrets).with_scoring_host("darwin", true);

    let assets = resolver.resolve("gh:o/r", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].target_type, TargetType::Github);
    assert_eq!(assets[0].url, "https://github.com/o/r/releases/download/v1.0.0/tool-arm.tar.gz");
    assert_eq!(assets[0].filename.as_deref(), Some("tool-1.0.0-aarch64-macos.tar.gz"));
    assert_eq!(assets[0].content_length, Some(1100));
    assert_eq!(assets[0].digest.as_deref(), Some("sha256:2222"));
}

#[tokio::test]
async fn github_asset_pattern_narrows_selection() {
    let release = serde_json::json!({
        "tag_name": "v1.0.0",
        "assets": [
            {"name": "tool-x86_64-apple-darwin.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-x86.tar.gz", "size": 10},
            {"name": "tool-aarch64-apple-darwin.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-arm.tar.gz", "size": 20}
        ]
    });
    let api = "https://api.github.com/repos/o/r/releases/tags/v2";
    let http = Scripted::new().json(api, 200, release);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = GithubResolver::new(http, secrets)
        .with_scoring_host("darwin", true)
        .with_asset_pattern("*x86_64*");

    let assets = resolver
        .resolve("https://github.com/o/r/releases/tag/v2", &context())
        .await
        .unwrap();
    assert_eq!(assets[0].url, "https://github.com/o/r/releases/download/v1.0.0/tool-x86.tar.gz");
}

// ===========================================================================
// GitHub acceleration — tests/test_github_acceleration.py (verbatim URLs)
// ===========================================================================

#[test]
fn github_archive_detection_matches_oracle() {
    assert!(is_github_archive("https://github.com/python/cpython/archive/refs/heads/main.zip"));
    assert!(is_github_archive("https://github.com/a/b/archive/refs/tags/v1.zip"));
    assert!(!is_github_archive("https://example.com/archive/x.zip"));
}

#[test]
fn github_archive_acceleration_is_byte_exact() {
    let url = "https://github.com/python/cpython/archive/refs/heads/main.zip";
    let mirror = "https://gh-proxy.com/";
    assert_eq!(
        accelerate_url_with_mirror(url, Some(mirror)),
        Some(format!("{mirror}{url}")),
    );
    // Empty mirror prefix disables acceleration (Python: `if not prefix.strip()`).
    assert_eq!(accelerate_url_with_mirror(url, Some("")), None);
    assert_eq!(
        accelerate_url_with_mirror("https://example.com/file.zip", Some(mirror)),
        None,
    );
}

// ===========================================================================
// Hugging Face — tests/unit/test_bug_fixes.py M3 + M4, engines/hf.py
// ===========================================================================

#[test]
fn hf_tree_url_parsing_matches_oracle() {
    let p = HfTarget::parse("https://huggingface.co/meta-llama/Llama-3.1-8B/tree/main/onnx");
    assert_eq!(p.repo_id, "meta-llama/Llama-3.1-8B");
    assert_eq!(p.subpath.as_deref(), Some("onnx"));
    assert_eq!(p.revision, "main");
    assert_eq!(p.repo_type, HfRepoType::Model);
    assert!(!p.is_direct_file);
}

#[test]
fn hf_shortcut_dataset_and_direct_file_parsing() {
    let short = HfTarget::parse("hf:thebloke/llama-7b-gguf");
    assert_eq!(short.repo_id, "thebloke/llama-7b-gguf");
    assert_eq!(short.subpath, None);
    assert_eq!(short.revision, "main");

    let ds = HfTarget::parse("hf:datasets/squad");
    assert_eq!(ds.repo_id, "squad");
    assert_eq!(ds.repo_type, HfRepoType::Dataset);

    let file = HfTarget::parse("https://huggingface.co/org/repo/resolve/main/model.bin?download=true");
    assert_eq!(file.repo_id, "org/repo");
    assert_eq!(file.subpath.as_deref(), Some("model.bin"));
    assert_eq!(file.revision, "main");
    assert!(file.is_direct_file);
}

#[test]
fn hf_resolve_url_is_byte_exact() {
    assert_eq!(
        build_resolve_url("meta-llama/Llama-3.1-8B", "main", "onnx/model.bin", HfRepoType::Model),
        "https://huggingface.co/meta-llama/Llama-3.1-8B/resolve/main/onnx/model.bin",
    );
    assert_eq!(
        build_resolve_url("squad", "main", "data.json", HfRepoType::Dataset),
        "https://huggingface.co/datasets/squad/resolve/main/data.json",
    );
}

#[test]
fn hf_unmatched_pattern_error_matches_oracle() {
    let siblings = vec![
        serde_json::json!({"rfilename": "model.bin", "size": 1000}),
        serde_json::json!({"rfilename": "config.json", "size": 100}),
    ];
    let err = filter_siblings(&siblings, None, Some("*.nonexistent"), "test/model").unwrap_err();
    assert_eq!(err.to_string(), "No files matched pattern '*.nonexistent' in repository test/model.");
}

#[tokio::test]
async fn hf_lfs_digest_and_resolve_url_matches_oracle() {
    let info = serde_json::json!({
        "siblings": [
            {"rfilename": "model.bin", "lfs": {"oid": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 1000}},
            {"rfilename": ".gitattributes"}
        ]
    });
    let api = "https://huggingface.co/api/models/test/model";
    let http = Scripted::new().json(api, 200, info);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = HfResolver::new(http, secrets);

    let assets = resolver.resolve("hf:test/model", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].url, "https://huggingface.co/test/model/resolve/main/model.bin");
    assert_eq!(assets[0].filename.as_deref(), Some("model.bin"));
    assert_eq!(assets[0].content_length, Some(1000));
    assert_eq!(
        assets[0].digest.as_deref(),
        Some("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    );
    assert_eq!(assets[0].target_type, TargetType::Hf);
}

// ===========================================================================
// Homebrew — engines/brew.py (bottle tag + GHCR resolution)
// ===========================================================================

#[test]
fn brew_parse_target_and_tag_fallback_are_byte_exact() {
    use paradl_net::sites::brew::parse_brew_target;
    assert_eq!(parse_brew_target("brew:wget"), "wget");
    assert_eq!(parse_brew_target("homebrew:jq"), "jq");
    assert_eq!(parse_brew_target("https://formulae.brew.sh/formula/ffmpeg"), "ffmpeg");
    assert_eq!(parse_brew_target("https://formulae.brew.sh/api/formula/wget.json"), "wget");

    assert_eq!(
        fallback_bottle_tags("darwin", "arm64", 15),
        vec!["arm64_sequoia", "arm64_sonoma", "arm64_ventura", "all"],
    );
    assert_eq!(
        fallback_bottle_tags("linux", "x86_64", 15),
        vec!["x86_64_linux", "x86_64_linux", "all"],
    );
}

#[tokio::test]
async fn brew_bottle_selection_and_sha256_matches_oracle() {
    let formula = serde_json::json!({
        "desc": "Internet download manager",
        "versions": {"stable": "1.24.0"},
        "bottle": {"stable": {"files": {
            "arm64_sonoma": {"url": "https://ghcr.io/v2/homebrew/core/wget/blobs/sha256:sonoma", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
            "arm64_ventura": {"url": "https://ghcr.io/v2/homebrew/core/wget/blobs/sha256:ventura", "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}
        }}},
        "dependencies": ["openssl@3"]
    });
    let api = "https://formulae.brew.sh/api/formula/wget.json";
    let token_url = "https://ghcr.io/token?service=ghcr.io&scope=repository:homebrew/core/wget:pull";
    let blob = "https://ghcr.io/v2/homebrew/core/wget/blobs/sha256:sonoma";
    let cdn = "https://pkg-containers.githubusercontent.com/ghcr1/blobs/sha256:sonoma?sig=abc";
    let http = Scripted::new()
        .json(api, 200, formula)
        .json(token_url, 200, serde_json::json!({"token": "pull-token"}))
        .redirect(blob, Some((cdn, Some(42_000))));
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = BrewResolver::new(http, secrets).with_host("darwin", "arm64", 15);

    let assets = resolver.resolve("brew:wget", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].url, cdn);
    assert_eq!(assets[0].filename.as_deref(), Some("wget-1.24.0.arm64_sonoma.bottle.tar.gz"));
    assert_eq!(assets[0].content_length, Some(42_000));
    assert_eq!(
        assets[0].digest.as_deref(),
        Some("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
    );
    assert_eq!(assets[0].target_type, TargetType::Brew);
}

// ===========================================================================
// PyPI — tests/unit/test_pypi_transitive.py + test_bug_fixes.py H2
// ===========================================================================

#[test]
fn pypi_parse_target_complex_specifiers_match_oracle() {
    let (pkg, ver) = parse_pypi_target("pip:llvmlite<0.51,>=0.50.0dev0");
    assert_eq!(pkg, "llvmlite");
    assert_eq!(ver, None);

    let (pkg, ver) = parse_pypi_target("pip:mpmath>=1.1.0,<1.4");
    assert_eq!(pkg, "mpmath");
    assert_eq!(ver, None);

    let (pkg, ver) = parse_pypi_target("pip:requests==2.32.3");
    assert_eq!(pkg, "requests");
    assert_eq!(ver, Some("2.32.3".to_owned()));

    let (pkg, ver) = parse_pypi_target("pypi:numpy@1.26.4");
    assert_eq!(pkg, "numpy");
    assert_eq!(ver, Some("1.26.4".to_owned()));

    let (pkg, ver) = parse_pypi_target("https://pypi.org/project/urllib3/2.2.0/");
    assert_eq!(pkg, "urllib3");
    assert_eq!(ver, Some("2.2.0".to_owned()));
}

#[test]
fn pypi_wheel_scoring_matches_oracle_abi_and_platform_tags() {
    let linux = WheelTarget::new((3, 12), "linux", "x86_64");
    assert_eq!(score_wheel("numpy-1.26.4-cp312-cp312-manylinux_2_17_x86_64.whl", "bdist_wheel", &linux), 100);
    assert_eq!(score_wheel("numpy-1.26.4-cp311-cp311-manylinux_2_17_x86_64.whl", "bdist_wheel", &linux), 40);
    assert_eq!(score_wheel("numpy-1.26.4-abi3-manylinux_2_17_x86_64.whl", "bdist_wheel", &linux), 90);
    assert_eq!(score_wheel("numpy-1.26.4-py3-none-any.whl", "bdist_wheel", &linux), 70);
    assert_eq!(score_wheel("numpy-1.26.4-py2.py3-none-any.whl", "bdist_wheel", &linux), 70);
    assert_eq!(score_wheel("numpy-1.26.4.tar.gz", "sdist", &linux), 10);
    assert_eq!(score_wheel("numpy-1.26.4-cp312-cp312-win_amd64.whl", "bdist_wheel", &linux), -1);
    assert_eq!(score_wheel("numpy-1.26.4-cp312-cp312-macosx_11_0_arm64.whl", "bdist_wheel", &linux), -1);

    let mac = WheelTarget::new((3, 12), "darwin", "arm64");
    assert_eq!(score_wheel("pkg-1.0-cp312-cp312-macosx_11_0_arm64.whl", "bdist_wheel", &mac), 100);
    assert_eq!(score_wheel("pkg-1.0-cp312-cp312-macosx_10_9_x86_64.whl", "bdist_wheel", &mac), -1);
    assert_eq!(score_wheel("pkg-1.0-cp312-cp312-macosx_11_0_universal2.whl", "bdist_wheel", &mac), 95);
}

#[tokio::test]
async fn pypi_wheel_selection_order_and_digest_match_oracle() {
    let index = serde_json::json!({
        "info": {"name": "numpy", "version": "1.26.4", "summary": "s", "requires_dist": []},
        "urls": [
            {"filename": "numpy-1.26.4.tar.gz", "packagetype": "sdist", "url": "https://files.invalid/numpy.tar.gz", "size": 1, "digests": {"sha256": "aaaa"}},
            {"filename": "numpy-1.26.4-cp311-cp311-manylinux_2_17_x86_64.whl", "packagetype": "bdist_wheel", "url": "https://files.invalid/cp311.whl", "size": 2, "digests": {"sha256": "bbbb"}},
            {"filename": "numpy-1.26.4-abi3-manylinux_2_17_x86_64.whl", "packagetype": "bdist_wheel", "url": "https://files.invalid/abi3.whl", "size": 3, "digests": {"sha256": "cccc"}},
            {"filename": "numpy-1.26.4-cp312-cp312-manylinux_2_17_x86_64.whl", "packagetype": "bdist_wheel", "url": "https://files.invalid/cp312-first.whl", "size": 4, "digests": {"sha256": "dddd"}},
            {"filename": "numpy-1.26.4-cp312-cp312-manylinux_2_17_x86_64.whl", "packagetype": "bdist_wheel", "url": "https://files.invalid/cp312-second.whl", "size": 5, "digests": {"sha256": "eeee"}}
        ]
    });
    let api = "https://pypi.org/pypi/numpy/json";
    let http = Scripted::new().json(api, 200, index);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = PyPIResolver::new(http, secrets)
        .with_target(WheelTarget::new((3, 12), "linux", "x86_64"));

    let assets = resolver.resolve("pip:numpy", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].url, "https://files.invalid/cp312-first.whl");
    assert_eq!(assets[0].filename.as_deref(), Some("numpy-1.26.4-cp312-cp312-manylinux_2_17_x86_64.whl"));
    assert_eq!(assets[0].content_length, Some(4));
    assert_eq!(assets[0].digest.as_deref(), Some("sha256:dddd"));
    assert_eq!(assets[0].target_type, TargetType::Pypi);
}

#[tokio::test]
async fn pypi_pinned_version_404_does_not_fall_back_to_latest() {
    let latest = serde_json::json!({
        "info": {"name": "nonexistent-pkg", "version": "1.0.0", "summary": "", "requires_dist": []},
        "urls": [{"filename": "nonexistent_pkg-1.0.0-py3-none-any.whl", "packagetype": "bdist_wheel", "url": "https://files.invalid/nonexistent_pkg-1.0.0-py3-none-any.whl", "size": 1, "digests": {"sha256": "aaaa"}}]
    });
    let http = Scripted::new()
        .json("https://pypi.org/pypi/nonexistent-pkg/9.9.9/json", 404, serde_json::Value::Null)
        .json("https://pypi.org/pypi/nonexistent-pkg/json", 200, latest);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = PyPIResolver::new(http, secrets);

    let err = resolver.resolve("pip:nonexistent-pkg==9.9.9", &context()).await.unwrap_err();
    assert_eq!(err.to_string(), "Failed to resolve PyPI package: pip:nonexistent-pkg==9.9.9");
}

#[test]
fn pypi_wheel_dependency_markers_are_filtered_like_oracle() {
    let dir = std::env::temp_dir().join(format!("paradl-w2-3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let wheel = dir.join("test_pkg-1.0.0-py3-none-any.whl");
    let metadata_content = concat!(
        "Metadata-Version: 2.1\n",
        "Name: test-pkg\n",
        "Version: 1.0.0\n",
        "Requires-Dist: required-core>=1.0\n",
        "Requires-Dist: linux-only>=2.0; platform_system == 'NonExistentOS'\n",
        "Requires-Dist: optional-feature; extra == 'feature'\n",
        "Requires-Dist: active-dep\n",
    );
    {
        let file = std::fs::File::create(&wheel).unwrap();
        let mut zf = zip::ZipWriter::new(file);
        zf.start_file(
            "test_pkg-1.0.0.dist-info/METADATA",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zf.write_all(metadata_content.as_bytes()).unwrap();
        zf.finish().unwrap();
    }
    let deps = extract_wheel_dependencies(&wheel).unwrap();
    assert_eq!(deps, vec!["required-core>=1.0".to_owned(), "active-dep".to_owned()]);
    std::fs::remove_dir_all(&dir).ok();
}

// ===========================================================================
// IPSW — engines/ipsw.py
// ===========================================================================

#[test]
fn ipsw_detection_identifiers_and_filename_parsing() {
    assert!(is_ipsw("ipsw:iphone 16"));
    assert!(is_ipsw("https://updates.cdn-apple.com/a/iPhone17,3_18.0_22A5297f_Restore.ipsw"));
    assert!(!is_ipsw("https://example.com/file.zip"));

    assert_eq!(resolve_device_identifier("iphone 16"), "iPhone17,3");
    assert_eq!(resolve_device_identifier("iphone 15 pro max"), "iPhone16,2");
    assert_eq!(resolve_device_identifier("ipsw:iPhone17,3"), "iPhone17,3");

    let meta = parse_ipsw_filename("iPhone17,3_18.0_22A5297f_Restore.ipsw").unwrap();
    assert_eq!(meta.identifier, "iPhone17,3");
    assert_eq!(meta.version, "18.0");
    assert_eq!(meta.build, "22A5297f");
}

#[tokio::test]
async fn ipsw_offline_synthetic_metadata_matches_oracle() {
    let http = Scripted::new();
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = IpswResolver::new(http, secrets).with_offline(true);
    let assets = resolver.resolve("ipsw:iphone 16", &context()).await.unwrap();
    assert_eq!(assets[0].url, "https://api.ipsw.me/v4/ipsw/download/iPhone17,3/latest");
    assert_eq!(assets[0].filename.as_deref(), Some("iPhone17,3_Restore.ipsw"));
    assert_eq!(assets[0].target_type, TargetType::IpSw);
}

#[tokio::test]
async fn ipsw_signed_firmware_selection_and_sha256_matches_oracle() {
    let data = serde_json::json!({
        "name": "iPhone 16",
        "firmwares": [
            {"signed": false, "url": "https://updates.cdn-apple.com/old.ipsw", "version": "17.0", "buildid": "old", "sha256sum": "0000", "filesize": 1},
            {"signed": true, "url": "https://updates.cdn-apple.com/iPhone17,3_18.0_22A5297f_Restore.ipsw", "version": "18.0", "buildid": "22A5297f", "sha256sum": "1111", "filesize": 12345}
        ]
    });
    let api = "https://api.ipsw.me/v4/device/iPhone17,3?type=ipsw";
    let http = Scripted::new().json(api, 200, data);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = IpswResolver::new(http, secrets);
    let assets = resolver.resolve("ipsw:iphone 16", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].url, "https://updates.cdn-apple.com/iPhone17,3_18.0_22A5297f_Restore.ipsw");
    assert_eq!(assets[0].filename.as_deref(), Some("iPhone17,3_18.0_22A5297f_Restore.ipsw"));
    assert_eq!(assets[0].content_length, Some(12345));
    assert_eq!(assets[0].digest.as_deref(), Some("sha256:1111"));
}

// ===========================================================================
// Cloud — engines/cloud.py (pure URL rewrite)
// ===========================================================================

#[test]
fn cloud_url_rewrites_are_byte_exact() {
    let (url, name) = resolve_cloud_url("https://drive.google.com/file/d/1AbC-_xY/view?usp=sharing");
    assert_eq!(url, "https://drive.usercontent.google.com/download?id=1AbC-_xY&export=download&confirm=t");
    assert_eq!(name, None);

    let (url, _) = resolve_cloud_url("https://drive.google.com/open?id=123abc");
    assert_eq!(url, "https://drive.usercontent.google.com/download?id=123abc&export=download&confirm=t");

    let (url, _) = resolve_cloud_url("https://www.dropbox.com/s/abc/file.zip?dl=0");
    assert_eq!(url, "https://www.dropbox.com/s/abc/file.zip?dl=1");

    let (url, _) = resolve_cloud_url("https://www.dropbox.com/s/abc/file.zip?dl=1");
    assert_eq!(url, "https://www.dropbox.com/s/abc/file.zip?dl=1");

    let (url, _) = resolve_cloud_url("https://www.dropbox.com/s/abc/file.zip?foo=1");
    assert_eq!(url, "https://www.dropbox.com/s/abc/file.zip?foo=1&dl=1");

    let (url, _) = resolve_cloud_url("https://www.dropbox.com/s/abc/file.zip");
    assert_eq!(url, "https://www.dropbox.com/s/abc/file.zip?dl=1");

    let (url, name) = resolve_cloud_url("https://pastebin.com/AbC123");
    assert_eq!(url, "https://pastebin.com/raw/AbC123");
    assert_eq!(name.as_deref(), Some("AbC123.txt"));

    let (url, _) = resolve_cloud_url("https://pastebin.com/raw/AbC123");
    assert_eq!(url, "https://pastebin.com/raw/AbC123");

    let (url, name) = resolve_cloud_url("https://example.com/file.zip");
    assert_eq!(url, "https://example.com/file.zip");
    assert_eq!(name, None);
}

#[tokio::test]
async fn cloud_resolver_returns_a_direct_asset() {
    let resolver = CloudResolver::new();
    let assets = resolver
        .resolve("https://pastebin.com/AbC123", &context())
        .await
        .unwrap();
    assert_eq!(assets[0].url, "https://pastebin.com/raw/AbC123");
    assert_eq!(assets[0].filename.as_deref(), Some("AbC123.txt"));
    assert_eq!(assets[0].target_type, TargetType::Direct);
}

// ===========================================================================
// Token flows — env precedence, keychain abstraction, no secret in argv
// ===========================================================================

#[test]
fn github_token_env_precedence_and_headers() {
    let env = HashMap::from([
        ("GITHUB_TOKEN".to_owned(), "github-env".to_owned()),
        ("GH_TOKEN".to_owned(), "gh-env".to_owned()),
    ]);
    let secrets = EnvSecretProvider::with_env(env, Some(Arc::new(TestKeychain { token: "kc".into() })));
    assert_eq!(secrets.token(SiteService::Github).as_deref(), Some("github-env"));
    let headers = SiteHeaders::rest(SiteService::Github, &secrets);
    assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer github-env"));
    assert_eq!(
        headers.get("Accept").map(String::as_str),
        Some("application/vnd.github.v3+json"),
    );
    assert_eq!(
        headers.get("User-Agent").map(String::as_str),
        Some("pdl-accelerator/1.0.0"),
    );
    assert_eq!(secrets.registered_secrets(), vec!["github-env".to_owned()]);
}

#[test]
fn hf_token_env_precedence_and_no_argv_secret() {
    let env = HashMap::from([
        ("HF_TOKEN".to_owned(), "hf-env".to_owned()),
        ("HUGGING_FACE_HUB_TOKEN".to_owned(), "hub-env".to_owned()),
    ]);
    let secrets = EnvSecretProvider::with_env(env, None::<Arc<dyn Keychain>>);
    assert_eq!(secrets.token(SiteService::Hf).as_deref(), Some("hf-env"));
    let headers = SiteHeaders::rest(SiteService::Hf, &secrets);
    assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer hf-env"));
    assert_eq!(
        headers.get("User-Agent").map(String::as_str),
        Some("pdl-hf-accelerator/1.0.0"),
    );
}

#[test]
fn keychain_fallback_uses_pdl_service_and_site_accounts() {
    struct RecordingKeychain(std::sync::Mutex<Vec<(String, String)>>);
    impl Keychain for RecordingKeychain {
        fn load_secret(&self, service: &str, account: &str) -> Option<String> {
            self.0.lock().unwrap().push((service.to_owned(), account.to_owned()));
            Some(format!("{service}:{account}"))
        }
    }
    let kc = Arc::new(RecordingKeychain(std::sync::Mutex::new(Vec::new())));
    let secrets = EnvSecretProvider::with_env(HashMap::new(), Some(kc.clone()));
    assert_eq!(secrets.token(SiteService::Github).as_deref(), Some("pdl:github"));
    assert_eq!(secrets.token(SiteService::Hf).as_deref(), Some("pdl:hf"));
    assert_eq!(
        kc.0.lock().unwrap().clone(),
        vec![
            ("pdl".to_owned(), "github".to_owned()),
            ("pdl".to_owned(), "hf".to_owned()),
        ],
    );
}

// ===========================================================================
// Router dispatch (entries are the shortcut prefixes from parity row C51)
// ===========================================================================

#[test]
fn site_router_recognizes_all_shortcut_prefixes() {
    let client = paradl_net::HttpClient::new(Default::default()).unwrap();
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let router = SiteRouter::new(client, secrets);
    assert_eq!(router.kind_of("gh:owner/repo"), Some("github"));
    assert_eq!(router.kind_of("hf:owner/repo"), Some("huggingface"));
    assert_eq!(router.kind_of("pip:requests"), Some("pypi"));
    assert_eq!(router.kind_of("brew:wget"), Some("brew"));
    assert_eq!(router.kind_of("ipsw:iphone 16"), Some("ipsw"));
    assert_eq!(router.kind_of("https://pastebin.com/AbC123"), Some("cloud"));
    assert_eq!(router.kind_of("https://example.com/file.zip"), None);
}

// ===========================================================================
// Review round 1 — specifier resolution (values captured from PyPIEngine.resolve)
// ===========================================================================

/// Load the shared fixture and run PyPI resolution through the scripted REST
/// seam, mirroring the Python oracle's `fetch_json_with_etag` monkeypatch.
async fn resolve_pypi_fixture_case(name: &str) -> ResolvedAsset {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pypi_specifiers.json")).unwrap();
    let case = fixture
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("fixture case {name} missing"));
    let target = case["target"].as_str().unwrap().to_owned();
    let api_url = case["url"].as_str().unwrap().to_owned();
    let body = case["body"].clone();
    let http = Scripted::new().json(&api_url, 200, body);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    PyPIResolver::new(http, secrets)
        .with_target(WheelTarget::new((3, 12), "linux", "x86_64"))
        .resolve(&target, &context())
        .await
        .unwrap()
        .remove(0)
}

#[tokio::test]
async fn pypi_llvmlite_complex_specifier_resolution_matches_oracle() {
    let asset = resolve_pypi_fixture_case("llvmlite").await;
    assert_eq!(asset.url, "https://files.invalid/llvmlite-0.50.1-py3-none-any.whl");
    assert_eq!(asset.filename.as_deref(), Some("llvmlite-0.50.1-py3-none-any.whl"));
    assert_eq!(asset.content_length, Some(501));
    assert_eq!(asset.digest.as_deref(), Some("sha256:00501"));
}

#[tokio::test]
async fn pypi_mpmath_complex_specifier_resolution_matches_oracle() {
    let asset = resolve_pypi_fixture_case("mpmath").await;
    assert_eq!(asset.url, "https://files.invalid/mpmath-1.3.0.post1-py3-none-any.whl");
    assert_eq!(asset.filename.as_deref(), Some("mpmath-1.3.0.post1-py3-none-any.whl"));
    assert_eq!(asset.content_length, Some(131));
    assert_eq!(asset.digest.as_deref(), Some("sha256:0130post1"));
}

#[tokio::test]
async fn pypi_compatible_release_rejects_non_matching_higher_major() {
    let asset = resolve_pypi_fixture_case("compatible").await;
    assert_eq!(asset.url, "https://files.invalid/pkg-1.9.9-py3-none-any.whl");
    assert!(!asset.url.contains("2.0.0"), "~=1.4 must not accept pkg 2.0.0");
    assert_eq!(asset.digest.as_deref(), Some("sha256:0199"));
}

#[tokio::test]
async fn pypi_satisfying_info_version_short_circuits_release_search() {
    let asset = resolve_pypi_fixture_case("shortcircuit").await;
    assert_eq!(asset.url, "https://files.invalid/short-1.4.0-BASE.whl");
    assert_eq!(asset.digest.as_deref(), Some("sha256:0140base"));
}

// ===========================================================================
// Review round 1 — fnmatch character classes (values from Python fnmatchcase)
// ===========================================================================

#[test]
fn fnmatch_character_classes_match_python() {
    assert!(glob_match("tool-aarch64.tar.gz", "tool-[ab]arch64*.tar.gz"));
    assert!(!glob_match("tool-carch64.tar.gz", "tool-[ab]arch64*.tar.gz"));
    assert!(glob_match("tool-carch64.tar.gz", "tool-[!ab]arch64*.tar.gz"));
    assert!(!glob_match("tool-aarch64.tar.gz", "tool-[!ab]arch64*.tar.gz"));
    assert!(glob_match("v1.zip", "v[0-9].zip"));
    assert!(!glob_match("vx.zip", "v[0-9].zip"));
}

#[tokio::test]
async fn github_asset_pattern_character_class_matches_oracle_fnmatch() {
    let release = serde_json::json!({
        "tag_name": "v1.0.0",
        "assets": [
            {"name": "tool-aarch64-macos.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-aarch64.tar.gz", "size": 10},
            {"name": "tool-carch64-macos.tar.gz", "browser_download_url": "https://github.com/o/r/releases/download/v1.0.0/tool-carch64.tar.gz", "size": 20}
        ]
    });
    let api = "https://api.github.com/repos/o/r/releases/latest";
    let http = Scripted::new().json(api, 200, release);
    let secrets = Arc::new(EnvSecretProvider::with_env(HashMap::new(), None::<Arc<dyn Keychain>>));
    let resolver = GithubResolver::new(http, secrets).with_asset_pattern("tool-[!ab]arch64*.tar.gz");
    let assets = resolver.resolve("gh:o/r", &context()).await.unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].url, "https://github.com/o/r/releases/download/v1.0.0/tool-carch64.tar.gz");
}
