//! Hugging Face site engine: repo/file shortcuts, LFS digests and token flows.
//!
//! Ports the resolution half of `src/paradl/engines/hf.py`: target parsing,
//! `resolve` URL construction, sibling filtering (including the exact
//! "No files matched ..." error) and LFS SHA-256 selection.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use paradl_core::{Error, TargetType};
use regex::Regex;
use serde_json::Value;

use super::matches_pattern;
use super::token::{SecretProvider, SiteHeaders, SiteService};
use super::SiteHttp;
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::Result;

static FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/]+)/(?:resolve|blob)/([^/]+)/(.+)")
        .expect("HF file regex must compile")
});
static TREE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/]+)/tree/([^/]+)(?:/(.+))?")
        .expect("HF tree regex must compile")
});
static REPO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/?#]+)(?:/)?(?:\?[^#]*)?(?:#.*)?$")
        .expect("HF repo regex must compile")
});

fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn strip_query(value: &str) -> &str {
    value.split(['?', '#']).next().unwrap_or(value)
}

/// Model or dataset repository type (Python `"model"` / `"dataset"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HfRepoType {
    /// A model repository.
    Model,
    /// A dataset repository.
    Dataset,
}

impl HfRepoType {
    /// Python string value (`model` / `dataset`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Dataset => "dataset",
        }
    }
}

/// A parsed Hugging Face target (mirrors Python's `parse_hf_target` dict).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfTarget {
    /// Repository id such as `meta-llama/Llama-3.1-8B`.
    pub repo_id: String,
    /// Model or dataset.
    pub repo_type: HfRepoType,
    /// Optional file/subpath inside the repository.
    pub subpath: Option<String>,
    /// Revision/branch (defaults to `main`).
    pub revision: String,
    /// `true` for `/resolve/` and `/blob/` file URLs.
    pub is_direct_file: bool,
}

impl HfTarget {
    /// Parse a `hf:` shortcut or a Hugging Face URL.
    pub fn parse(target: &str) -> Self {
        let t = target.trim();
        let mut repo_type = HfRepoType::Model;

        if let Some(raw) = t
            .strip_prefix("hf:")
            .or_else(|| t.strip_prefix("huggingface:"))
        {
            let mut raw_path = raw.trim();
            if let Some(rest) = raw_path.strip_prefix("datasets/") {
                repo_type = HfRepoType::Dataset;
                raw_path = rest;
            }
            let parts: Vec<&str> = raw_path.split('/').filter(|part| !part.is_empty()).collect();
            let repo_id = if parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else if parts.len() == 1 {
                parts[0].to_owned()
            } else {
                return Self {
                    repo_id: t.to_owned(),
                    repo_type,
                    subpath: None,
                    revision: "main".to_owned(),
                    is_direct_file: false,
                };
            };
            return Self {
                repo_id,
                repo_type,
                subpath: (parts.len() > 2).then(|| parts[2..].join("/")),
                revision: "main".to_owned(),
                is_direct_file: false,
            };
        }

        if let Some(caps) = FILE_RE.captures(t) {
            return Self {
                repo_id: caps[2].to_owned(),
                repo_type: if caps.get(1).is_some() {
                    HfRepoType::Dataset
                } else {
                    HfRepoType::Model
                },
                subpath: Some(strip_query(&caps[4]).to_owned()),
                revision: caps[3].to_owned(),
                is_direct_file: true,
            };
        }
        if let Some(caps) = TREE_RE.captures(t) {
            return Self {
                repo_id: caps[2].to_owned(),
                repo_type: if caps.get(1).is_some() {
                    HfRepoType::Dataset
                } else {
                    HfRepoType::Model
                },
                subpath: caps.get(4).map(|m| strip_query(m.as_str()).to_owned()),
                revision: caps[3].to_owned(),
                is_direct_file: false,
            };
        }
        if let Some(caps) = REPO_RE.captures(t) {
            return Self {
                repo_id: caps[2].to_owned(),
                repo_type: if caps.get(1).is_some() {
                    HfRepoType::Dataset
                } else {
                    HfRepoType::Model
                },
                subpath: None,
                revision: "main".to_owned(),
                is_direct_file: false,
            };
        }
        Self {
            repo_id: t.to_owned(),
            repo_type: HfRepoType::Model,
            subpath: None,
            revision: "main".to_owned(),
            is_direct_file: false,
        }
    }
}

/// Build a Hugging Face `resolve` URL (byte-exact with `_build_resolve_url`).
pub fn build_resolve_url(
    repo_id: &str,
    revision: &str,
    filename: &str,
    repo_type: HfRepoType,
) -> String {
    let prefix = if repo_type == HfRepoType::Dataset {
        "datasets/"
    } else {
        ""
    };
    format!("https://huggingface.co/{prefix}{repo_id}/resolve/{revision}/{filename}")
}

