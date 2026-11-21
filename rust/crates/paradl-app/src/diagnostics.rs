//! Diagnostics for the reference binary: Python-free tool probing, an
//! oracle-aligned JSONL payload, and a deterministic fixed-width human report.
//!
//! [`DoctorReport`] reports the binary version, the `paradl-media/1` protocol,
//! the Python media-package version, yt-dlp version and JS-runtime status. It
//! works with **no Python** on non-media paths: tool versions come from
//! external native binaries and on-disk package metadata, never from
//! `python -m paradl_media` and never from PyO3.
//!
//! The JSON event field order mirrors `src/paradl/diagnostics/doctor.py`
//! exactly (Python dicts preserve insertion order): every nested object is a
//! typed struct with declared field order, never a sorted `serde_json::Map`.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use paradl_events::{emit_jsonl, now_timestamp};
use paradl_resilience::auth::{KeychainSecretStore, OsEnv};
use serde::Serialize;

/// Protocol spoken with the (Python) media worker.
pub const MEDIA_PROTOCOL: &str = "paradl-media/1";
/// Version of the standalone Python media package (`media/pyproject.toml`).
pub const MEDIA_PACKAGE_VERSION: &str = "0.1.0";
/// Version reported by the Python oracle (`src/paradl/__init__.py:5`).
pub const ORACLE_VERSION: &str = "1.0.0";
/// Stable token used for absent values in the human report.
pub const NOT_FOUND: &str = "not found";
/// Fixed label width used by every line of the human report.
const LABEL_WIDTH: usize = 18;
/// Fixed section separator (no terminal-width detection).
const SEPARATOR: &str = "------------------";

/// Versions and paths of the external tools `doctor` reports, gathered
/// without Python. Each field is `None` when the native tool is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolProbe {
    /// yt-dlp version (`yt-dlp --version` first line).
    pub yt_dlp_version: Option<String>,
    /// Resolved path of the yt-dlp binary.
    pub yt_dlp_path: Option<String>,
    /// JS runtime label (`deno (/opt/homebrew/bin/deno)`, …).
    pub js_runtime: Option<String>,
    /// Node.js version (`node --version`).
    pub node_version: Option<String>,
    /// Deno version (`deno --version` first line).
    pub deno_version: Option<String>,
    /// Bun version (`bun --version`).
    pub bun_version: Option<String>,
    /// ffmpeg version (`ffmpeg -version` first line).
    pub ffmpeg_version: Option<String>,
    /// npm version (`npm --version`).
    pub npm_version: Option<String>,
    /// Resolved path of the npm binary.
    pub npm_path: Option<String>,
    /// Resolved path of the deno binary (preferred JS runtime in the oracle).
    pub deno_path: Option<String>,
    /// Resolved path of the aria2c binary.
    pub aria2c_path: Option<String>,
    /// Resolved path of the afconvert binary.
    pub afconvert_path: Option<String>,
    /// Resolved path of the ffmpeg binary.
    pub ffmpeg_path: Option<String>,
    /// yt-dlp-ejs version from on-disk package metadata (never Python).
    pub ytdlp_ejs_version: Option<String>,
}

impl ToolProbe {
    /// Probe external tool binaries on `PATH` with short, bounded commands.
    ///
    /// Only native binaries are executed; no Python interpreter is discovered
    /// or spawned. A missing or slow tool yields `None`.
    pub fn detect() -> Self {
        let deno_path = tool_path("deno", &[]);
        let js_runtime = deno_path
            .as_ref()
            .map(|path| format!("deno ({path})"))
            .or_else(|| tool_path("node", &[]).map(|path| format!("node ({path})")))
            .or_else(|| tool_path("quickjs", &[]).map(|path| format!("quickjs ({path})")))
            .or_else(|| tool_path("bun", &[]).map(|path| format!("bun [deprecated] ({path})")));
        Self {
            yt_dlp_version: first_line(&["yt-dlp", "--version"]),
            yt_dlp_path: tool_path("yt-dlp", &[]),
            js_runtime,
            node_version: first_line(&["node", "--version"]),
            deno_version: first_line(&["deno", "--version"]),
            bun_version: first_line(&["bun", "--version"]),
            ffmpeg_version: first_line(&["ffmpeg", "-version"]),
            npm_version: first_line(&["npm", "--version"]),
            npm_path: tool_path("npm", &[]),
            deno_path,
            aria2c_path: tool_path("aria2c", &[]),
            afconvert_path: tool_path("afconvert", &["/usr/bin/afconvert"]),
            ffmpeg_path: tool_path("ffmpeg", &[]),
            ytdlp_ejs_version: discover_ytdlp_ejs_version(),
        }
    }
}

