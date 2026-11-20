//! GitHub site engine: release assets, trees, archives and source shortcuts.
//!
//! Ports the pure resolution half of `src/paradl/engines/github.py`:
//! `parse_github_target`, `score_asset_for_host`, `fetch_release_info` and
//! asset digest extraction. Downloading, checksum verification and `install`
//! remain in the engine/post-processing layers.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use paradl_core::TargetType;
use regex::Regex;
use serde_json::Value;
use tracing::warn;

use super::accelerate::{accelerate_url, accelerate_url_with_mirror};
use super::token::{SecretProvider, SiteHeaders, SiteService};
use super::{JsonResponse, SiteHttp};
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::{Error, Result};

static ASSET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/]+)/([^/]+)/releases/download/([^/]+)/(.+)")
        .expect("asset regex must compile")
});
static RELEASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"github\.com/([^/]+)/([^/]+)/releases(?:/tag/([^/?#]+)|/latest)?(?:/)?(?:\?[^#]*)?(?:#.*)?$",
    )
    .expect("release regex must compile")
});
static BLOB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/]+)/([^/]+)/blob/([^/]+)/(.+)").expect("blob regex must compile")
});
static RAW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.+)")
        .expect("raw regex must compile")
});
static TREE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/]+)/([^/]+)/tree/([^/]+)/(.+)").expect("tree regex must compile")
});
static GIST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"gist\.github\.com/([^/]+)/([a-f0-9]+)").expect("gist regex must compile")
});
static REPO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"github\.com/([^/]+)/([^/?#]+?)(?:\.git)?(?:/)?(?:\?[^#]*)?(?:#.*)?$")
        .expect("repo regex must compile")
});

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

/// A classified GitHub target, mirroring the dict returned by Python's
/// `GitHubEngine.parse_github_target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubTarget {
    /// A direct release asset URL.
    Asset {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
        /// Release tag.
        tag: String,
        /// Asset filename (query string removed).
        filename: String,
        /// Original URL.
        url: String,
    },
    /// A `/releases` or `/releases/tag/<tag>` page.
    Release {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
        /// Release tag (`latest` when omitted).
        tag: String,
        /// Original URL.
        url: String,
    },
    /// A `/blob/` path or `raw.githubusercontent.com` URL.
    Blob {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
        /// Branch/ref.
        branch: String,
        /// Path inside the repository.
        path: String,
        /// Raw download URL.
        url: String,
        /// Filename.
        filename: String,
    },
    /// A `/tree/<branch>/<path>` directory.
    Tree {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
        /// Branch/ref.
        branch: String,
        /// Directory path.
        path: String,
        /// Original URL.
        url: String,
    },
    /// A repository (shortcut or plain URL).
    Repo {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
        /// Canonical GitHub URL.
        url: String,
    },
    /// A Gist URL.
    Gist {
        /// Gist user.
        user: String,
        /// Gist id.
        gist_id: String,
        /// Original URL.
        url: String,
    },
    /// Anything else (downloaded directly).
    Generic {
        /// Original URL.
        url: String,
    },
}

