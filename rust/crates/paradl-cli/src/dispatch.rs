//! Typed commands and the dispatch seam handed to `paradl-app` (W4-1).
//!
//! [`Command::from_cli`] reproduces the Python `run_cli` disambiguation: the first
//! positional `target`, when it is in [`SUBCOMMANDS`], selects a subcommand and the
//! remaining positionals become its arguments; anything else is a plain download.
//! Missing subcommand arguments produce [`Error::Usage`] carrying the *exact*
//! `print_plain("  [error]Usage: …[/error]")` string from `dispatch.py` and therefore
//! map to exit code `2`.
//!
//! Command bodies deliberately live in W4-1. [`Dispatch`] is the trait `paradl-app`
//! implements; this crate only owns parsing and routing.
//!
//! ```
//! use paradl_cli::{Cli, Command, Dispatch, ExitCode};
//! let cli = Cli::try_parse_from(["info", "https://example.com/f.zip"]).unwrap();
//! let cmd = Command::from_cli(&cli).unwrap();
//! assert!(matches!(cmd, Command::Info(_)));
//! struct Noop;
//! impl Dispatch for Noop {
//!     fn handle(&mut self, _: &Command, _: &Cli) -> ExitCode { ExitCode::Ok }
//! }
//! assert_eq!(Noop.handle(&cmd, &cli), ExitCode::Ok);
//! ```

use crate::{Cli, Error, ExitCode, ThrottleCli};

/// Positional tokens the Python dispatcher treats as subcommands (`parser.py:34`).
pub const SUBCOMMANDS: &[&str] = &[
    "auth", "doctor", "install", "info", "hash", "extract", "history", "clean", "config",
    "diagnose", "netprobe", "bench", "profiles", "queue", "resume", "server", "tui", "throttle",
    "pip", "pypi", "brew",
];

/// `auth` action (`dispatch.py:170-175`; bare `auth` defaults to `status`, C21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAction {
    /// Store a token for `hf` or `github`.
    Set,
    /// Report token sources without printing secrets.
    Status,
    /// Delete a stored token.
    Remove,
}

/// `config` action (`dispatch.py:159-166`; bare `config` defaults to `show`, C45).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigAction {
    /// Show the current configuration.
    Show,
    /// Set one key/value pair.
    Set,
    /// Reset the whole configuration.
    Reset,
    /// Clear learned host strategies.
    ResetHosts,
}

/// `profiles` action (`dispatch.py:192-198`; bare `profiles` defaults to `list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfilesAction {
    /// List learned profiles.
    List,
    /// Reset profiles for one host.
    Reset,
}

/// `queue` action (C13; bare `queue` defaults to `list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    /// Add a job.
    Add,
    /// List jobs.
    List,
    /// Pause jobs.
    Pause,
    /// Resume jobs.
    Resume,
    /// Remove jobs.
    Remove,
    /// Run the foreground queue runner.
    Run,
    /// Generate a LaunchAgent plist.
    LaunchdPlist,
}

/// `pip`/`pypi` action (C23).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipAction {
    /// Install a package.
    Install,
    /// Upgrade a package.
    Upgrade,
    /// Treat remaining positionals as `pip:` targets.
    Targets,
}

/// `brew` action (C24-C26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewAction {
    /// Install a formula.
    Install,
    /// Update the formulae catalog.
    Update,
    /// Upgrade installed formulae.
    Upgrade,
    /// Treat remaining positionals as `brew:` targets.
    Targets,
}

/// Typed payload for `auth <action> [service]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthCommand {
    /// The auth action.
    pub action: AuthAction,
    /// `hf` or `github`, when required.
    pub service: Option<String>,
}

/// Typed payload for `install <target>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCommand {
    /// `owner/repo` release target.
    pub target: String,
}

/// Typed payload for `info <target>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoCommand {
    /// Target to inspect.
    pub target: String,
}

/// Typed payload for `hash <file>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashCommand {
    /// File to hash.
    pub file: String,
}