/// Environment-dependent doctor values that cannot be derived from constants.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DoctorEnv {
    /// `get_auth_status()` equivalent: per-service source metadata.
    pub auth: serde_json::Value,
    /// `check_platform_utilities()` equivalent.
    pub platform_utilities: PlatformUtilities,
    /// Version of the legacy PyO3 engine module (always absent post-W0-1).
    pub rust_engine_version: Option<String>,
    /// npm version, when installed.
    pub npm_version: Option<String>,
    /// npm path, when installed.
    pub npm_path: Option<String>,
    /// Recommended fix commands (deduplicated, stable order).
    pub fixes: Vec<String>,
}

/// Complete `check_platform_utilities()` result with the oracle's key order.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PlatformUtilities {
    /// `caffeinate` availability.
    pub caffeinate: UtilityRow,
    /// `scutil` availability.
    pub scutil: UtilityRow,
    /// `pmset` availability.
    pub pmset: UtilityRow,
    /// `security` availability.
    pub security: UtilityRow,
    /// `codesign` availability.
    pub codesign: UtilityRow,
    /// `spctl` availability.
    pub spctl: UtilityRow,
    /// `xattr` availability.
    pub xattr: UtilityRow,
}

/// One entry of `platform_utilities` / `macos_utilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UtilityRow {
    /// Whether the binary was found.
    pub available: bool,
    /// Absolute path, or `null`.
    pub path: Option<String>,
    /// Human-readable remediation hint.
    pub fix: String,
}

/// The complete `doctor` / `diagnose` payload, independent of stdout.
#[derive(Debug, Clone, PartialEq)]
pub struct DoctorReport {
    /// Reference-binary crate version (`CARGO_PKG_VERSION`, currently `1.0.0`).
    pub binary_version: String,
    /// Python oracle version reported in `--json` (`src/paradl/__init__.py`).
    pub oracle_version: String,
    /// Media protocol identifier (`paradl-media/1`).
    pub media_protocol: String,
    /// Python media-package version (`0.1.0`).
    pub media_package_version: String,
    /// yt-dlp version, when the external binary is installed.
    pub yt_dlp_version: Option<String>,
    /// Age of the yt-dlp release in days, when its version encodes a date.
    pub ytdlp_age_days: Option<i64>,
    /// Whether the detected yt-dlp release is older than 30 days.
    pub ytdlp_outdated: bool,
    /// First available JS-runtime label, when one is installed.
    pub js_runtime: Option<String>,
    /// Whether the `media` feature is compiled in.
    pub media_enabled: bool,
    /// Always `false`: non-media diagnostics never probe or spawn Python.
    pub python_probed: bool,
    /// Full native-tool probe data.
    pub probe: ToolProbe,
    /// Environment-dependent fields of the oracle payload.
    pub env: DoctorEnv,
}

impl DoctorReport {
    /// Build a report from constants and Python-free environment inspection.
    pub fn collect(media_enabled: bool) -> Self {
        Self {
            binary_version: env!("CARGO_PKG_VERSION").to_owned(),
            oracle_version: ORACLE_VERSION.to_owned(),
            media_protocol: MEDIA_PROTOCOL.to_owned(),
            media_package_version: MEDIA_PACKAGE_VERSION.to_owned(),
            yt_dlp_version: None,
            ytdlp_age_days: None,
            ytdlp_outdated: false,
            js_runtime: None,
            media_enabled,
            python_probed: false,
            probe: ToolProbe::default(),
            env: DoctorEnv {
                auth: auth_status(),
                ..DoctorEnv::default()
            },
        }
    }

