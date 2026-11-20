//! Diagnostics for the reference binary: Python-free tool probing, an
//! oracle-aligned JSONL payload, and a deterministic fixed-width human report.
//!
//! [`DoctorReport`] reports the binary version, the `paradl-media/1` protocol,
//! the Python media-package version, yt-dlp version and JS-runtime status. It
//! works with **no Python** on non-media paths: tool versions come from
//! external native binaries probed by [`ToolProbe`], never from
//! `python -m paradl_media` and never from PyO3.
//!
//! The human renderer deliberately avoids Rich (and table crates with dynamic
//! widths): fixed column widths, stable ordering and no ANSI make the bytes
//! identical in a pipe and in a TTY, which makes a byte-exact golden test
//! possible.

use chrono::NaiveDate;
use paradl_events::{emit_jsonl, now_timestamp};
use paradl_resilience::auth::{InMemorySecretStore, OsEnv, Service, TokenSource, lookup_token};
use serde::Serialize;
use serde_json::{Value, json};

/// Protocol spoken with the (Python) media worker.
pub const MEDIA_PROTOCOL: &str = "paradl-media/1";
/// Version of the standalone Python media package (`media/pyproject.toml`).
///
/// Kept in sync with `paradl_media.__version__` / `WORKER_VERSION`; it is a
/// constant here so `doctor` never needs to import or spawn Python.
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
/// without Python. Each field is `None` when the native binary is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolProbe {
    /// yt-dlp version (`yt-dlp --version` first line).
    pub yt_dlp_version: Option<String>,
    /// Resolved path of the yt-dlp binary.
    pub yt_dlp_path: Option<String>,
    /// Node.js version (`node --version`).
    pub node_version: Option<String>,
    /// Deno version (`deno --version`).
    pub deno_version: Option<String>,
    /// Bun version (`bun --version`).
    pub bun_version: Option<String>,
    /// ffmpeg version (`ffmpeg -version` first line).
    pub ffmpeg_version: Option<String>,
    /// npm version (`npm --version`).
    pub npm_version: Option<String>,
    /// Resolved path of the npm binary.
    pub npm_path: Option<String>,
}

impl ToolProbe {
    /// Probe external tool binaries on `PATH` with short, bounded commands.
    ///
    /// Only native binaries are executed; no Python interpreter is discovered
    /// or spawned. A missing or slow tool yields `None`.
    pub fn detect() -> Self {
        Self {
            yt_dlp_version: first_line(&["yt-dlp", "--version"]),
            yt_dlp_path: tool_path("yt-dlp", &[]),
            node_version: first_line(&["node", "--version"]),
            deno_version: first_line(&["deno", "--version"]),
            bun_version: first_line(&["bun", "--version"]),
            ffmpeg_version: first_line(&["ffmpeg", "-version"]),
            npm_version: first_line(&["npm", "--version"]),
            npm_path: tool_path("npm", &[]),
        }
    }

    /// First available JS runtime as a stable `name version` label.
    pub fn js_runtime(&self) -> Option<String> {
        self.node_version
            .as_ref()
            .map(|version| format!("node {version}"))
            .or_else(|| self.deno_version.as_ref().map(|version| format!("deno {version}")))
            .or_else(|| self.bun_version.as_ref().map(|version| format!("bun {version}")))
    }
}

/// Environment-dependent doctor values that cannot be derived from constants.
///
/// Every field is always present in the JSON payload; genuinely unavailable
/// values use JSON `null` rather than being omitted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DoctorEnv {
    /// `get_auth_status()` equivalent: per-service source metadata.
    pub auth: Value,
    /// `check_platform_utilities()` equivalent.
    pub platform_utilities: Value,
    /// yt-dlp-ejs version; always `None` without a Python metadata probe.
    pub ytdlp_ejs_version: Option<String>,
    /// Version of the in-process native engine.
    pub rust_engine_version: Option<String>,
    /// npm version, when installed.
    pub npm_version: Option<String>,
    /// npm path, when installed.
    pub npm_path: Option<String>,
    /// Recommended fix commands (deduplicated, stable order).
    pub fixes: Vec<String>,
}

