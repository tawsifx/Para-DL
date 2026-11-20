//! # paradl-cli
//!
//! The clap-based command-line surface of the paraDL framework. This crate is a
//! faithful port of `src/paradl/cli/{parser,main,dispatch}.py` and the standalone
//! `paradl.throttle.cli` parser:
//!
//! - [`Cli`] parses all ~60 flags and the throttle flags with clap derive.
//! - [`HELP_TEXT`] / [`THROTTLE_HELP_TEXT`] are the *byte-exact* oracle `--help`
//!   outputs, installed via `clap::Command::override_help`.
//! - [`VERSION_TEXT`] is the exact oracle `--version` line.
//! - [`Command`] plus [`Dispatch`] are the typed routing seam used by `paradl-app`
//!   (W4-1); command bodies deliberately live in that crate.
//! - Usage errors become [`Error::Usage`], which maps to [`ExitCode::Usage`] (`2`).
//!
//! ```
//! use paradl_cli::{Cli, Command, Dispatch, ExitCode, NoopDispatch, ParseOutcome};
//!
//! let outcome = Cli::parse_command_line(vec![
//!     "https://example.com/file.iso".into(),
//!     "--engine".into(),
//!     "rust".into(),
//!     "--no-caffeinate".into(),
//! ])
//! .unwrap();
//! let ParseOutcome::Command(cli) = outcome else { unreachable!() };
//! assert_eq!(cli.connections, 16);
//! assert!(!cli.caffeinate);
//!
//! let cmd = Command::from_cli(&cli).unwrap();
//! assert!(matches!(cmd, Command::Download(_)));
//! assert_eq!(NoopDispatch.handle(&cmd, &cli), ExitCode::Ok);
//! ```

#![warn(missing_docs)]

pub mod dispatch;
pub mod parser;
pub mod throttle;

pub use dispatch::{
    AuthAction, AuthCommand, BenchCommand, BrewAction, BrewCommand, CLI_ENV_VARS, CleanCommand, Command,
    ConfigAction, ConfigCommand, Dispatch, ExtractCommand, HashCommand, HistoryCommand,
    InstallCommand, InfoCommand, NetprobeCommand, NoopDispatch, PipAction, PipCommand,
    ProfilesAction, ProfilesCommand, QueueAction, QueueCommand, ResumeCommand, ServerCommand,
    SUBCOMMANDS, ThrottleCommand, USAGE_BENCH, USAGE_BREW, USAGE_BREW_CMD, USAGE_BREW_MISSING,
    USAGE_EXTRACT, USAGE_HASH, USAGE_HISTORY_LIMIT, USAGE_INFO, USAGE_INSTALL, USAGE_NETPROBE,
    USAGE_PIP, USAGE_PIP_CMD, dispatch,
};
pub use parser::{
    AudioCodec, Cli, Engine, HELP_TEXT, HistoryType, ParseOutcome, Quality, VERSION_TEXT,
};
pub use throttle::{ThrottleCli, ThrottleSubcommand, THROTTLE_HELP_TEXT};

// Re-export the shared error/exit-code model so downstream crates only need this
// dependency to route parse results.
pub use paradl_core::{Error, ExitCode};