impl GithubTarget {
    /// Classify `target` into one of the GitHub target variants.
    pub fn parse(target: &str) -> Self {
        let t = target.trim();
        let lower = t.to_ascii_lowercase();

        if let Some(rest) = lower
            .strip_prefix("gh:")
            .or_else(|| lower.strip_prefix("github:"))
        {
            let parts: Vec<&str> = rest.trim().split('/').filter(|p| !p.is_empty()).collect();
            if parts.len() >= 2 {
                return Self::Repo {
                    owner: parts[0].to_owned(),
                    repo: parts[1].to_owned(),
                    url: format!("https://github.com/{}/{}", parts[0], parts[1]),
                };
            }
        }

        if let Some(caps) = GIST_RE.captures(t) {
            return Self::Gist {
                user: caps[1].to_owned(),
                gist_id: caps[2].to_owned(),
                url: t.to_owned(),
            };
        }
        if let Some(caps) = ASSET_RE.captures(t) {
            let filename = caps[4].split('?').next().unwrap_or(&caps[4]).to_owned();
            return Self::Asset {
                owner: caps[1].to_owned(),
                repo: caps[2].to_owned(),
                tag: caps[3].to_owned(),
                filename,
                url: t.to_owned(),
            };
        }
        if let Some(caps) = RELEASE_RE.captures(t) {
            return Self::Release {
                owner: caps[1].to_owned(),
                repo: caps[2].to_owned(),
                tag: caps.get(3).map_or_else(|| "latest".to_owned(), |m| m.as_str().to_owned()),
                url: t.to_owned(),
            };
        }
        if let Some(caps) = BLOB_RE.captures(t) {
            let path = caps[4].to_owned();
            return Self::Blob {
                owner: caps[1].to_owned(),
                repo: caps[2].to_owned(),
                branch: caps[3].to_owned(),
                url: format!(
                    "https://raw.githubusercontent.com/{}/{}/{}/{}",
                    &caps[1], &caps[2], &caps[3], path
                ),
                filename: basename(path.split('?').next().unwrap_or(&path)),
                path,
            };
        }
        if let Some(caps) = RAW_RE.captures(t) {
            let path = caps[4].to_owned();
            return Self::Blob {
                owner: caps[1].to_owned(),
                repo: caps[2].to_owned(),
                branch: caps[3].to_owned(),
                url: t.to_owned(),
                filename: basename(path.split('?').next().unwrap_or(&path)),
                path,
            };
        }
        if let Some(caps) = TREE_RE.captures(t) {
            return Self::Tree {
                owner: caps[1].to_owned(),
                repo: caps[2].to_owned(),
                branch: caps[3].to_owned(),
                path: caps[4].to_owned(),
                url: t.to_owned(),
            };
        }
        if let Some(caps) = REPO_RE.captures(t) {
            let repo = caps[2].strip_suffix(".git").unwrap_or(&caps[2]).to_owned();
            return Self::Repo {
                owner: caps[1].to_owned(),
                url: format!("https://github.com/{}/{}", &caps[1], repo),
                repo,
            };
        }
        Self::Generic { url: t.to_owned() }
    }

    /// Python-style type string (`asset`, `release`, `blob`, ...).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Asset { .. } => "asset",
            Self::Release { .. } => "release",
            Self::Blob { .. } => "blob",
            Self::Tree { .. } => "tree",
            Self::Repo { .. } => "repo",
            Self::Gist { .. } => "gist",
            Self::Generic { .. } => "generic",
        }
    }

    /// Repository owner, when this target has one.
    pub fn owner(&self) -> Option<&str> {
        match self {
            Self::Asset { owner, .. }
            | Self::Release { owner, .. }
            | Self::Blob { owner, .. }
            | Self::Tree { owner, .. }
            | Self::Repo { owner, .. } => Some(owner),
            _ => None,
        }
    }

    /// Repository name, when this target has one.
    pub fn repo(&self) -> Option<&str> {
        match self {
            Self::Asset { repo, .. }
            | Self::Release { repo, .. }
            | Self::Blob { repo, .. }
            | Self::Tree { repo, .. }
            | Self::Repo { repo, .. } => Some(repo),
            _ => None,
        }
    }

    /// Release tag, when this target has one.
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::Asset { tag, .. } | Self::Release { tag, .. } => Some(tag),
            _ => None,
        }
    }

    /// Asset filename, when this target has one.
    pub fn filename(&self) -> Option<&str> {
        match self {
            Self::Asset { filename, .. } | Self::Blob { filename, .. } => Some(filename),
            _ => None,
        }
    }

    /// Directory/path, when this target has one.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Blob { path, .. } | Self::Tree { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Gist user, when this target is a gist.
    pub fn user(&self) -> Option<&str> {
        match self {
            Self::Gist { user, .. } => Some(user),
            _ => None,
        }
    }

    /// Gist id, when this target is a gist.
    pub fn gist_id(&self) -> Option<&str> {
        match self {
            Self::Gist { gist_id, .. } => Some(gist_id),
            _ => None,
        }
    }

    /// The canonical or raw URL for this target.
    pub fn url(&self) -> &str {
        match self {
            Self::Asset { url, .. }
            | Self::Release { url, .. }
            | Self::Blob { url, .. }
            | Self::Tree { url, .. }
            | Self::Repo { url, .. }
            | Self::Gist { url, .. }
            | Self::Generic { url } => url,
        }
    }
}

