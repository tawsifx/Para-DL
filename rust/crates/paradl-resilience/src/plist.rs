//! Deterministic launchd `LaunchAgent` plist generation.
//!
//! Port of `src/paradl/resilience/queue.py:755-813` and the throttle sampler
//! generator. The XML is sorted-key (`plistlib.dumps(..., sort_keys=True)`),
//! written to disk but **never loaded** with `launchctl`. The resulting event
//! is a typed `launchd_plist_generated` value; the raw `json.dumps` at
//! `throttle/cli.py:296-308` (E58) must use the sibling typed event in
//! `paradl-events` instead.
//!
//! ```
//! use std::collections::BTreeMap;
//! use paradl_resilience::plist::{PlistConfig, PlistStart, generate_launchd_plist};
//! # let dir = tempfile::tempdir()?;
//! let config = PlistConfig {
//!     label: "com.example.worker".into(),
//!     program_arguments: vec!["/usr/local/bin/paradl".into(), "queue".into(), "run".into()],
//!     working_directory: dir.path().to_path_buf(),
//!     environment_variables: BTreeMap::new(),
//!     logs_dir: dir.path().join("logs"),
//!     start: PlistStart::Interval(900),
//! };
//! let out = generate_launchd_plist(&config, Some(dir.path().to_path_buf()))?;
//! assert_eq!(out.lint, "OK");
//! # Ok::<(), paradl_core::Error>(())
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Timelike};
use paradl_events::{Event, LaunchdPlistGeneratedEvent};
use paradl_core::Error;

use crate::{Emitted, Result, now};

/// Start policy for the generated agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlistStart {
    /// `StartInterval` in seconds (minimum 60, matching the oracle).
    Interval(u64),
    /// `StartCalendarInterval` with an hour/minute pair.
    Calendar {
        /// Hour of day (0–23).
        hour: u32,
        /// Minute of hour (0–59).
        minute: u32,
    },
}

/// Inputs for plist generation.
#[derive(Debug, Clone, PartialEq)]
pub struct PlistConfig {
    /// LaunchAgent label.
    pub label: String,
    /// `ProgramArguments`; in the Rust framework the first item is the native binary.
    pub program_arguments: Vec<String>,
    /// `WorkingDirectory`.
    pub working_directory: PathBuf,
    /// `EnvironmentVariables` (e.g. `PARADL_HOME`, `PATH`).
    pub environment_variables: BTreeMap<String, String>,
    /// Directory for `StandardOutPath` / `StandardErrorPath`.
    pub logs_dir: PathBuf,
    /// Schedule mode.
    pub start: PlistStart,
}

/// Generated plist artifact and its typed event.
#[derive(Debug, Clone, PartialEq)]
pub struct PlistOutput {
    /// Destination path.
    pub path: PathBuf,
    /// Exact XML bytes as UTF-8.
    pub xml: String,
    /// Validation message (`"OK"` on success).
    pub lint: String,
    /// Typed `launchd_plist_generated` event.
    pub emitted: Emitted,
}

/// Parse `HH:MM` into `(hour, minute)`; rejects out-of-range values.
pub fn parse_start_at_time(value: &str) -> Option<(u32, u32)> {
    let (hour, minute) = value.trim().split_once(':')?;
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour < 24 && minute < 60 {
        Some((hour, minute))
    } else {
        None
    }
}

/// Whether a `--at HH:MM` schedule has arrived.
pub fn is_schedule_due(start_at: &str, now: &DateTime<Local>) -> bool {
    let Some((hour, minute)) = parse_start_at_time(start_at) else {
        return true;
    };
    (now.hour(), now.minute()) >= (hour, minute)
}

fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn string_line(indent: usize, key: &str, value: &str) -> String {
    format!("{}<key>{}</key>\n{}<string>{}</string>\n", "\t".repeat(indent), xml_escape(key), "\t".repeat(indent), xml_escape(value))
}

fn integer_line(indent: usize, key: &str, value: u64) -> String {
    format!("{}<key>{}</key>\n{}<integer>{}</integer>\n", "\t".repeat(indent), xml_escape(key), "\t".repeat(indent), value)
}