/// One filtered repository file with optional LFS digest/size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HfFile {
    /// `rfilename` from the Hub API.
    pub filename: String,
    /// LFS OID (SHA-256), when the file is Git LFS-backed.
    pub lfs_sha: Option<String>,
    /// File size in bytes, when advertised.
    pub size: Option<u64>,
}

impl HfFile {
    fn from_entry(entry: &Value) -> Option<Self> {
        let filename = entry.get("rfilename")?.as_str()?.to_owned();
        let lfs = entry.get("lfs").filter(|value| value.is_object());
        let lfs_sha = lfs
            .and_then(|value| value.get("oid"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let size = lfs
            .and_then(|value| value.get("size"))
            .and_then(Value::as_u64)
            .or_else(|| entry.get("size").and_then(Value::as_u64));
        Some(Self {
            filename,
            lfs_sha,
            size,
        })
    }
}

/// Filter Hub API siblings by hidden files, subpath and asset pattern.
///
/// Returns the Python oracle's exact error when a filter is present but nothing
/// matches: `No files matched <pattern|subpath> in repository <repo_id>.`
pub fn filter_siblings(
    siblings: &[Value],
    subpath: Option<&str>,
    pattern: Option<&str>,
    repo_id: &str,
) -> std::result::Result<Vec<HfFile>, Error> {
    let mut valid: Vec<HfFile> = siblings
        .iter()
        .filter_map(HfFile::from_entry)
        .filter(|file| !file.filename.starts_with('.'))
        .collect();

    if let Some(subpath) = subpath {
        let suffix = format!("/{subpath}");
        valid.retain(|file| file.filename == subpath || file.filename.ends_with(&suffix));
    }
    if let Some(pattern) = pattern {
        valid.retain(|file| matches_pattern(&file.filename, pattern));
    }
    if valid.is_empty() && (subpath.is_some() || pattern.is_some()) {
        let description = match pattern {
            Some(pattern) => format!("pattern '{pattern}'"),
            None => format!("subpath '{}'", subpath.unwrap_or_default()),
        };
        return Err(Error::failure(format!(
            "No files matched {description} in repository {repo_id}."
        )));
    }
    Ok(valid)
}

/// [`TargetResolver`] for Hugging Face models and datasets.
#[derive(Clone)]
pub struct HfResolver {
    http: Arc<dyn SiteHttp>,
    secrets: Arc<dyn SecretProvider>,
    pattern: Option<String>,
}

impl HfResolver {
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
        }
    }

    /// Restrict matched files with a glob/regex pattern (Python `--asset`).
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    fn asset(&self, file: &HfFile, repo_id: &str, revision: &str, repo_type: HfRepoType) -> ResolvedAsset {
        let url = build_resolve_url(repo_id, revision, &file.filename, repo_type);
        ResolvedAsset {
            url,
            filename: Some(basename(&file.filename)),
            content_length: file.size,
            digest: file.lfs_sha.as_ref().map(|sha| format!("sha256:{sha}")),
            target_type: TargetType::Hf,
            headers: SiteHeaders::rest(SiteService::Hf, self.secrets.as_ref()),
            media_op: None,
        }
    }
}

#[async_trait]
impl TargetResolver for HfResolver {
    fn scheme(&self) -> &'static str {
        "hf"
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let parsed = HfTarget::parse(target);
        let headers = SiteHeaders::rest(SiteService::Hf, self.secrets.as_ref());

        if parsed.is_direct_file
            && let Some(subpath) = &parsed.subpath
        {
            return Ok(vec![ResolvedAsset {
                url: build_resolve_url(
                    &parsed.repo_id,
                    &parsed.revision,
                    subpath,
                    parsed.repo_type,
                ),
                filename: Some(basename(subpath)),
                target_type: TargetType::Hf,
                headers,
                ..Default::default()
            }]);
        }

        let endpoint = if parsed.repo_type == HfRepoType::Dataset {
            "datasets"
        } else {
            "models"
        };
        let api_url = format!("https://huggingface.co/api/{endpoint}/{}", parsed.repo_id);
        let response = self.http.fetch_json(&api_url, &headers, ctx.timeout).await?;
        if response.status != 200 || !response.body.is_object() {
            return Err(Error::failure(format!(
                "Repository not found or private on Hugging Face: {}",
                parsed.repo_id
            )));
        }
        let siblings = response
            .body
            .get("siblings")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if siblings.is_empty() {
            return Err(Error::failure(format!(
                "No files found in repository {}",
                parsed.repo_id
            )));
        }
        let files = filter_siblings(
            &siblings,
            parsed.subpath.as_deref(),
            self.pattern.as_deref(),
            &parsed.repo_id,
        )?;
        Ok(files
            .iter()
            .map(|file| self.asset(file, &parsed.repo_id, &parsed.revision, parsed.repo_type))
            .collect())
    }
}