/// Score a release asset filename for the current host (byte-exact port of
/// `GitHubEngine.score_asset_for_host`).
///
/// `platform` is the Python `sys.platform` string (`darwin`/`windows`/`linux`)
/// and `is_arm` selects the ARM64 branch. Use
/// [`score_asset_for_current_host`] for automatic host detection.
pub fn score_asset_for_host(
    filename: &str,
    install_mode: bool,
    platform: &str,
    is_arm: bool,
) -> i32 {
    let name = filename.to_ascii_lowercase();
    if [
        ".sha256", ".sha256sum", ".sha512", ".md5", ".sig", ".asc", ".sbom", ".txt",
    ]
    .iter()
    .any(|ext| name.ends_with(ext))
    {
        return 0;
    }

    let has_any = |needles: &[&str]| needles.iter().any(|needle| name.contains(needle));
    let mut score = 0;

    if platform == "windows" {
        let is_win = has_any(&[
            "windows", "win64", "win32", "pc-windows", "msvc", ".exe", ".msi",
        ]);
        if is_win {
            score += 40;
            if name.ends_with(".zip") {
                score += 25;
            } else if name.ends_with(".exe") {
                score += if install_mode { 15 } else { 20 };
            } else if name.ends_with(".msi") {
                score += if install_mode { 0 } else { 15 };
            } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
                score += 10;
            }
            if is_arm {
                if has_any(&["arm64", "aarch64"]) {
                    score += 35;
                } else if has_any(&["x86_64", "x64", "amd64", "win64"]) {
                    score += 5;
                }
            } else if has_any(&["x86_64", "x64", "amd64", "win64", "msvc"]) {
                score += 35;
            } else if has_any(&["i686", "x86", "win32"]) {
                score += 15;
            }
        } else if has_any(&["darwin", "macos", "apple", "linux", ".deb", ".rpm", ".dmg", ".pkg"])
        {
            score = 0;
        } else if name.ends_with(".zip") {
            score += 10;
        }
        return score;
    }

    let is_mac = has_any(&["darwin", "macos", "apple", "osx", ".dmg", ".pkg"]);
    if is_mac {
        score += 40;
        if name.ends_with(".dmg") {
            score += 25;
        } else if name.ends_with(".pkg") {
            score += 20;
        } else if [".tar.gz", ".tgz", ".zip", ".tar.xz"]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            score += 15;
        }
        if is_arm {
            if has_any(&["arm64", "aarch64", "apple-darwin"]) {
                score += 35;
            } else if name.contains("universal") {
                score += 30;
            } else if has_any(&["x86_64", "x64", "amd64"]) {
                score += 5;
            }
        } else if has_any(&["x86_64", "x64", "amd64"]) {
            score += 35;
        } else if name.contains("universal") {
            score += 30;
        }
    } else if has_any(&[
        "linux", "windows", "win32", "win64", ".exe", ".msi", ".deb", ".rpm", ".apk", "msvc",
    ]) {
        score = 0;
    } else if [".zip", ".tar.gz", ".tgz"]
        .iter()
        .any(|ext| name.ends_with(ext))
    {
        score += 5;
    }
    score
}

/// Score using the current process's OS/architecture.
pub fn score_asset_for_current_host(filename: &str, install_mode: bool) -> i32 {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "windows",
        other => other,
    };
    let is_arm = matches!(std::env::consts::ARCH, "aarch64" | "arm");
    score_asset_for_host(filename, install_mode, platform, is_arm)
}