/// Typed payload for `extract <archive> [dest_dir]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractCommand {
    /// Archive to extract.
    pub archive: String,
    /// Optional destination directory.
    pub dest_dir: Option<String>,
}

/// Typed payload for `history [limit]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCommand {
    /// Optional entry limit (overrides `--history-limit`).
    pub limit: Option<i64>,
}

/// Typed payload for `clean [dir]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanCommand {
    /// Optional directory to clean.
    pub dir: Option<String>,
}

/// Typed payload for `config [show|set|reset|reset-hosts]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigCommand {
    /// The config action.
    pub action: ConfigAction,
    /// Key for `set`.
    pub key: Option<String>,
    /// Value for `set`.
    pub value: Option<String>,
}

/// Typed payload for `netprobe <url>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetprobeCommand {
    /// URL to probe.
    pub url: String,
}

/// Typed payload for `bench <url>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchCommand {
    /// URL to benchmark.
    pub url: String,
}

/// Typed payload for `profiles [reset <host>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilesCommand {
    /// The profiles action.
    pub action: ProfilesAction,
    /// Host for `reset`.
    pub host: Option<String>,
}

/// Typed payload for `queue <action> …`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueCommand {
    /// The queue action.
    pub action: QueueAction,
    /// Raw remaining arguments (job ids, targets, flags) for W4-1.
    pub args: Vec<String>,
}

/// Typed payload for `resume --all`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCommand {
    /// Raw arguments forwarded to `handle_resume_command`.
    pub args: Vec<String>,
}

/// Typed payload for `server`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    /// Bind host.
    pub host: String,
    /// Bind port.
    pub port: i64,
    /// Auto-reload flag.
    pub reload: bool,
}

/// Typed payload for `pip`/`pypi`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipCommand {
    /// Whether this was the `pypi` spelling.
    pub alias_pypi: bool,
    /// The pip action.
    pub action: PipAction,
    /// Packages or `pip:` target strings.
    pub packages: Vec<String>,
}

/// Typed payload for `brew`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewCommand {
    /// The brew action.
    pub action: BrewAction,
    /// Formula name, when required.
    pub formula: Option<String>,
}

/// Typed payload for `throttle …`.
#[derive(Debug, Clone, PartialEq)]
pub struct ThrottleCommand {
    /// Parsed standalone throttle CLI namespace.
    pub options: ThrottleCli,
}

/// A fully typed command ready for `paradl-app` to execute.
///
/// The size difference is intentional: `Download` carries the complete flat CLI
/// namespace so downstream code never needs a second parse.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Plain download(s); carries the full CLI so all transfer flags are available.
    Download(Cli),
    /// `auth` subcommand.
    Auth(AuthCommand),
    /// `doctor` subcommand.
    Doctor,
    /// `install` subcommand.
    Install(InstallCommand),
    /// `info` subcommand.
    Info(InfoCommand),
    /// `hash` subcommand.
    Hash(HashCommand),
    /// `extract` subcommand.
    Extract(ExtractCommand),
    /// `history` subcommand.
    History(HistoryCommand),
    /// `clean` subcommand.
    Clean(CleanCommand),
    /// `config` subcommand.
    Config(ConfigCommand),
    /// `netprobe` subcommand.
    Netprobe(NetprobeCommand),
    /// `bench` subcommand.
    Bench(BenchCommand),
    /// `profiles` subcommand.
    Profiles(ProfilesCommand),
    /// `queue` subcommand.
    Queue(QueueCommand),
    /// `resume` subcommand.
    Resume(ResumeCommand),
    /// `server` subcommand.
    Server(ServerCommand),
    /// `tui` subcommand.
    Tui,
    /// `throttle` subcommand.
    Throttle(ThrottleCommand),
    /// `pip`/`pypi` subcommand.
    Pip(PipCommand),
    /// `brew` subcommand.
    Brew(BrewCommand),
    /// `--version` fast path.
    Version,
}

