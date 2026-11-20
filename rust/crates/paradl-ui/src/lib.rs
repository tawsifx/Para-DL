//! # paradl-ui
//!
//! Presentation layer for paraDL. It **consumes** typed values from
//! [`paradl_events`] and renders them as plain text, or — with the `tui`
//! feature — as an interactive [`ratatui`](https://docs.rs/ratatui) screen.
//!
//! ## Single-emitter rule
//!
//! This crate never writes JSONL. [`paradl_events::emit_jsonl`] is the
//! framework's only emitter; a chatty media worker can therefore never
//! corrupt `--json` output by rendering through this crate. Everything here is
//! read-only with respect to events.
//!
//! ## Default build
//!
//! The default build needs no TTY and no system tools: [`ProgressDisplay`] and
//! [`event_summary`] are pure string renderers. The interactive terminal app
//! is opt-in behind the `tui` feature and is exposed as the `tui` module.
//!
//! ```
//! use paradl_events::{Event, ProgressEvent};
//! use paradl_ui::{ProgressDisplay, event_summary};
//!
//! let event = Event::Progress(ProgressEvent::new(
//!     1.0, "demo.bin".into(), 512, Some(1024), Some(50.0), Some(2048.0),
//!     Some(0.5), Some(4), None, Some("Active".into()), None, None,
//! ));
//! let display = ProgressDisplay::new();
//! assert_eq!(
//!     event_summary(&event),
//!     display.simple_line(&match event { Event::Progress(ref p) => p.clone(), _ => unreachable!() }),
//! );
//! ```
//!
//! Fallible entry points (notably `tui::run_tui`) return
//! [`Error`], whose [`ExitCode`] mapping is owned by `paradl-core`.

#![warn(missing_docs)]

mod event;
mod format;
mod progress;

pub use event::event_summary;
pub use format::{format_size, format_speed};
pub use progress::ProgressDisplay;

/// The framework's single public error type, re-exported for convenience.
pub use paradl_core::Error;
/// Process exit codes (`0/1/2/3/4/130`), re-exported for convenience.
pub use paradl_core::ExitCode;

/// Result alias used by fallible `paradl-ui` entry points.
pub type Result<T> = std::result::Result<T, Error>;

/// Feature-gated interactive terminal UI driven by typed events.
#[cfg(feature = "tui")]
pub mod tui;