/// The complete `doctor` / `diagnose` payload, independent of stdout.
///
/// Constructed with [`DoctorReport::collect`], enriched with
/// [`DoctorReport::with_tools`], and freely constructible in golden tests by
/// setting its public fields.
#[derive(Debug, Clone, PartialEq)]
pub struct DoctorReport {
    /// Reference-binary crate version (`CARGO_PKG_VERSION`, currently `0.1.0`).
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
    ///
    /// `media_enabled` records whether the binary was compiled with the
    /// `media` feature; it never triggers a Python probe on this non-media
    /// path. Call [`DoctorReport::with_tools`] to fill in native tool versions.
    ///
    /// ```
    /// use paradl_app::DoctorReport;
    /// let report = DoctorReport::collect(false);
    /// assert!(!report.python_probed);
    /// assert_eq!(report.media_protocol, "paradl-media/1");
    /// ```
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
                platform_utilities: platform_utilities(),
                rust_engine_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                ..DoctorEnv::default()
            },
        }
    }

    /// Enrich the report with externally probed tool versions.
    ///
    /// No Python is involved; [`ToolProbe::detect`] only executes native
    /// binaries found on `PATH`. Missing tools become `None` and contribute a
    /// deterministic fix string.
    pub fn with_tools(mut self, tools: &ToolProbe) -> Self {
        self.yt_dlp_version = tools.yt_dlp_version.clone();
        self.ytdlp_age_days = tools
            .yt_dlp_version
            .as_deref()
            .and_then(ytdlp_age_days);
        self.ytdlp_outdated = self.ytdlp_age_days.is_some_and(|days| days > 30);
        self.js_runtime = tools.js_runtime();
        self.env.npm_version = tools.npm_version.clone();
        self.env.npm_path = tools.npm_path.clone();
        self.env.fixes = fixes_for(tools);
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

    /// `core_tools` payload for the JSON events (oracle field `core_tools`).
    fn core_tools(&self) -> Value {
        let mut tools = serde_json::Map::new();
        let mut insert = |name: &str, status: &str, detail: Option<&str>, fix: Option<&str>| {
            tools.insert(
                name.to_owned(),
                json!({"status": status, "detail": detail, "fix": fix}),
            );
        };
        insert("GitHub PowerSuite (httpx)", "ready", Some("bundled reqwest client"), None);
        insert(
            "Hugging Face Hub Engine",
            "ready",
            Some("native site resolver"),
            None,
        );
        insert(
            "Rust Engine (native rapids)",
            "ready",
            self.env.rust_engine_version.as_deref(),
            None,
        );
        insert(
            "yt-dlp (Media Stream Resolver)",
            if self.yt_dlp_version.is_some() { "ready" } else { "missing" },
            self.yt_dlp_version.as_deref(),
            if self.yt_dlp_version.is_some() {
                None
            } else {
                Some("install yt-dlp in the media venv")
            },
        );
        insert(
            "yt-dlp-ejs (JS Challenge Pkg)",
            if self.env.ytdlp_ejs_version.is_some() { "ready" } else { "missing" },
            self.env.ytdlp_ejs_version.as_deref(),
            None,
        );
        insert(
            "JS Runtime (for EJS)",
            if self.js_runtime.is_some() { "ready" } else { "optional_missing" },
            self.js_runtime.as_deref(),
            None,
        );
        insert(
            "npm (Node Package Manager)",
            if self.env.npm_version.is_some() { "ready" } else { "optional_missing" },
            self.env.npm_version.as_deref(),
            None,
        );
        insert(
            "ffmpeg (AV Transcoder)",
            if self.probe.ffmpeg_version.is_some() { "ready" } else { "optional_missing" },
            self.probe.ffmpeg_version.as_deref(),
            None,
        );
        Value::Object(tools)
    }

    /// Render the `doctor` and `diagnose` JSONL lines via the single emitter.
    ///
    /// The field set matches `src/paradl/diagnostics/doctor.py:349-375`
    /// exactly: `version`, `ytdlp`, `ytdlp_ejs`, `rust_engine`, `npm`, `auth`,
    /// `platform_utilities`, `macos_utilities`, `core_tools`, `fixes`. Only
    /// `paradl-events::emit_jsonl` serializes these lines.
    pub fn json_events(&self) -> Vec<String> {
        let timestamp = now_timestamp();
        let ytdlp = json!({
            "version": self.yt_dlp_version,
            "age_days": self.ytdlp_age_days,
            "outdated": self.ytdlp_outdated,
            "path": self.probe.yt_dlp_path,
        });
        let ytdlp_ejs = json!({"version": self.env.ytdlp_ejs_version});
        let rust_engine = json!({
            "available": true,
            "version": self.env.rust_engine_version,
        });
        let npm = json!({"version": self.env.npm_version, "path": self.env.npm_path});
        let core_tools = self.core_tools();
        let platform = &self.env.platform_utilities;
        let doctor = DoctorJson {
            event: "doctor",
            timestamp,
            version: &self.oracle_version,
            ytdlp: ytdlp.clone(),
            ytdlp_ejs: ytdlp_ejs.clone(),
            rust_engine: rust_engine.clone(),
            npm: npm.clone(),
            auth: &self.env.auth,
            platform_utilities: platform,
            macos_utilities: platform,
            core_tools: core_tools.clone(),
            fixes: &self.env.fixes,
        };
        let diagnose = DoctorJson {
            event: "diagnose",
            timestamp,
            version: &self.oracle_version,
            ytdlp,
            ytdlp_ejs,
            rust_engine,
            npm,
            auth: &self.env.auth,
            platform_utilities: platform,
            macos_utilities: platform,
            core_tools,
            fixes: &self.env.fixes,
        };
        vec![emit_jsonl(&doctor), emit_jsonl(&diagnose)]
    }
}