impl Command {
    /// Classify a parsed CLI into a typed command.
    ///
    /// Returns [`Error::Usage`] (exit code `2`) for a recognised subcommand that is
    /// missing required positional arguments, using the oracle's exact message.
    pub fn from_cli(cli: &Cli) -> Result<Self, Error> {
        if cli.version {
            return Ok(Self::Version);
        }

        let targets = &cli.targets;
        let Some(first) = targets.first() else {
            if let Some(file) = &cli.hash_file {
                return Ok(Self::Hash(HashCommand { file: file.clone() }));
            }
            return Ok(Self::Download(cli.clone()));
        };

        let sub = first.to_ascii_lowercase();
        if !SUBCOMMANDS.contains(&sub.as_str()) {
            return Ok(Self::Download(cli.clone()));
        }
        let rest = &targets[1..];

        match sub.as_str() {
            "install" => {
                let Some(target) = rest.first() else {
                    return Err(usage(USAGE_INSTALL));
                };
                Ok(Self::Install(InstallCommand {
                    target: target.clone(),
                }))
            }
            "pip" | "pypi" => {
                let Some(cmd0) = rest.first() else {
                    return Err(usage(&USAGE_PIP.replace("{sub}", &sub)));
                };
                let action = match cmd0.to_ascii_lowercase().as_str() {
                    "install" => PipAction::Install,
                    "upgrade" => PipAction::Upgrade,
                    _ => PipAction::Targets,
                };
                if action != PipAction::Targets {
                    let Some(pkg) = rest.get(1) else {
                        return Err(usage(&USAGE_PIP_CMD.replace("{sub}", &sub).replace("{cmd}", cmd0)));
                    };
                    Ok(Self::Pip(PipCommand {
                        alias_pypi: sub == "pypi",
                        action,
                        packages: vec![format!("pip:{pkg}")],
                    }))
                } else {
                    Ok(Self::Pip(PipCommand {
                        alias_pypi: sub == "pypi",
                        action,
                        packages: rest.iter().map(|r| pip_target(r)).collect(),
                    }))
                }
            }
            "brew" => {
                let Some(cmd0) = rest.first() else {
                    return Err(usage(USAGE_BREW));
                };
                match cmd0.to_ascii_lowercase().as_str() {
                    "update" => Ok(Self::Brew(BrewCommand {
                        action: BrewAction::Update,
                        formula: None,
                    })),
                    "upgrade" => match rest.get(1) {
                        Some(formula) => Ok(Self::Brew(BrewCommand {
                            action: BrewAction::Upgrade,
                            formula: Some(formula.clone()),
                        })),
                        None => Ok(Self::Brew(BrewCommand {
                            action: BrewAction::Upgrade,
                            formula: None,
                        })),
                    },
                    "install" => {
                        let Some(formula) = rest.get(1) else {
                            return Err(usage(&USAGE_BREW_CMD.replace("{cmd}", cmd0)));
                        };
                        Ok(Self::Brew(BrewCommand {
                            action: BrewAction::Install,
                            formula: Some(formula.clone()),
                        }))
                    }
                    _ => Ok(Self::Brew(BrewCommand {
                        action: BrewAction::Targets,
                        formula: None,
                    })),
                }
            }
            "info" => {
                let Some(target) = rest.first() else {
                    return Err(usage(USAGE_INFO));
                };
                Ok(Self::Info(InfoCommand {
                    target: target.clone(),
                }))
            }
            "hash" => {
                let Some(file) = rest.first() else {
                    return Err(usage(USAGE_HASH));
                };
                Ok(Self::Hash(HashCommand { file: file.clone() }))
            }
            "extract" => {
                let Some(archive) = rest.first() else {
                    return Err(usage(USAGE_EXTRACT));
                };
                let dest_dir = rest.get(1).cloned().or_else(|| cli.output.clone());
                Ok(Self::Extract(ExtractCommand {
                    archive: archive.clone(),
                    dest_dir,
                }))
            }
            "history" => {
                let limit = match rest.first() {
                    Some(raw) => Some(
                        raw.parse::<i64>()
                            .map_err(|_| usage(&USAGE_HISTORY_LIMIT.replace("{value}", raw)))?,
                    ),
                    None => None,
                };
                Ok(Self::History(HistoryCommand { limit }))
            }
            "clean" => Ok(Self::Clean(CleanCommand {
                dir: rest.first().cloned().or_else(|| cli.output.clone()),
            })),
            "config" => {
                let action = match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                    None | Some("show") => ConfigAction::Show,
                    Some("set") => ConfigAction::Set,
                    Some("reset") => ConfigAction::Reset,
                    Some("reset-hosts") => ConfigAction::ResetHosts,
                    Some(other) => return Err(usage(&format!("  [error]Unknown config action: {other}[/error]"))),
                };
                Ok(Self::Config(ConfigCommand {
                    action,
                    key: rest.get(1).cloned(),
                    value: rest.get(2).cloned(),
                }))
            }
            "diagnose" | "doctor" => Ok(Self::Doctor),
            "auth" => {
                let action = match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                    None | Some("status") => AuthAction::Status,
                    Some("set") => AuthAction::Set,
                    Some("remove") => AuthAction::Remove,
                    Some(other) => return Err(usage(&format!("  [error]Unknown auth action: {other}[/error]"))),
                };
                Ok(Self::Auth(AuthCommand {
                    action,
                    service: rest.get(1).cloned(),
                }))
            }
            "netprobe" => {
                let Some(url) = rest.first() else {
                    return Err(usage(USAGE_NETPROBE));
                };
                Ok(Self::Netprobe(NetprobeCommand { url: url.clone() }))
            }
            "bench" => {
                let Some(url) = rest.first() else {
                    return Err(usage(USAGE_BENCH));
                };
                Ok(Self::Bench(BenchCommand { url: url.clone() }))
            }
            "profiles" => {
                let (action, host) = match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                    None | Some("list") => (ProfilesAction::List, None),
                    Some("reset") => (ProfilesAction::Reset, rest.get(1).cloned()),
                    Some(other) => return Err(usage(&format!("  [error]Unknown profiles action: {other}[/error]"))),
                };
                Ok(Self::Profiles(ProfilesCommand { action, host }))
            }
            "queue" => {
                let tail: &[String] = rest.split_first().map_or(&[], |(_, tail)| tail);
                let (action, args) = match rest.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                    None | Some("list") => (QueueAction::List, tail.to_vec()),
                    Some("add") => (QueueAction::Add, tail.to_vec()),
                    Some("pause") => (QueueAction::Pause, tail.to_vec()),
                    Some("resume") => (QueueAction::Resume, tail.to_vec()),
                    Some("remove") => (QueueAction::Remove, tail.to_vec()),
                    Some("run") => (QueueAction::Run, tail.to_vec()),
                    Some("launchd-plist") => (QueueAction::LaunchdPlist, tail.to_vec()),
                    Some(other) => return Err(usage(&format!("  [error]Unknown queue action: {other}[/error]"))),
                };
                Ok(Self::Queue(QueueCommand { action, args }))
            }
            "resume" => Ok(Self::Resume(ResumeCommand {
                args: rest.to_vec(),
            })),
            "server" => Ok(Self::Server(ServerCommand {
                host: cli.host.clone(),
                port: cli.port,
                reload: cli.reload,
            })),
            "tui" => Ok(Self::Tui),
            "throttle" => Ok(Self::Throttle(ThrottleCommand {
                options: ThrottleCli::try_parse_from(rest.to_vec())
                    .map_err(|e| usage(&e.to_string()))?,
            })),
            _ => Ok(Self::Download(cli.clone())),
        }
    }
}

