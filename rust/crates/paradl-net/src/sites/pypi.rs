//! PyPI site engine: PEP 503 normalization, wheel scoring and METADATA parsing.
//!
//! Ports `src/paradl/engines/pypi.py` resolution: `parse_pypi_target`,
//! `score_wheel`, the strict pinned-version 404 behaviour (no fallback to
//! latest), PEP 440 specifier selection and `extract_wheel_dependencies`.
//! Installing with `uv`/`pip --no-index` stays in the CLI layer.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use paradl_core::{Error, TargetType};
use regex::Regex;
use serde_json::Value;
use url::Url;

use super::SiteHttp;
use crate::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use crate::Result;

static SPEC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9_\-\.]+)(?:([=><~@]+)([A-Za-z0-9_\-\.]+))?$")
        .expect("PyPI specifier regex must compile")
});
static NORMALIZE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-_.]+").expect("normalize regex must compile"));

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')
}

/// Extract a package name and optional pinned version from a PyPI target.
///
/// Mirrors `PyPIEngine.parse_pypi_target`, including the "complex specifiers
/// yield no pinned version" oracle cases (`llvmlite<0.51,>=0.50.0dev0`).
pub fn parse_pypi_target(target: &str) -> (String, Option<String>) {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    let spec = if ["pip:", "pip/", "pypi:", "pypi/"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        let after_colon = t.split_once(':').map_or(t, |(_, rest)| rest);
        after_colon
            .split_once('/')
            .map_or(after_colon, |(_, rest)| rest)
            .trim()
            .to_owned()
    } else if lower.contains("pypi.org/project/") || lower.contains("pypi.python.org/pypi/") {
        let path = Url::parse(t).map_or_else(|_| t.to_owned(), |url| url.path().to_owned());
        let parts: Vec<&str> = path
            .split('/')
            .filter(|part| !part.is_empty() && *part != "project" && *part != "pypi")
            .collect();
        return (
            parts.first().map_or_else(String::new, |part| (*part).to_owned()),
            parts.get(1).map(|part| (*part).to_string()),
        );
    } else {
        t.to_owned()
    };

    if let Some(caps) = SPEC_RE.captures(&spec) {
        let package = caps[1].to_owned();
        let version = caps
            .get(3)
            .filter(|_| matches!(caps.get(2).map(|m| m.as_str()), Some("==" | "@")))
            .map(|m| m.as_str().to_owned());
        return (package, version);
    }

    // packaging.Requirement fallback: take the leading name, then a pinned
    // version only for `==` or PEP 508 `@`.
    let package: String = spec.chars().take_while(|c| is_name_char(*c)).collect();
    let version = ["==", "@"]
        .iter()
        .find_map(|op| spec.find(op).map(|index| (op, index)))
        .and_then(|(op, index)| {
            let rest = &spec[index + op.len()..];
            let value: String = rest.chars().take_while(|c| is_name_char(*c)).collect();
            (!value.is_empty()).then_some(value)
        });
    (package, version)
}

/// Python runtime and platform tuple used for wheel compatibility scoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WheelTarget {
    /// `(major, minor)` Python version, e.g. `(3, 12)`.
    pub py_version: (u32, u32),
    /// Python `sys.platform` string (`darwin`, `linux`, `win32`).
    pub platform: String,
    /// Python `platform.machine()` string (`x86_64`, `arm64`, ...).
    pub machine: String,
}

impl WheelTarget {
    /// Build an explicit scoring target.
    pub fn new(py_version: (u32, u32), platform: impl Into<String>, machine: impl Into<String>) -> Self {
        Self {
            py_version,
            platform: platform.into(),
            machine: machine.into(),
        }
    }

    /// Derive a scoring target from a resolve context / host platform.
    pub fn from_context(ctx: &ResolveContext) -> Self {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let machine = match std::env::consts::ARCH {
            "aarch64" | "arm" => "arm64",
            other => other,
        };
        Self {
            py_version: ctx.py_version.unwrap_or((3, 12)),
            platform: platform.to_owned(),
            machine: machine.to_owned(),
        }
    }
}