/// Extract the `sha256:...` digest from a GitHub release asset object.
pub fn extract_asset_digest(asset: &Value) -> Option<String> {
    asset
        .get("digest")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn check_rate_limit(headers: &HashMap<String, String>, status: u16) {
    if status == 403 || status == 429 {
        let remaining = headers.get("x-ratelimit-remaining").map(String::as_str);
        let reset = headers.get("x-ratelimit-reset").map(String::as_str);
        if remaining == Some("0") || status == 429 {
            let suffix = reset.map_or_else(String::new, |ts| format!(" (resets at epoch {ts})"));
            warn!(
                "GitHub API rate limit exceeded{suffix}. Set GITHUB_TOKEN or sign in with 'gh auth login' for higher limits."
            );
        }
    }
}

/// [`TargetResolver`] for GitHub repos, releases, assets and trees.
#[derive(Clone)]
pub struct GithubResolver {
    http: Arc<dyn SiteHttp>,
    secrets: Arc<dyn SecretProvider>,
    pattern: Option<String>,
    download_all: bool,
    source: bool,
    branch: Option<String>,
    platform: &'static str,
    is_arm: bool,
    archive_mirror: Option<String>,
}

impl GithubResolver {
    /// Build a resolver with production HTTP I/O.
    pub fn new<H: SiteHttp + 'static>(http: H, secrets: Arc<dyn SecretProvider>) -> Self {
        Self::from_shared(Arc::new(http), secrets)
    }

    /// Build a resolver sharing a [`SiteHttp`] instance.
    pub fn from_shared(http: Arc<dyn SiteHttp>, secrets: Arc<dyn SecretProvider>) -> Self {
        Self {
            http,
            secrets,
            pattern: None,
            download_all: false,
            source: false,
            branch: None,
            platform: match std::env::consts::OS {
                "macos" => "darwin",
                "windows" => "windows",
                other => other,
            },
            is_arm: matches!(std::env::consts::ARCH, "aarch64" | "arm"),
            archive_mirror: None,
        }
    }

    /// Restrict release selection to assets matching a pattern (Python `--asset`).
    pub fn with_asset_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Resolve every release asset instead of picking one (Python `--all-assets`).
    pub fn with_download_all(mut self, enabled: bool) -> Self {
        self.download_all = enabled;
        self
    }

    /// Skip release assets and download the source archive (Python `--source`).
    pub fn with_source(mut self, enabled: bool) -> Self {
        self.source = enabled;
        self
    }

    /// Override the default branch used for a repo archive (Python `--branch`).
    pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }

    /// Override the host used for asset scoring (deterministic tests).
    pub fn with_scoring_host(mut self, platform: &'static str, is_arm: bool) -> Self {
        self.platform = platform;
        self.is_arm = is_arm;
        self
    }

    /// Override the archive mirror prefix (`None` reads the env var at resolve time).
    pub fn with_archive_mirror(mut self, mirror: Option<String>) -> Self {
        self.archive_mirror = mirror;
        self
    }

    async fn fetch(
        &self,
        url: &str,
        ctx: &ResolveContext,
    ) -> Result<JsonResponse> {
        let headers = SiteHeaders::rest(SiteService::Github, self.secrets.as_ref());
        let response = self.http.fetch_json(url, &headers, ctx.timeout).await?;
        check_rate_limit(&response.headers, response.status);
        Ok(response)
    }

    fn asset(&self, value: &Value, headers: &HashMap<String, String>) -> ResolvedAsset {
        ResolvedAsset {
            url: value
                .get("browser_download_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            filename: value
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            content_length: value.get("size").and_then(Value::as_u64),
            digest: extract_asset_digest(value),
            target_type: TargetType::Github,
            headers: headers.clone(),
            media_op: None,
        }
    }

    fn select_assets(&self, assets: &[Value]) -> Vec<ResolvedAsset> {
        let headers = SiteHeaders::download(SiteService::Github, self.secrets.as_ref());
        let candidates: Vec<&Value> = match &self.pattern {
            Some(pattern) => assets
                .iter()
                .filter(|asset| {
                    asset
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| super::matches_pattern(name, pattern))
                })
                .collect(),
            None => assets.iter().collect(),
        };
        let candidates = if candidates.is_empty() {
            // Python shows all assets when a pattern matches nothing.
            assets.iter().collect::<Vec<_>>()
        } else {
            candidates
        };

        if self.download_all {
            return candidates
                .iter()
                .map(|asset| self.asset(asset, &headers))
                .collect();
        }

        let mut best: Option<&Value> = None;
        let mut best_score = i32::MIN;
        for asset in &candidates {
            let name = asset.get("name").and_then(Value::as_str).unwrap_or_default();
            let score = score_asset_for_host(name, false, self.platform, self.is_arm);
            if score > best_score {
                best_score = score;
                best = Some(asset);
            }
        }
        best.into_iter()
            .map(|asset| self.asset(asset, &headers))
            .collect()
    }

    fn archive_asset(&self, owner: &str, repo: &str, branch: &str) -> ResolvedAsset {
        let canonical = format!("https://github.com/{owner}/{repo}/archive/refs/heads/{branch}.zip");
        let accelerated = match &self.archive_mirror {
            Some(mirror) => accelerate_url_with_mirror(&canonical, Some(mirror)),
            None => accelerate_url(&canonical),
        };
        let mut headers = SiteHeaders::download(SiteService::Github, self.secrets.as_ref());
        headers.insert("x-paradl-canonical-url".to_owned(), canonical.clone());
        ResolvedAsset {
            url: accelerated.unwrap_or(canonical),
            filename: Some(format!("{repo}-{branch}.zip")),
            target_type: TargetType::Github,
            headers,
            ..Default::default()
        }
    }

    async fn default_branch(
        &self,
        owner: &str,
        repo: &str,
        ctx: &ResolveContext,
    ) -> Result<String> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}");
        let response = self.fetch(&url, ctx).await?;
        Ok(response
            .body
            .get("default_branch")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("main")
            .to_owned())
    }

    async fn release_assets(
        &self,
        owner: &str,
        repo: &str,
        tag: &str,
        ctx: &ResolveContext,
    ) -> Result<Option<(Vec<Value>, String)>> {
        let url = if tag == "latest" || tag.is_empty() {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}")
        };
        let response = self.fetch(&url, ctx).await?;
        if response.status != 200 || !response.body.is_object() {
            return Ok(None);
        }
        let assets = response
            .body
            .get("assets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let tag_name = response
            .body
            .get("tag_name")
            .and_then(Value::as_str)
            .unwrap_or(tag)
            .to_owned();
        Ok(Some((assets, tag_name)))
    }

    async fn tree_assets(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        path: &str,
        ctx: &ResolveContext,
    ) -> Result<Vec<ResolvedAsset>> {
        let headers = SiteHeaders::download(SiteService::Github, self.secrets.as_ref());
        let mut items = Vec::new();
        let mut pending = vec![path.to_owned()];
        while let Some(current) = pending.pop() {
            if pending.len() > 64 {
                break;
            }
            let url = format!(
                "https://api.github.com/repos/{owner}/{repo}/contents/{current}?ref={branch}"
            );
            let response = self.fetch(&url, ctx).await?;
            let Some(array) = response.body.as_array() else {
                continue;
            };
            for entry in array {
                match entry.get("type").and_then(Value::as_str) {
                    Some("file") => {
                        if let Some(download_url) = entry.get("download_url").and_then(Value::as_str)
                        {
                            items.push(ResolvedAsset {
                                url: download_url.to_owned(),
                                filename: entry
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned),
                                content_length: entry.get("size").and_then(Value::as_u64),
                                target_type: TargetType::Github,
                                headers: headers.clone(),
                                ..Default::default()
                            });
                        }
                    }
                    Some("dir") => {
                        if entry.get("url").and_then(Value::as_str).is_some()
                            && let Some(name) = entry.get("name").and_then(Value::as_str)
                        {
                            pending.push(format!("{current}/{name}"));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(items)
    }
}

#[async_trait]
impl TargetResolver for GithubResolver {
    fn scheme(&self) -> &'static str {
        "gh"
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        match GithubTarget::parse(target) {
            GithubTarget::Repo { owner, repo, .. } => {
                if !self.source
                    && let Some((assets, _tag_name)) =
                        self.release_assets(&owner, &repo, "latest", ctx).await?
                    && !assets.is_empty()
                {
                    return Ok(self.select_assets(&assets));
                }
                let branch = match &self.branch {
                    Some(branch) => branch.clone(),
                    None => self.default_branch(&owner, &repo, ctx).await?,
                };
                Ok(vec![self.archive_asset(&owner, &repo, &branch)])
            }
            GithubTarget::Release { owner, repo, tag, .. } => {
                let Some((assets, tag_name)) = self.release_assets(&owner, &repo, &tag, ctx).await?
                else {
                    return Err(Error::failure(format!(
                        "Release not found for {owner}/{repo}"
                    )));
                };
                if assets.is_empty() {
                    let branch = tag_name.trim_start_matches('v');
                    return Ok(vec![self.archive_asset(&owner, &repo, branch)]);
                }
                Ok(self.select_assets(&assets))
            }
            GithubTarget::Asset { url, filename, .. } | GithubTarget::Blob { url, filename, .. } => {
                Ok(vec![ResolvedAsset {
                    url,
                    filename: Some(filename),
                    target_type: TargetType::Github,
                    headers: SiteHeaders::download(SiteService::Github, self.secrets.as_ref()),
                    ..Default::default()
                }])
            }
            GithubTarget::Tree {
                owner,
                repo,
                branch,
                path,
                ..
            } => self.tree_assets(&owner, &repo, &branch, &path, ctx).await,
            GithubTarget::Gist { url, .. } | GithubTarget::Generic { url } => {
                Ok(vec![ResolvedAsset {
                    url,
                    target_type: TargetType::Github,
                    headers: SiteHeaders::download(SiteService::Github, self.secrets.as_ref()),
                    ..Default::default()
                }])
            }
        }
    }
}