fn pip_target(raw: &str) -> String {
    if raw.starts_with('-') {
        raw.to_string()
    } else {
        format!("pip:{raw}")
    }
}

fn usage(message: &str) -> Error {
    Error::usage(message)
}

/// Exact `print_plain` usage strings from `src/paradl/cli/dispatch.py`.
pub const USAGE_INSTALL: &str = "  [error]Usage: pdl install <target> [--yes][/error]";
/// Exact `print_plain` usage string for `pip`/`pypi` (contains `{sub}`).
pub const USAGE_PIP: &str = "  [error]Usage: pdl {sub} [install|upgrade] <package>[/error]";
/// Exact `print_plain` usage string for a pip sub-action (contains `{sub}`/`{cmd}`).
pub const USAGE_PIP_CMD: &str = "  [error]Usage: pdl {sub} {cmd} <package>[/error]";
/// Exact `print_plain` usage string for `brew`.
pub const USAGE_BREW: &str = "  [error]Usage: pdl brew [install|update|upgrade] <formula>[/error]";
/// Exact `print_plain` usage string for a brew sub-action (contains `{cmd}`).
pub const USAGE_BREW_CMD: &str = "  [error]Usage: pdl brew {cmd} <formula>[/error]";
/// Exact `print_plain` usage string for `info`.
pub const USAGE_INFO: &str = "  [error]Usage: pdl info <target>[/error]";
/// Exact `print_plain` usage string for `hash`.
pub const USAGE_HASH: &str = "  [error]Usage: pdl hash <file>[/error]";
/// Exact `print_plain` usage string for `extract`.
pub const USAGE_EXTRACT: &str = "  [error]Usage: pdl extract <archive> [dest_dir][/error]";
/// Exact `print_plain` template for an invalid history limit (contains `{value}`).
pub const USAGE_HISTORY_LIMIT: &str = "  [error]Invalid history limit: {value}[/error]";
/// Exact `print_plain` usage string for `netprobe`.
pub const USAGE_NETPROBE: &str =
    "  [error]Usage: pdl netprobe <url> [--duration 60] [--http3-probe][/error]";