    /// Enrich the report with externally probed tool versions.
    ///
    /// No Python is involved; [`ToolProbe::detect`] only executes native
    /// binaries found on `PATH` and reads on-disk package metadata.
    pub fn with_tools(mut self, tools: &ToolProbe) -> Self {
        let age = tools.yt_dlp_version.as_deref().and_then(ytdlp_age_days);
        self.yt_dlp_version = tools.yt_dlp_version.clone();
        self.ytdlp_age_days = age;
        self.ytdlp_outdated = age.is_some_and(|days| days > 30);
        self.js_runtime = tools.js_runtime.clone();
        self.env.npm_version = tools.npm_version.clone();
        self.env.npm_path = tools.npm_path.clone();
        self.env.platform_utilities = platform_utilities();
        self.env.fixes = fixes_for(tools, &self.env);
        self.probe = tools.clone();
        self
    }

    /// Render the deterministic, fixed-width plain human report.
    ///
    /// No terminal-width detection, no colour/ANSI, stable section and field
    /// ordering, and a trailing newline. Absent values render as
    /// [`NOT_FOUND`]; this is byte-identical in a pipe and in a TTY.
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        let section = |title: &str, out: &mut String| {
            out.push_str(title);
            out.push('\n');
            out.push_str(SEPARATOR);
            out.push('\n');
        };
        let row = |label: &str, value: &str, out: &mut String| {
            out.push_str(&format!("{label:<LABEL_WIDTH$}{value}\n"));
        };

        section("paraDL doctor", &mut out);
        row("binary", &self.binary_version, &mut out);
        row("oracle version", &self.oracle_version, &mut out);
        row("media protocol", &self.media_protocol, &mut out);
        row("media package", &self.media_package_version, &mut out);
        row("python probed", bool_token(self.python_probed), &mut out);
        row("media enabled", bool_token(self.media_enabled), &mut out);
        out.push('\n');

        section("toolchain", &mut out);
        row("yt-dlp", self.yt_dlp_version.as_deref().unwrap_or(NOT_FOUND), &mut out);
        row("node", self.probe.node_version.as_deref().unwrap_or(NOT_FOUND), &mut out);
        row("deno", self.probe.deno_version.as_deref().unwrap_or(NOT_FOUND), &mut out);
        row("bun", self.probe.bun_version.as_deref().unwrap_or(NOT_FOUND), &mut out);
        row("ffmpeg", self.probe.ffmpeg_version.as_deref().unwrap_or(NOT_FOUND), &mut out);
        row("js runtime", self.js_runtime.as_deref().unwrap_or(NOT_FOUND), &mut out);
        out.push('\n');

        section("authentication", &mut out);
        row("hf", auth_source(&self.env.auth, "hf"), &mut out);
        row("github", auth_source(&self.env.auth, "github"), &mut out);
        out.push('\n');

        section("fixes", &mut out);
        if self.env.fixes.is_empty() {
            row("status", "none", &mut out);
        } else {
            for fix in &self.env.fixes {
                row("recommended", fix, &mut out);
            }
        }
        out
    }

    /// Render the `doctor` and `diagnose` JSONL lines via the single emitter.
    ///
    /// Field order and nesting match `src/paradl/diagnostics/doctor.py:349-375`
    /// byte-for-byte; only the timestamp value differs (wall clock).
    pub fn json_events(&self) -> Vec<String> {
        let timestamp = now_timestamp();
        let doctor = DoctorJson::new("doctor", timestamp, self);
        let diagnose = DoctorJson::new("diagnose", timestamp, self);
        vec![emit_jsonl(&doctor), emit_jsonl(&diagnose)]
    }
}

// ---- Typed JSON rows: field order is the byte contract --------------------

#[derive(Serialize)]
struct DoctorJson<'a> {
    event: &'static str,
    timestamp: f64,
    version: &'a str,
    ytdlp: YtdlpJson,
    ytdlp_ejs: VersionOnlyJson,
    rust_engine: RustEngineJson,
    npm: NpmJson,
    auth: AuthJson<'a>,
    platform_utilities: &'a PlatformUtilities,
    macos_utilities: &'a PlatformUtilities,
    core_tools: CoreToolsJson,
    fixes: &'a [String],
}

