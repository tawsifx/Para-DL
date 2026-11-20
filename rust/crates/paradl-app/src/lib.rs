//! # paradl-app
//!
//! Application-level orchestration for the `paradl` reference binary. This
//! crate is the thin integration seam that wires every framework crate behind
//! the byte-exact [`paradl_cli`] parser:
//!
//! | Surface | Owning crate | Adapter in this crate |
//! |---|---|---|
//! | flags / help / errors / dispatch | `paradl-cli` | [`App`] implements `Dispatch` |
//! | models, config, paths, `Error`/`ExitCode` | `paradl-core` | config / path commands |
//! | resolve, route, probes, ISP | `paradl-net` | download / info / netprobe |
//! | planner, transfer, sidecars | `paradl-engine` | transfer command |
//! | history v4, `queue_jobs` | `paradl-store` | history / queue commands |
//! | v1/v2 watchdog | `paradl-watchdog` | [`handoffs::watchdog_config_for`] |
//! | classifier, reports | `paradl-throttle` | throttle command |
//! | checksums, archives, ADB | `paradl-postproc` | hash / extract commands |
//! | metadata + keychain | `paradl-platform` | auth command |
//! | one-shot media worker | `paradl-media` | media route (`media` feature) |
//! | queue, plist, hooks, auth sources | `paradl-resilience` | queue / launchd commands |
//! | single JSONL emitter | `paradl-events` | `App::publish` (private) |
//! | progress / TUI | `paradl-ui` | human summaries (`tui` feature) |
//! | axum API | `paradl-api` | server command (`api` feature) |
//!
//! ## No Python on non-media paths
//!
//! [`DoctorReport`] reports the binary version, the `paradl-media/1` protocol,
//! the Python media-package version, yt-dlp version and JS-runtime status using
//! only compile-time constants and short probes of native binaries. The Python
//! worker is discovered and spawned **only** for a media route when the `media`
//! feature is enabled.
//!
//! ```
//! use paradl_app::{App, DoctorReport};
//! use paradl_cli::{Cli, Command, Dispatch, ParseOutcome};
//!
//! let report = DoctorReport::collect(false);
//! assert!(!report.python_probed);
//! let cli = match Cli::parse_command_line(vec!["doctor".into()]) {
//!     Ok(ParseOutcome::Command(cli)) => cli,
//!     _ => unreachable!(),
//! };
//! let command = Command::from_cli(&cli).unwrap();
//! assert_eq!(App::new_offline().handle(&command, &cli), paradl_cli::ExitCode::Ok);
//! ```

#![warn(missing_docs)]

pub mod app;
pub mod diagnostics;
pub mod handoffs;

pub use app::{ApiBridge, ApiResolve, App, Capture, OutputSink, TransferSetup};
pub use diagnostics::{DoctorEnv, DoctorReport, MEDIA_PACKAGE_VERSION, MEDIA_PROTOCOL, ToolProbe};