/// Exact `print_plain` usage string for `bench`.
pub const USAGE_BENCH: &str =
    "  [error]Usage: pdl bench <url> [--watchdog v1,v2,off] [--duration 60][/error]";
/// Exact `print_plain` message when `brew` is not on `PATH`.
pub const USAGE_BREW_MISSING: &str =
    "  [error]Homebrew ('brew') binary not found on PATH.[/error]";

/// The dispatch seam implemented by `paradl-app` in W4-1.
///
/// Implementations execute typed commands; this crate intentionally supplies no
/// command bodies. A default-free single method keeps the contract small while
/// still allowing `paradl-app` to match on every [`Command`] variant.
pub trait Dispatch {
    /// Execute `command` using the fully parsed `cli` for global flags.
    ///
    /// Returns the process [`ExitCode`] chosen by the implementation.
    fn handle(&mut self, command: &Command, cli: &Cli) -> ExitCode;
}

/// Convenience free function around [`Dispatch::handle`].
pub fn dispatch<D: Dispatch>(handler: &mut D, command: &Command, cli: &Cli) -> ExitCode {
    handler.handle(command, cli)
}

/// A no-op dispatcher useful for tests and for tooling that only needs parsing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopDispatch;

impl Dispatch for NoopDispatch {
    fn handle(&mut self, _: &Command, _: &Cli) -> ExitCode {
        ExitCode::Ok
    }
}

/// Environment variables the CLI contract reads (parity matrix §4).
///
/// `argparse` itself binds no environment variables; the Python entry point reads
/// these through `PDLConfig` and the auth layer. They are surfaced here so Rust can
/// honour the same contract without inventing new flags.
pub const CLI_ENV_VARS: &[&str] = &[
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "PARADL_HOME",
    "PARADL_MEDIA_PY",
    "PARADL_ASSERT_NO_PYTHON",
    "NONINTERACTIVE",
    "HOMEBREW_NO_AUTO_UPDATE",
    "PARADL_GITHUB_ARCHIVE_MIRROR",
    "VIRTUAL_ENV",
    "LOCALAPPDATA",
    "PYO3_PYTHON",
];