impl<'a> DoctorJson<'a> {
    fn new(event: &'static str, timestamp: f64, report: &'a DoctorReport) -> Self {
        let probe = &report.probe;
        let env = &report.env;
        let ytdlp_detail = match report.ytdlp_age_days {
            Some(days) => format!("v{} ({days}d old)", report.yt_dlp_version.as_deref().unwrap_or("")),
            None => format!("v{}", report.yt_dlp_version.as_deref().unwrap_or("")),
        };
        let js_detail = probe.js_runtime.clone();
        let npm_detail = match (&env.npm_version, &env.npm_path) {
            (Some(version), Some(path)) => Some(format!("v{version} ({path})")),
            (Some(version), None) => Some(format!("v{version}")),
            (None, Some(path)) => Some(path.clone()),
            (None, None) => None,
        };
        let ejs_detail = probe
            .ytdlp_ejs_version
            .as_ref()
            .map(|version| format!("v{version}"));
        Self {
            event,
            timestamp,
            version: &report.oracle_version,
            ytdlp: YtdlpJson {
                version: report.yt_dlp_version.clone(),
                age_days: report.ytdlp_age_days,
                outdated: report.ytdlp_outdated,
                path: probe.yt_dlp_path.clone(),
            },
            ytdlp_ejs: VersionOnlyJson {
                version: probe.ytdlp_ejs_version.clone(),
            },
            rust_engine: RustEngineJson {
                available: false,
                version: None,
            },
            npm: NpmJson {
                version: env.npm_version.clone(),
                path: env.npm_path.clone(),
            },
            auth: AuthJson::new(&env.auth),
            platform_utilities: &env.platform_utilities,
            macos_utilities: &env.platform_utilities,
            core_tools: CoreToolsJson {
                httpx: ToolRow::ready("ready"),
                hf: ToolRow::ready("ready (Aria2 RPC)"),
                rust_engine: ToolRow {
                    status: "optional_missing".to_owned(),
                    detail: "not built (HTTP(S) falls back to --engine aria2)".to_owned(),
                    // The legacy PyO3 module was deleted in W0-1 and is never
                    // reintroduced, so there is no build command to recommend.
                    fix: None,
                },
                aria2c: path_row(probe.aria2c_path.as_deref()),
                ytdlp: status_detail_row(report.yt_dlp_version.is_some(), Some(&ytdlp_detail)),
                ytdlp_ejs: status_detail_row(
                    probe.ytdlp_ejs_version.is_some(),
                    ejs_detail.as_deref(),
                ),
                js_runtime: status_detail_row(js_detail.is_some(), js_detail.as_deref()),
                npm: status_detail_row(npm_detail.is_some(), npm_detail.as_deref()),
                ffmpeg: path_row(probe.ffmpeg_path.as_deref()),
                afconvert: path_row(probe.afconvert_path.as_deref()),
            },
            fixes: &env.fixes,
        }
    }
}

#[derive(Serialize)]
struct YtdlpJson {
    version: Option<String>,
    age_days: Option<i64>,
    outdated: bool,
    path: Option<String>,
}

#[derive(Serialize)]
struct VersionOnlyJson {
    version: Option<String>,
}

#[derive(Serialize)]
struct RustEngineJson {
    available: bool,
    version: Option<String>,
}

#[derive(Serialize)]
struct NpmJson {
    version: Option<String>,
    path: Option<String>,
}

#[derive(Serialize)]
struct ToolRow {
    status: String,
    detail: String,
    fix: Option<String>,
}

impl ToolRow {
    fn ready(detail: &str) -> Self {
        Self {
            status: "ready".to_owned(),
            detail: detail.to_owned(),
            fix: None,
        }
    }
}