/// Score a release artifact for the given Python/platform target.
///
/// Returns `-1` for an incompatible wheel; `10` for an sdist fallback. This is
/// a byte-exact port of `PyPIEngine.score_wheel` (higher score wins, first
/// candidate wins ties).
pub fn score_wheel(filename: &str, packagetype: &str, target: &WheelTarget) -> i32 {
    let name = filename.to_ascii_lowercase();
    if packagetype == "sdist" || name.ends_with(".tar.gz") || name.ends_with(".zip") {
        return 10;
    }
    if !name.ends_with(".whl") {
        return -1;
    }

    let (major, minor) = target.py_version;
    let py_ver = format!("cp{major}{minor}");
    let py_maj = format!("py{major}");
    let platform = target.platform.to_ascii_lowercase();
    let machine = target.machine.to_ascii_lowercase();
    let arch_tags: &[&str] = if matches!(machine.as_str(), "arm64" | "aarch64") {
        &["arm64", "aarch64", "universal2", "win_arm64"]
    } else {
        &["x86_64", "amd64", "universal2", "x86", "win_amd64", "win32"]
    };

    let is_universal = name.ends_with("-any.whl") || name.contains("none-any");
    // The arch tag applies to all non-universal platform branches, exactly as
    // in the Python oracle's parenthesised expression.
    let is_plat_compat = if is_universal {
        true
    } else if platform == "darwin" {
        name.contains("macosx") && arch_tags.iter().any(|tag| name.contains(tag))
    } else if platform.starts_with("linux") {
        ["manylinux", "musllinux", "linux"]
            .iter()
            .any(|tag| name.contains(tag))
            && arch_tags.iter().any(|tag| name.contains(tag))
    } else if platform == "win32" {
        ["win_amd64", "win_arm64", "win32", "win"]
            .iter()
            .any(|tag| name.contains(tag))
            && arch_tags.iter().any(|tag| name.contains(tag))
    } else {
        false
    };
    if !is_plat_compat {
        return -1;
    }

    let mut score = 20;
    if name.contains(&py_ver) {
        score += 60;
    } else if name.contains("abi3") {
        score += 50;
    } else if name.contains("py3-none-any")
        || name.contains("py2.py3-none-any")
        || is_universal
    {
        score += 40;
    } else if name.contains(&format!("-{py_maj}-"))
        || name.starts_with(&format!("{py_maj}-"))
        || name.contains(&format!(".{py_maj}."))
    {
        score += 30;
    }

    if platform == "win32" {
        if matches!(machine.as_str(), "arm64" | "aarch64") {
            if name.contains("win_arm64") {
                score += 20;
            } else if name.contains("win_amd64") {
                score += 5;
            } else if name.contains("win32") {
                score += 2;
            } else if is_universal {
                score += 10;
            }
        } else if matches!(machine.as_str(), "x86" | "i386" | "i686") {
            if name.contains("win32") && !name.contains("win_amd64") {
                score += 20;
            } else if is_universal {
                score += 10;
            }
        } else if name.contains("win_amd64") {
            score += 20;
        } else if name.contains("win32") {
            score += 5;
        } else if is_universal {
            score += 10;
        }
    } else if arch_tags
        .iter()
        .any(|tag| *tag != "universal2" && name.contains(tag))
    {
        score += 20;
    } else if name.contains("universal2") {
        score += 15;
    } else if is_universal {
        score += 10;
    }
    score
}

static PEP440_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?ix)^v?(?:(?P<epoch>\d+)!)?(?P<release>\d+(?:\.\d+)*)",
        r"(?:(?P<pre>alpha|beta|preview|pre|rc|a|b|c)[-_.]?(?P<pre_n>\d*))?",
        r"(?:(?:-(?P<post_dash>\d+)|\.?(?P<post>post|rev|r)[-_.]?(?P<post_n>\d*)))?",
        r"(?:\.?(?P<dev>dev)[-_.]?(?P<dev_n>\d*))?",
        r"(?:\+[A-Za-z0-9_.-]+)?$"
    ))
    .expect("PEP 440 regex must compile")
});

/// Minimal PEP 440 version model for PyPI specifier filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PepVersion {
    raw: String,
    epoch: u64,
    release: Vec<u64>,
    pre: Option<(String, u64)>,
    post: Option<u64>,
    dev: Option<u64>,
}

/// Total ordering key matching PEP 440's release/pre/post/dev precedence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct VersionKey {
    epoch: u64,
    release: Vec<u64>,
    pre_rank: i32,
    pre_num: u64,
    post_num: u64,
    dev_num: u64,
}

