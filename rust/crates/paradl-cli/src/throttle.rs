//! clap port of the standalone `paradl.throttle.cli` argument parser.
//!
//! The Python dispatcher fast-paths `pdl throttle …` into this separate argparse
//! parser before the main parser runs. Keeping it as a distinct clap struct means
//! `paradl-app` can reproduce that split (and its byte-exact `--help`) precisely.
//!
//! ```
//! use paradl_cli::{ThrottleCli, ThrottleSubcommand};
//! let cli = ThrottleCli::try_parse_from(["quick", "--seed", "42"]).unwrap();
//! assert_eq!(cli.subcommand, ThrottleSubcommand::Quick);
//! assert_eq!(cli.seed, 42);
//! ```

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, ValueEnum};

/// Exact `pdl throttle --help` text captured from the Python oracle.
pub const THROTTLE_HELP_TEXT: &str = include_str!("../tests/fixtures/throttle_help.txt");

/// The eight throttle diagnostic subcommands (`throttle/cli.py:55-64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThrottleSubcommand {
    /// ~300 MB paired quick run.
    Quick,
    /// Full M0-M10 diagnostic run (~3 GB budget).
    Full,
    /// Short idempotent background sample probe.
    Sample,
    /// 24-hour x weekday/weekend coverage matrix.
    Coverage,
    /// Generate (never load) a LaunchAgent plist.
    LaunchdPlist,
    /// Rebuild/report a stored run.
    Report,
    /// Run the VPS measurement server.
    Server,
    /// Validate custom target candidates.
    Targets,
}

/// Parsed `pdl throttle …` namespace, mirroring `build_throttle_parser`.
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(name = "pdl throttle")]
pub struct ThrottleCli {
    /// Diagnostic subcommand (defaults to `quick`).
    #[arg(value_enum, default_value = "quick")]
    pub subcommand: ThrottleSubcommand,
    /// Extra positional arguments (output path or run id).
    #[arg(value_name = "extra_args")]
    pub extra_args: Vec<String>,

    /// Path to a custom targets.toml file.
    #[arg(long = "targets")]
    pub targets_file: Option<String>,
    /// Domestic host or URL (repeatable).
    #[arg(long = "domestic", action = ArgAction::Append)]
    pub domestic_urls: Vec<String>,
    /// Subscribed ISP plan speed in Mbps.
    #[arg(long)]
    pub plan_mbps: Option<f64>,
    /// ISP display name.
    #[arg(long)]
    pub isp: Option<String>,
    /// HTTP or SOCKS5 proxy URL for paired M9 comparison.
    #[arg(long = "via-proxy")]
    pub via_proxy: Option<String>,
    /// Preferred network interface name.
    #[arg(long)]
    pub interface: Option<String>,
    /// Force IPv4 target resolution.
    #[arg(long, action = ArgAction::SetTrue)]
    pub ipv4: bool,
    /// Force IPv6 target resolution.
    #[arg(long, action = ArgAction::SetTrue)]
    pub ipv6: bool,
    /// Hard total data budget (e.g. `300MB`, `3GB`).
    #[arg(long)]
    pub data_budget: Option<String>,
    /// Directory for report files or the LaunchAgent plist.
    #[arg(short = 'o', long = "output")]
    pub output_dir: Option<String>,
    /// Generate a self-contained HTML report (default: True).
    #[arg(long, action = ArgAction::SetTrue, default_value_t = true)]
    pub html: bool,
    /// Rebuild this stored run id.
    #[arg(long = "run-id")]
    pub run_id: Option<String>,
    /// Bypass the 45-minute idempotency window or battery skip.
    #[arg(long, action = ArgAction::SetTrue)]
    pub force: bool,
    /// Allow the 32-stream M3 probe.
    #[arg(long, action = ArgAction::SetTrue)]
    pub allow_32: bool,
    /// RNG seed for deterministic jitter.
    #[arg(long, default_value_t = 42)]
    pub seed: i64,

    /// Bind address for the VPS server.
    #[arg(long = "host", default_value = "127.0.0.1")]
    pub server_host: String,
    /// TCP port for the VPS server.
    #[arg(long = "port", default_value_t = 8765)]
    pub server_port: i64,
    /// UDP echo port for the VPS server.
    #[arg(long = "udp-port", default_value_t = 8766)]
    pub server_udp_port: i64,
    /// Optional server run duration in seconds.
    #[arg(long = "duration")]
    pub server_duration: Option<f64>,
    /// Emit structured JSON output.
    #[arg(long, action = ArgAction::SetTrue)]
    pub json: bool,
    /// Automatic confirmation for non-interactive runs.
    #[arg(short = 'y', long, action = ArgAction::SetTrue)]
    pub yes: bool,
    /// Run without interactive prompts.
    #[arg(long, action = ArgAction::SetTrue)]
    pub non_interactive: bool,
}

impl ThrottleCli {
    /// Build the clap command with the oracle's exact throttle help text.
    pub fn command() -> clap::Command {
        let mut cmd = <Self as CommandFactory>::command();
        cmd = cmd.override_help(THROTTLE_HELP_TEXT);
        cmd
    }

    /// Parse throttle arguments (the binary name is injected for clap).
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut argv = Vec::with_capacity(8);
        argv.push(std::ffi::OsString::from("pdl throttle"));
        argv.extend(itr.into_iter().map(Into::into));
        let matches = Self::command().try_get_matches_from(argv)?;
        <Self as FromArgMatches>::from_arg_matches(&matches)
    }

    /// Render the exact oracle throttle help text.
    pub fn render_help() -> String {
        THROTTLE_HELP_TEXT.to_string()
    }
}
