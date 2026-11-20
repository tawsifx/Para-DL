//! Thin diagnostic orchestration for the reference binary.
//!
//! [`DoctorReport`] reports the binary version, the `paradl-media/1` protocol,
//! the Python media-package version, the yt-dlp version and JS-runtime status.
//! It works with **no Python** on non-media paths: tool versions come from
//! external binaries probed by [`ToolProbe`], never from `python -m paradl_media`
//! and never from PyO3. A missing tool degrades to `None` instead of failing.

use paradl_events::{DiagnoseEvent, DoctorEvent, emit_jsonl, now_timestamp};
use serde_json::{Value, json};

/// Protocol spoken with the (Python) media worker.
pub const MEDIA_PROTOCOL: &str = "paradl-media/1";
/// Version of the standalone Python media package (`media/pyproject.toml`).
///
/// Kept in sync with `paradl_media.__version__` / `WORKER_VERSION`; it is a
/// constant here so `doctor` never needs to import or spawn Python.
pub const MEDIA_PACKAGE_VERSION: &str = "0.1.0";

/// Versions of the external tools `doctor` reports, gathered without Python.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolProbe {
    /// yt-dlp version (`yt-dlp --version` first line), when installed.
    pub yt_dlp_version: Option<String>,
    /// JS runtime label (`node vX.Y.Z`, `deno X.Y.Z`, …), when installed.
    pub js_runtime: Option<String>,
}

impl ToolProbe {
    /// Probe external tool binaries on `PATH` with short, bounded commands.
    ///
    /// Only native binaries are executed; no Python interpreter is discovered
    /// or spawned. A missing or slow tool yields `None`.
    pub fn detect() -> Self {
        Self {
            yt_dlp_version: first_line(&["yt-dlp", "--version"]),
            js_runtime: ["node", "deno", "bun"]
                .iter()
                .find_map(|runtime| {
                    first_line(&[runtime, "--version"])
                        .map(|version| format!("{runtime} {version}"))
                }),
        }
    }
}

/// Run `argv` and return its first non-empty stdout line, or `None` on error.
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

/// The complete `doctor` / `diagnose` payload, independent of stdout.
///
/// Constructed with [`DoctorReport::collect`] for hermetic callers, then
/// optionally enriched with [`DoctorReport::with_tools`]. The binary uses
/// [`ToolProbe::detect`] to fill in external tool versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// Reference-binary version (`CARGO_PKG_VERSION` = `0.1.0`).
    pub binary_version: String,
    /// Media protocol identifier (`paradl-media/1`).
    pub media_protocol: String,
    /// Python media-package version (`0.1.0`).
    pub media_package_version: String,
    /// yt-dlp version, when the external binary is installed.
    pub yt_dlp_version: Option<String>,
    /// JS-runtime label, when an external runtime is installed.
    pub js_runtime: Option<String>,
    /// Whether the `media` feature is compiled in.
    pub media_enabled: bool,
    /// Always `false`: non-media diagnostics never probe or spawn Python.
    pub python_probed: bool,
}

impl DoctorReport {
    /// Build a report from compile-time constants only (no external probes).
    ///
    /// `media_enabled` records whether the binary was compiled with the `media`
    /// feature; it never triggers a Python probe on this non-media path.
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
            media_protocol: MEDIA_PROTOCOL.to_owned(),
            media_package_version: MEDIA_PACKAGE_VERSION.to_owned(),
            yt_dlp_version: None,
            js_runtime: None,
            media_enabled,
            python_probed: false,
        }
    }

    /// Enrich the report with externally probed tool versions.
    ///
    /// No Python is involved; [`ToolProbe::detect`] only executes native
    /// binaries found on `PATH`.
    pub fn with_tools(mut self, tools: &ToolProbe) -> Self {
        self.yt_dlp_version = tools.yt_dlp_version.clone();
        self.js_runtime = tools.js_runtime.clone();
        self
    }

    /// Render the stable, single-line human diagnostic.
    ///
    /// Unlike the Python Rich tables this output does not depend on terminal
    /// width, so it is byte-stable across environments. It reports every field
    /// the ruling requires and omits any `python=` field on non-media paths.
    pub fn render_human(&self) -> String {
        let yt_dlp = self.yt_dlp_version.as_deref().unwrap_or("not found");
        let js_runtime = self.js_runtime.as_deref().unwrap_or("not found");
        format!(
            "ready version={} protocol={} media_package={} yt_dlp={} js_runtime={} media_enabled={}",
            self.binary_version,
            self.media_protocol,
            self.media_package_version,
            yt_dlp,
            js_runtime,
            self.media_enabled,
        )
    }

    /// `core_tools` payload used by the JSONL `doctor` event (E28).
    fn core_tools(&self) -> Value {
        json!({
            "version": {"status": "ready", "detail": self.binary_version},
            "media_protocol": {"status": "ready", "detail": self.media_protocol},
            "media_package": {"status": "ready", "detail": self.media_package_version},
            "yt-dlp": {
                "status": if self.yt_dlp_version.is_some() { "ready" } else { "optional_missing" },
                "detail": self.yt_dlp_version.as_deref().unwrap_or("not found"),
            },
            "js_runtime": {
                "status": if self.js_runtime.is_some() { "ready" } else { "optional_missing" },
                "detail": self.js_runtime.as_deref().unwrap_or("not found"),
            },
        })
    }

    /// Render the `doctor` and `diagnose` JSONL lines via the single emitter.
    ///
    /// The Python doctor emits both events with the same payload (backward
    /// compatibility alias, `doctor.py:374-375`); this preserves that contract
    /// using `paradl-events` as the only JSON serializer.
    pub fn json_events(&self) -> Vec<String> {
        let timestamp = now_timestamp();
        let version = Some(self.binary_version.clone());
        let ytdlp = self.yt_dlp_version.clone();
        let core_tools = Some(self.core_tools());
        let fixes = Some(Value::Array(Vec::new()));
        let doctor = DoctorEvent::new(
            timestamp,
            version.clone(),
            ytdlp.clone(),
            None,
            None,
            None,
            core_tools.clone(),
            fixes.clone(),
        );
        let diagnose = DiagnoseEvent::new(
            timestamp, version, ytdlp, None, None, None, core_tools, fixes,
        );
        vec![emit_jsonl(&doctor), emit_jsonl(&diagnose)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_are_pinned() {
        assert_eq!(MEDIA_PROTOCOL, "paradl-media/1");
        assert_eq!(MEDIA_PACKAGE_VERSION, "0.1.0");
    }

    #[test]
    fn with_tools_does_not_enable_python_probing() {
        let report = DoctorReport::collect(false).with_tools(&ToolProbe {
            yt_dlp_version: Some("2026.10.26".into()),
            js_runtime: Some("node v20.0.0".into()),
        });
        assert!(!report.python_probed);
        assert!(report.render_human().contains("yt_dlp=2026.10.26"));
        assert!(report.render_human().contains("js_runtime=node v20.0.0"));
    }
}