impl PepVersion {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let caps = PEP440_RE.captures(trimmed)?;
        let release = caps
            .name("release")?
            .as_str()
            .split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect::<Option<Vec<_>>>()?;
        let number = |name: &str| {
            caps.name(name).map_or(0, |group| {
                let value = group.as_str();
                if value.is_empty() {
                    0
                } else {
                    value.parse::<u64>().unwrap_or(0)
                }
            })
        };
        let pre = caps.name("pre").map(|group| {
            let token = group.as_str().to_ascii_lowercase();
            let normalized = match token.as_str() {
                "alpha" => "a",
                "beta" => "b",
                "c" | "pre" | "preview" => "rc",
                other => other,
            };
            (normalized.to_owned(), number("pre_n"))
        });
        let post = if caps.name("post_dash").is_some() {
            Some(number("post_dash"))
        } else if caps.name("post").is_some() {
            Some(number("post_n"))
        } else {
            None
        };
        let dev = caps.name("dev").map(|_| number("dev_n"));
        Some(Self {
            raw: trimmed.to_owned(),
            epoch: caps
                .name("epoch")
                .and_then(|group| group.as_str().parse::<u64>().ok())
                .unwrap_or(0),
            release,
            pre,
            post,
            dev,
        })
    }

    /// PEP 440 `Version.is_prerelease`: alpha/beta/rc **or** dev segment.
    /// A post-release (`1.3.0.post1`) is stable and must not be filtered out.
    fn is_prerelease(&self) -> bool {
        self.pre.is_some() || self.dev.is_some()
    }

    fn key(&self) -> VersionKey {
        let mut release = self.release.clone();
        release.resize(8, 0);
        let (pre_rank, pre_num) = match (&self.pre, self.dev, self.post) {
            (Some((kind, number)), _, _) => (
                match kind.as_str() {
                    "a" => -2,
                    "b" => -1,
                    _ => 0,
                },
                *number,
            ),
            (None, Some(_), _) => (-3, 0),
            (None, None, Some(_)) => (2, 0),
            (None, None, None) => (1, 0),
        };
        VersionKey {
            epoch: self.epoch,
            release,
            pre_rank,
            pre_num,
            post_num: self.post.unwrap_or(0),
            dev_num: self.dev.unwrap_or(0),
        }
    }
}

fn compare_versions(left: &PepVersion, right: &PepVersion) -> Ordering {
    left.key().cmp(&right.key())
}

fn release_starts_with(version: &PepVersion, prefix: &[u64]) -> bool {
    prefix
        .iter()
        .enumerate()
        .all(|(index, part)| version.release.get(index) == Some(part))
}

/// Evaluate one PEP 440 specifier (including compatible-release `~=` and
/// wildcard `==`/`!=`) against a parsed version.
fn satisfies(constraint: &str, version: &PepVersion) -> bool {
    let constraint = constraint.trim();
    if let Some(raw) = constraint.strip_prefix("===") {
        return raw.trim() == version.raw;
    }
    // ~=X.Y       -> >=X.Y, ==X.*
    // ~=X.Y.Z     -> >=X.Y.Z, ==X.Y.*
    if let Some(raw) = constraint.strip_prefix("~=") {
        let Some(lower) = PepVersion::parse(raw) else {
            return false;
        };
        if lower.release.len() < 2 {
            return false;
        }
        let prefix = &lower.release[..lower.release.len() - 1];
        return compare_versions(version, &lower) != Ordering::Less
            && release_starts_with(version, prefix);
    }
    for (op, operand) in [
        ("==", constraint.strip_prefix("==")),
        ("!=", constraint.strip_prefix("!=")),
        (">=", constraint.strip_prefix(">=")),
        ("<=", constraint.strip_prefix("<=")),
        (">", constraint.strip_prefix(">")),
        ("<", constraint.strip_prefix("<")),
    ] {
        if let Some(raw) = operand {
            let raw = raw.trim();
            if let Some(prefix) = raw.strip_suffix(".*") {
                let Some(lower) = PepVersion::parse(prefix) else {
                    return false;
                };
                let matches = release_starts_with(version, &lower.release);
                return if op == "!=" { !matches } else { matches };
            }
            let Some(expected) = PepVersion::parse(raw) else {
                return false;
            };
            return match op {
                "==" => compare_versions(version, &expected) == Ordering::Equal,
                "!=" => compare_versions(version, &expected) != Ordering::Equal,
                ">=" => compare_versions(version, &expected) != Ordering::Less,
                "<=" => compare_versions(version, &expected) != Ordering::Greater,
                ">" => compare_versions(version, &expected) == Ordering::Greater,
                "<" => compare_versions(version, &expected) == Ordering::Less,
                _ => true,
            };
        }
    }
    true
}