#[derive(Serialize)]
struct CoreToolsJson {
    #[serde(rename = "GitHub PowerSuite (httpx)")]
    httpx: ToolRow,
    #[serde(rename = "Hugging Face Hub Engine")]
    hf: ToolRow,
    #[serde(rename = "Rust Engine (native rapids)")]
    rust_engine: ToolRow,
    #[serde(rename = "aria2c (legacy JSON-RPC fallback)")]
    aria2c: ToolRow,
    #[serde(rename = "yt-dlp (Media Stream Resolver)")]
    ytdlp: ToolRow,
    #[serde(rename = "yt-dlp-ejs (JS Challenge Pkg)")]
    ytdlp_ejs: ToolRow,
    #[serde(rename = "JS Runtime (for EJS)")]
    js_runtime: ToolRow,
    #[serde(rename = "npm (Node Package Manager)")]
    npm: ToolRow,
    #[serde(rename = "ffmpeg (AV Transcoder)")]
    ffmpeg: ToolRow,
    #[serde(rename = "afconvert (CoreAudio)")]
    afconvert: ToolRow,
}

#[derive(Serialize)]
struct AuthServiceJson<'a> {
    service: &'a str,
    label: &'a str,
    configured: bool,
    source: &'a str,
}

#[derive(Serialize)]
struct AuthJson<'a> {
    hf: AuthServiceJson<'a>,
    github: AuthServiceJson<'a>,
}

impl<'a> AuthJson<'a> {
    fn new(value: &'a serde_json::Value) -> Self {
        Self {
            hf: auth_service(value, "hf", "Hugging Face Hub"),
            github: auth_service(value, "github", "GitHub API"),
        }
    }
}

fn auth_service<'a>(
    value: &'a serde_json::Value,
    name: &'a str,
    label: &'a str,
) -> AuthServiceJson<'a> {
    let entry = value.get(name);
    let configured = entry
        .and_then(|entry| entry.get("configured"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let source = match entry
        .and_then(|entry| entry.get("source"))
        .and_then(serde_json::Value::as_str)
    {
        Some("env") => "env",
        Some("keychain") | Some("credential_manager") => "keychain",
        Some("cache_file") => "cache_file",
        Some("gh_cli") => "gh_cli",
        _ => "none",
    };
    AuthServiceJson { service: name, label, configured, source }
}

// ---- small helpers --------------------------------------------------------

fn bool_token(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn auth_source(auth: &serde_json::Value, service: &str) -> &'static str {
    match auth
        .get(service)
        .and_then(|entry| entry.get("source"))
        .and_then(serde_json::Value::as_str)
    {
        Some("env") => "env",
        Some("keychain") | Some("credential_manager") => "keychain",
        Some("cache_file") => "cache_file",
        Some("gh_cli") => "gh_cli",
        _ => "none",
    }
}

fn status_detail_row(available: bool, detail: Option<&str>) -> ToolRow {
    ToolRow {
        status: if available { "ready" } else { "missing" }.to_owned(),
        detail: detail.unwrap_or("not found").to_owned(),
        fix: None,
    }
}

fn path_row(path: Option<&str>) -> ToolRow {
    match path {
        Some(path) => ToolRow {
            status: "ready".to_owned(),
            detail: path.to_owned(),
            fix: None,
        },
        None => ToolRow {
            status: "optional_missing".to_owned(),
            detail: "not found".to_owned(),
            fix: None,
        },
    }
}

fn first_line(argv: &[&str]) -> Option<String> {
    let output = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

fn tool_path(name: &str, fallbacks: &[&str]) -> Option<String> {
    first_line(&["which", name]).or_else(|| {
        fallbacks
            .iter()
            .find(|path| Path::new(path).exists())
            .map(|path| (*path).to_owned())
    })
}

fn ytdlp_age_days(version: &str) -> Option<i64> {
    let mut parts = version.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let release = NaiveDate::from_ymd_opt(year, month, day)?;
    Some((chrono::Local::now().date_naive() - release).num_days())
}

/// Discover `yt-dlp-ejs` from `*.dist-info` on disk (no Python import).
fn discover_ytdlp_ejs_version() -> Option<String> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(override_path) = std::env::var_os("PARADL_MEDIA_PY") {
        roots.push(PathBuf::from(override_path));
    }
    // Compile-time crate dir and the running binary's ancestors cover both a
    // source checkout and an installed binary next to its managed venv.
    roots.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    if let Ok(exe) = std::env::current_exe() {
        roots.push(exe);
    }
    for root in roots {
        let mut candidate = root.as_path();
        for _ in 0..8 {
            let venv = candidate.join(".venv");
            if let Some(version) = dist_info_version(&venv) {
                return Some(version);
            }
            candidate = candidate.parent()?;
        }
    }
    None
}

fn dist_info_version(venv: &Path) -> Option<String> {
    let site = venv.join("lib");
    let entries = std::fs::read_dir(site).ok()?;
    for python_dir in entries.flatten() {
        let packages = python_dir.path().join("site-packages");
        let children = match std::fs::read_dir(&packages) {
            Ok(children) => children,
            Err(_) => continue,
        };
        for entry in children.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("yt_dlp_ejs-") && name.ends_with(".dist-info") {
                let version = name
                    .trim_start_matches("yt_dlp_ejs-")
                    .trim_end_matches(".dist-info");
                if !version.is_empty() {
                    return Some(version.to_owned());
                }
            }
        }
    }
    None
}

