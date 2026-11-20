//! Homebrew site engine: formula parsing, bottle-tag selection and GHCR redirect.
//!
//! Ports `src/paradl/engines/brew.py` resolution: `parse_brew_target`,
//! `get_fallback_bottle_tags`, the formulae.brew.sh API lookup, and the
//! anonymous GHCR token → signed-CDN redirect flow. Installing/pouring is owned
//! by the CLI and post-processing layers.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use paradl_core::{Error, TargetType};
use regex::Regex;
use serde_json::Value;

use super::token::SecretProvider;
use super::SiteHttp;
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::Result;

static GHCR_REPO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"ghcr\.io/v2/([^/]+/[^/]+(?:/[^/]+)*)/blobs/")
        .expect("GHCR repo regex must compile")
});

fn has_prefix(target: &str, prefixes: &[&str]) -> bool {
    let lower = target.to_ascii_lowercase();
    prefixes.iter().any(|prefix| lower.starts_with(prefix))
}

/// Extract a formula name from a `brew:` shortcut or formulae.brew.sh URL.
pub fn parse_brew_target(target: &str) -> String {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    if has_prefix(t, &["brew:", "brew/", "homebrew:", "homebrew/"]) {
        let rest = t.split_once([':', '/']).map_or(t, |(_, rest)| rest);
        return rest.trim().to_owned();
    }
    if lower.contains("formulae.brew.sh/formula/")
        || lower.contains("formulae.brew.sh/api/formula/")
    {
        let path = url::Url::parse(t).map_or_else(|_| t.to_owned(), |url| url.path().to_owned());
        let segments: Vec<&str> = path
            .split('/')
            .filter(|part| {
                !part.is_empty() && *part != "api" && *part != "formula"
            })
            .collect();
        return segments
            .first()
            .map_or_else(|| t.to_owned(), |part| part.replace(".json", ""));
    }
    t.to_owned()
}