/// Select the release whose files should be used for a specifier target.
/// Mirrors `pypi.py:261-288`, including the `info.version` short-circuit and
/// the exclusion of PEP 440 prereleases from the fallback search.
fn apply_specifiers(target: &str, body: &Value, urls: &mut Vec<Value>, version: &mut String) {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    let spec_text = if ["pip:", "pip/", "pypi:", "pypi/"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        t.split_once(':').map_or(t, |(_, rest)| rest)
    } else {
        t
    };
    let name_len = spec_text
        .chars()
        .take_while(|c| is_name_char(*c))
        .map(char::len_utf8)
        .sum::<usize>();
    let constraints: Vec<&str> = spec_text[name_len..]
        .trim()
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if constraints.is_empty() {
        return;
    }

    // Oracle short-circuit: the base API response already links the newest
    // version; keep those files when that version satisfies the specifier.
    let current = body
        .get("info")
        .and_then(|info| info.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if let Some(current_version) = PepVersion::parse(current)
        && constraints
            .iter()
            .all(|constraint| satisfies(constraint, &current_version))
    {
        return;
    }

    let Some(releases) = body.get("releases").and_then(Value::as_object) else {
        return;
    };
    let mut matched: Vec<(VersionKey, String)> = Vec::new();
    for (version_string, files) in releases {
        let Some(parsed) = PepVersion::parse(version_string) else {
            continue;
        };
        if parsed.is_prerelease()
            || !constraints
                .iter()
                .all(|constraint| satisfies(constraint, &parsed))
        {
            continue;
        }
        if files.as_array().is_some_and(|files| !files.is_empty()) {
            matched.push((parsed.key(), version_string.clone()));
        }
    }
    matched.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some((_, chosen)) = matched.last() {
        if let Some(files) = releases.get(chosen).and_then(Value::as_array) {
            *urls = files.clone();
        }
        *version = chosen.clone();
    }
}

fn default_marker_env() -> HashMap<String, String> {
    let os = std::env::consts::OS;
    HashMap::from([
        (
            "platform_system".to_owned(),
            match os {
                "macos" => "Darwin",
                "windows" => "Windows",
                _ => "Linux",
            }
            .to_owned(),
        ),
        (
            "sys_platform".to_owned(),
            match os {
                "macos" => "darwin",
                "windows" => "win32",
                _ => "linux",
            }
            .to_owned(),
        ),
        (
            "os_name".to_owned(),
            if os == "windows" { "nt" } else { "posix" }.to_owned(),
        ),
        ("python_version".to_owned(), "3.12".to_owned()),
        ("python_full_version".to_owned(), "3.12.0".to_owned()),
    ])
}

fn eval_comparison(expression: &str, env: &HashMap<String, String>) -> bool {
    for op in ["==", "!=", ">=", "<=", ">", "<"] {
        if let Some((lhs, rhs)) = expression.split_once(op) {
            let actual = env.get(lhs.trim()).map_or("", String::as_str);
            let expected = rhs.trim().trim_matches(['\'', '"']);
            return match op {
                "==" => actual == expected,
                "!=" => actual != expected,
                ">=" => actual >= expected,
                "<=" => actual <= expected,
                ">" => actual > expected,
                _ => actual < expected,
            };
        }
    }
    false
}

fn eval_marker(marker: &str, env: &HashMap<String, String>) -> bool {
    marker.split(" or ").any(|group| {
        group
            .split(" and ")
            .all(|clause| eval_comparison(clause.trim(), env))
    })
}

/// Read active `Requires-Dist` entries from a local wheel's METADATA.
///
/// Inactive environment markers (e.g. `platform_system == 'NonExistentOS'`)
/// and extras (`extra == 'feature'`) are dropped, matching the Python oracle.
pub fn extract_wheel_dependencies(wheel_path: &Path) -> Result<Vec<String>> {
    let env = default_marker_env();
    extract_wheel_dependencies_with_env(wheel_path, &env)
}

/// [`extract_wheel_dependencies`] with an explicit PEP 508 marker environment.
pub fn extract_wheel_dependencies_with_env(
    wheel_path: &Path,
    env: &HashMap<String, String>,
) -> Result<Vec<String>> {
    if wheel_path.extension().and_then(|ext| ext.to_str()) != Some("whl") || !wheel_path.is_file() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(wheel_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| Error::failure(format!("Failed to read wheel {wheel_path:?}: {error}")))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::failure(format!("Failed to read wheel entry: {error}")))?;
        if !entry.name().ends_with(".dist-info/METADATA") {
            continue;
        }
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|error| Error::failure(format!("Failed to read wheel METADATA: {error}")))?;
        let mut requirements = Vec::new();
        for line in content.lines() {
            let Some(raw) = line.strip_prefix("Requires-Dist:") else {
                continue;
            };
            let raw = raw.trim();
            let (requirement, marker) = raw.split_once(';').map_or((raw, None), |(req, mark)| {
                (req.trim(), Some(mark.trim()))
            });
            if let Some(marker) = marker {
                if marker.contains("extra ==") || marker.contains("extra==") {
                    continue;
                }
                if !eval_marker(marker, env) {
                    continue;
                }
            }
            if !requirement.is_empty() {
                requirements.push(requirement.to_owned());
            }
        }
        return Ok(requirements);
    }
    Ok(Vec::new())
}