fn fixes_for(tools: &ToolProbe, env: &DoctorEnv) -> Vec<String> {
    let mut fixes = Vec::new();
    if auth_source(&env.auth, "hf") == "none" {
        fixes.push("paradl auth set hf  # or: export HF_TOKEN=<token>".to_owned());
    }
    if auth_source(&env.auth, "github") == "none" {
        fixes.push("paradl auth set github  # or: export GITHUB_TOKEN=<token>".to_owned());
    }
    if tools.yt_dlp_version.is_none() {
        fixes.push("install yt-dlp in the binary-managed media venv".to_owned());
    }
    fixes
}

/// `get_auth_status()` equivalent, using env → keychain → none.
fn auth_status() -> serde_json::Value {
    let (value, _emitted) = paradl_resilience::auth::auth_status(&OsEnv, &KeychainSecretStore)
        .unwrap_or_else(|_| (serde_json::Value::Object(serde_json::Map::new()), unreachable_event()));
    value
}

fn unreachable_event() -> paradl_resilience::Emitted {
    paradl_resilience::Emitted::from(paradl_events::Event::AuthStatus(
        paradl_events::AuthStatusEvent::new(0.0, serde_json::Value::Null),
    ))
}

fn platform_utilities() -> PlatformUtilities {
    fn row(name: &str, fallbacks: &[&str], fix: &str) -> UtilityRow {
        let path = tool_path(name, fallbacks);
        UtilityRow { available: path.is_some(), path, fix: fix.to_owned() }
    }
    PlatformUtilities {
        caffeinate: row("caffeinate", &["/usr/bin/caffeinate"], "Built-in macOS binary (/usr/bin/caffeinate)"),
        scutil: row("scutil", &["/usr/sbin/scutil"], "Built-in macOS binary (/usr/sbin/scutil)"),
        pmset: row("pmset", &["/usr/bin/pmset"], "Built-in macOS binary (/usr/bin/pmset)"),
        security: row("security", &["/usr/bin/security"], "Built-in macOS binary (/usr/bin/security)"),
        codesign: row("codesign", &["/usr/bin/codesign"], "Run 'xcode-select --install' to install macOS developer tools"),
        spctl: row("spctl", &["/usr/sbin/spctl"], "Built-in macOS binary (/usr/sbin/spctl)"),
        xattr: row("xattr", &["/usr/bin/xattr"], "Built-in macOS binary (/usr/bin/xattr)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_pinned() {
        assert_eq!(MEDIA_PROTOCOL, "paradl-media/1");
        assert_eq!(MEDIA_PACKAGE_VERSION, "0.1.0");
        assert_eq!(ORACLE_VERSION, "1.0.0");
    }

    #[test]
    fn with_tools_does_not_enable_python_probing() {
        let tools = ToolProbe {
            yt_dlp_version: Some("2026.10.26".into()),
            node_version: Some("v20.11.1".into()),
            ..ToolProbe::default()
        };
        let report = DoctorReport::collect(false).with_tools(&tools);
        assert!(!report.python_probed);
        assert!(report.render_human().contains("yt-dlp"));
        assert!(report.render_human().contains("2026.10.26"));
        assert!(report.render_human().contains("v20.11.1"));
    }
}