/// Prioritized bottle tags for an OS/architecture pair (port of
/// `BrewEngine.get_fallback_bottle_tags`).
///
/// `os` uses Python's `sys.platform` strings (`darwin`, `linux`, `win32`). The
/// duplicate `x86_64_linux` on Linux is deliberate: the oracle emits it twice.
pub fn fallback_bottle_tags(os: &str, machine: &str, mac_major: u32) -> Vec<String> {
    let mut tags = Vec::new();
    let machine = machine.to_ascii_lowercase();
    let arch = if matches!(machine.as_str(), "arm64" | "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };

    if os == "darwin" {
        let names: &[&str] = match mac_major {
            27 => &["golden_gate", "tahoe", "sequoia"],
            26 => &["tahoe", "sequoia", "sonoma"],
            15 => &["sequoia", "sonoma", "ventura"],
            14 => &["sonoma", "ventura", "monterey"],
            13 => &["ventura", "monterey", "big_sur"],
            12 => &["monterey", "big_sur", "catalina"],
            _ => &["sequoia", "sonoma", "ventura"],
        };
        for name in names {
            if arch == "arm64" {
                tags.push(format!("arm64_{name}"));
            } else {
                tags.push((*name).to_owned());
                tags.push(format!("x86_64_{name}"));
            }
        }
    } else if os.starts_with("linux") {
        tags.push(format!("{arch}_linux"));
        tags.push("x86_64_linux".to_owned());
    }
    tags.push("all".to_owned());
    tags
}

/// [`TargetResolver`] for Homebrew formulae and bottles.
#[derive(Clone)]
pub struct BrewResolver {
    http: Arc<dyn SiteHttp>,
    detected_tag: Option<String>,
    os: &'static str,
    machine: &'static str,
    mac_major: u32,
}

impl BrewResolver {
    /// Build a resolver with production HTTP I/O and host detection.
    pub fn new<H: SiteHttp + 'static>(
        http: H,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Self {
        Self::from_shared(Arc::new(http))
    }

    /// Build a resolver sharing a [`SiteHttp`] instance.
    pub fn from_shared(http: Arc<dyn SiteHttp>) -> Self {
        Self {
            http,
            detected_tag: None,
            os: match std::env::consts::OS {
                "macos" => "darwin",
                other => other,
            },
            machine: std::env::consts::ARCH,
            mac_major: 15,
        }
    }

    /// Use a tag detected from `brew --cache <formula>` (W4-1 supplies it).
    pub fn with_detected_tag(mut self, tag: impl Into<String>) -> Self {
        self.detected_tag = Some(tag.into());
        self
    }

    /// Override the host used for bottle-tag fallback (deterministic tests).
    pub fn with_host(mut self, os: &'static str, machine: &'static str, mac_major: u32) -> Self {
        self.os = os;
        self.machine = machine;
        self.mac_major = mac_major;
        self
    }

    async fn ghcr_direct_url(
        &self,
        blob_url: &str,
        formula: &str,
        ctx: &ResolveContext,
    ) -> Result<(String, HashMap<String, String>, Option<u64>)> {
        let repo = GHCR_REPO_RE
            .captures(blob_url)
            .map(|caps| caps[1].to_owned())
            .unwrap_or_else(|| format!("homebrew/core/{}", formula.replace('@', "/")));
        let token_url = format!(
            "https://ghcr.io/token?service=ghcr.io&scope=repository:{repo}:pull"
        );
        let token_response = self
            .http
            .fetch_json(&token_url, &HashMap::new(), ctx.timeout)
            .await?;
        let mut auth_headers = HashMap::new();
        if token_response.status == 200
            && let Some(token) = token_response
                .body
                .get("token")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        {
            auth_headers.insert("Authorization".to_owned(), format!("Bearer {token}"));
        }
        if !auth_headers.contains_key("Authorization") {
            return Ok((blob_url.to_owned(), HashMap::new(), None));
        }
        match self
            .http
            .redirect(blob_url, &auth_headers, ctx.timeout)
            .await?
        {
            Some(info) => Ok((info.url, info.headers, info.content_length)),
            None => Ok((blob_url.to_owned(), auth_headers, None)),
        }
    }
}

#[async_trait]
impl TargetResolver for BrewResolver {
    fn scheme(&self) -> &'static str {
        "brew"
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let formula = parse_brew_target(target);
        let api_url = format!("https://formulae.brew.sh/api/formula/{formula}.json");
        let response = self
            .http
            .fetch_json(&api_url, &HashMap::new(), ctx.timeout)
            .await?;
        if response.status != 200 || !response.body.is_object() {
            return Err(Error::failure(format!(
                "No pre-built bottles available for {formula}"
            )));
        }
        let version = response
            .body
            .get("versions")
            .and_then(|value| value.get("stable"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let files = response
            .body
            .get("bottle")
            .and_then(|value| value.get("stable"))
            .and_then(|value| value.get("files"));
        let Some(files) = files.and_then(Value::as_object) else {
            return Err(Error::failure(format!(
                "No pre-built bottles available for {formula}"
            )));
        };
        if files.is_empty() {
            return Err(Error::failure(format!(
                "No pre-built bottles available for {formula}"
            )));
        }

        let chosen = if let Some(tag) = &self.detected_tag {
            if files.contains_key(tag) {
                Some(tag.clone())
            } else {
                None
            }
        } else {
            None
        };
        let chosen_tag = match chosen {
            Some(tag) => tag,
            None => fallback_bottle_tags(self.os, self.machine, self.mac_major)
                .into_iter()
                .find(|tag| files.contains_key(tag))
                .or_else(|| files.keys().next().cloned())
                .unwrap_or_else(|| "all".to_owned()),
        };
        let bottle = &files[&chosen_tag];
        let sha256 = bottle
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let blob_url = bottle
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let filename = format!("{formula}-{version}.{chosen_tag}.bottle.tar.gz");

        let (url, headers, content_length) = if blob_url.contains("ghcr.io") && !blob_url.is_empty()
        {
            self.ghcr_direct_url(&blob_url, &formula, ctx).await?
        } else {
            (blob_url, HashMap::new(), None)
        };

        Ok(vec![ResolvedAsset {
            url,
            filename: Some(filename),
            content_length,
            digest: (!sha256.is_empty()).then(|| format!("sha256:{sha256}")),
            target_type: TargetType::Brew,
            headers,
            media_op: None,
        }])
    }
}