/// Render sorted-key XML plist matching `plistlib.FMT_XML`.
pub fn render_plist_xml(config: &PlistConfig) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );

    // Keys are emitted in plistlib's sorted order.
    xml.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
    for (key, value) in &config.environment_variables {
        let _ = write!(xml, "{}", string_line(2, key, value));
    }
    xml.push_str("\t</dict>\n");
    let _ = write!(xml, "{}", string_line(1, "Label", &config.label));
    xml.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    for argument in &config.program_arguments {
        let _ = writeln!(xml, "\t\t<string>{}</string>", xml_escape(argument));
    }
    xml.push_str("\t</array>\n");
    xml.push_str("\t<key>RunAtLoad</key>\n\t<true/>\n");
    let _ = write!(
        xml,
        "{}",
        string_line(
            1,
            "StandardErrorPath",
            &config.logs_dir.join("queue.err.log").to_string_lossy(),
        )
    );
    let _ = write!(
        xml,
        "{}",
        string_line(
            1,
            "StandardOutPath",
            &config.logs_dir.join("queue.out.log").to_string_lossy(),
        )
    );
    match config.start {
        PlistStart::Interval(seconds) => {
            let _ = write!(xml, "{}", integer_line(1, "StartInterval", seconds.max(60)));
        }
        PlistStart::Calendar { hour, minute } => {
            xml.push_str("\t<key>StartCalendarInterval</key>\n\t<dict>\n");
            let _ = write!(xml, "{}", integer_line(2, "Hour", u64::from(hour)));
            let _ = write!(xml, "{}", integer_line(2, "Minute", u64::from(minute)));
            xml.push_str("\t</dict>\n");
        }
    }
    let _ = write!(
        xml,
        "{}",
        string_line(
            1,
            "WorkingDirectory",
            &config.working_directory.to_string_lossy(),
        )
    );
    let _ = write!(xml, "</dict>\n</plist>\n");
    xml
}

/// Lightweight structural validation (no external `plutil` dependency).
pub fn validate_plist_xml(xml: &str) -> std::result::Result<String, Error> {
    for required in [
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<plist version=\"1.0\">",
        "<key>Label</key>",
        "<key>ProgramArguments</key>",
        "<key>RunAtLoad</key>",
    ] {
        if !xml.contains(required) {
            return Err(Error::failure(format!(
                "generated plist is missing required element: {required}"
            )));
        }
    }
    Ok("OK".to_string())
}

/// Write a launchd plist and emit `launchd_plist_generated` (E40).
pub fn generate_launchd_plist(config: &PlistConfig, dest: Option<PathBuf>) -> Result<PlistOutput> {
    let out_plist: PathBuf = match dest {
        Some(path) if path.is_dir() => path.join(format!("{}.plist", config.label)),
        Some(path) => path,
        None => std::env::current_dir()
            .map_err(Error::from)?
            .join(format!("{}.plist", config.label)),
    };
    if let Some(parent) = out_plist.parent() {
        std::fs::create_dir_all(parent).map_err(Error::from)?;
    }
    let xml = render_plist_xml(config);
    let lint = validate_plist_xml(&xml)?;
    std::fs::write(&out_plist, xml.as_bytes()).map_err(Error::from)?;

    let emitted = Emitted::new(Event::LaunchdPlistGenerated(LaunchdPlistGeneratedEvent::new(
        now(),
        out_plist.to_string_lossy().into_owned(),
        config.label.clone(),
        config.logs_dir.to_string_lossy().into_owned(),
        lint.clone(),
    )));
    Ok(PlistOutput {
        path: out_plist,
        xml,
        lint,
        emitted,
    })
}

/// Typed event for the E58 fix: replaces the raw `json.dumps` in the throttle CLI.
pub fn throttle_plist_event(path: impl Into<String>, label: impl Into<String>, lint: impl Into<String>) -> Emitted {
    Emitted::new(Event::ThrottleLaunchdPlistGenerated(
        paradl_events::ThrottleLaunchdPlistGeneratedEvent::new(
            now(),
            path.into(),
            label.into(),
            lint.into(),
        ),
    ))
}

#[allow(dead_code)]
fn _path_hint(_path: &Path) {}