/// [`TargetResolver`] for PyPI packages and URLs.
#[derive(Clone)]
pub struct PyPIResolver {
    http: Arc<dyn SiteHttp>,
    target: WheelTarget,
    offline: bool,
}

impl PyPIResolver {
    /// Build a resolver with production HTTP I/O.
    pub fn new<H: SiteHttp + 'static>(http: H, secrets: Arc<dyn crate::sites::token::SecretProvider>) -> Self {
        Self::from_shared(Arc::new(http), secrets)
    }

    /// Build a resolver sharing a [`SiteHttp`] instance.
    pub fn from_shared(http: Arc<dyn SiteHttp>, _secrets: Arc<dyn crate::sites::token::SecretProvider>) -> Self {
        Self {
            http,
            target: WheelTarget::from_context(&ResolveContext::default()),
            offline: false,
        }
    }

    /// Override the wheel scoring target (deterministic tests).
    pub fn with_target(mut self, target: WheelTarget) -> Self {
        self.target = target;
        self
    }

    /// Produce offline synthetic metadata (classification-table testing).
    pub fn with_offline(mut self, enabled: bool) -> Self {
        self.offline = enabled;
        self
    }
}

#[async_trait]
impl TargetResolver for PyPIResolver {
    fn scheme(&self) -> &'static str {
        "pip"
    }

    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>> {
        let (package, version) = parse_pypi_target(target);
        let failure = || Error::failure(format!("Failed to resolve PyPI package: {target}"));
        if package.is_empty() {
            return Err(failure());
        }
        let normalized = NORMALIZE_RE.replace_all(&package, "-").to_ascii_lowercase();
        let api_url = if let Some(version) = &version {
            format!("https://pypi.org/pypi/{normalized}/{version}/json")
        } else {
            format!("https://pypi.org/pypi/{normalized}/json")
        };
        if self.offline {
            return Ok(vec![ResolvedAsset {
                filename: Some(format!("{normalized}.whl")),
                target_type: TargetType::Pypi,
                ..Default::default()
            }]);
        }

        let response = self.http.fetch_json(&api_url, &HashMap::new(), ctx.timeout).await?;
        if response.status != 200 || !response.body.is_object() {
            // Oracle H2: a pinned 404 never falls back to the latest release.
            return Err(failure());
        }
        let mut urls = response
            .body
            .get("urls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut resolved_version = version.unwrap_or_default();
        apply_specifiers(target, &response.body, &mut urls, &mut resolved_version);
        if urls.is_empty() {
            return Err(failure());
        }

        let mut best: Option<&Value> = None;
        let mut best_score = -1;
        for candidate in &urls {
            let filename = candidate.get("filename").and_then(Value::as_str).unwrap_or_default();
            let packagetype = candidate
                .get("packagetype")
                .and_then(Value::as_str)
                .unwrap_or("bdist_wheel");
            let score = score_wheel(filename, packagetype, &self.target);
            if score > best_score {
                best_score = score;
                best = Some(candidate);
            }
        }
        let Some(candidate) = best else {
            return Err(failure());
        };
        let digest = candidate
            .get("digests")
            .and_then(|value| value.get("sha256"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format!("sha256:{value}"));
        Ok(vec![ResolvedAsset {
            url: candidate
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            filename: Some(
                candidate
                    .get("filename")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("{normalized}.whl"), ToOwned::to_owned),
            ),
            content_length: candidate.get("size").and_then(Value::as_u64),
            digest,
            target_type: TargetType::Pypi,
            ..Default::default()
        }])
    }
}