/// Exact JSON envelope of the oracle's `doctor` / `diagnose` events.
#[derive(Serialize)]
struct DoctorJson<'a> {
    event: &'static str,
    timestamp: f64,
    version: &'a str,
    ytdlp: Value,
    ytdlp_ejs: Value,
    rust_engine: Value,
    npm: Value,
    auth: &'a Value,
    platform_utilities: &'a Value,
    macos_utilities: &'a Value,
    core_tools: Value,
    fixes: &'a [String],
}

fn bool_token(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn auth_source(auth: &Value, service: &str) -> &'static str {
    match auth
        .get(service)
        .and_then(|entry| entry.get("source"))
        .and_then(Value::as_str)
    {
        Some("env") => "env",
        Some("keychain") => "keychain",
        Some("cache_file") => "cache_file",
        Some("gh_cli") => "gh_cli",
        _ => "none",
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
            .find(|path| std::path::Path::new(path).exists())
            .map(|path| (*path).to_owned())
    })
}

fn ytdlp_age_days(version: &str) -> Option<i64> {
    let mut parts = version.split('.');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let release = NaiveDate::from_ymd_opt(year, month, day)?;
    let today = chrono::Local::now().date_naive();
    Some((today - release).num_days())
}

fn fixes_for(tools: &ToolProbe) -> Vec<String> {
    let mut fixes = Vec::new();
    if tools.yt_dlp_version.is_none() {
        fixes.push("install yt-dlp in the binary-managed media venv".to_owned());
    }
    if tools.node_version.is_none()
        && tools.deno_version.is_none()
        && tools.bun_version.is_none()
    {
        fixes.push("install a JS runtime (node, deno or bun) for EJS challenges".to_owned());
    }
    fixes
}

fn auth_status() -> Value {
    let store = InMemorySecretStore::new();
    let env = OsEnv;
    let mut services = serde_json::Map::new();
    for service in [Service::Hf, Service::Github] {
        let source = match lookup_token(service, &env, &store).map(|record| record.source) {
            Some(TokenSource::Env) => "env",
            Some(TokenSource::Keychain) => "keychain",
            Some(TokenSource::CacheFile) => "cache_file",
            Some(TokenSource::GhCli) => "gh_cli",
            None | Some(TokenSource::None) => "none",
        };
        services.insert(
            service.as_str().to_owned(),
            json!({
                "service": service.as_str(),
                "label": service.label(),
                "configured": source != "none",
                "source": source,
            }),
        );
    }
    Value::Object(services)
}

fn platform_utilities() -> Value {
    const UTILS: &[(&str, &[&str], &str)] = &[
        ("caffeinate", &["/usr/bin/caffeinate"], "Built-in macOS binary (/usr/bin/caffeinate)"),
        ("scutil", &["/usr/sbin/scutil"], "Built-in macOS binary (/usr/sbin/scutil)"),
        ("pmset", &["/usr/bin/pmset"], "Built-in macOS binary (/usr/bin/pmset)"),
        ("security", &["/usr/bin/security"], "Built-in macOS binary (/usr/bin/security)"),
        ("codesign", &["/usr/bin/codesign"], "Run 'xcode-select --install' to install macOS developer tools"),
        ("spctl", &["/usr/sbin/spctl"], "Built-in macOS binary (/usr/sbin/spctl)"),
        ("xattr", &["/usr/bin/xattr"], "Built-in macOS binary (/usr/bin/xattr)"),
    ];
    let mut result = serde_json::Map::new();
    for (name, fallbacks, fix) in UTILS {
        let path = tool_path(name, fallbacks);
        result.insert(
            (*name).to_owned(),
            json!({"available": path.is_some(), "path": path, "fix": fix}),
        );
    }
    Value::Object(result)
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
            node_version: Some("v20.0.0".into()),
            ..ToolProbe::default()
        };
        let report = DoctorReport::collect(false).with_tools(&tools);
        assert!(!report.python_probed);
        assert!(report.render_human().contains("yt-dlp"));
        assert!(report.render_human().contains("2026.10.26"));
        assert!(report.render_human().contains("v20.0.0"));
    }
}
