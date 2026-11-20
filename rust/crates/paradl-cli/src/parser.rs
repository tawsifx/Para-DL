//! clap-derived command-line parser mirroring `paradl.cli.parser.build_parser`.
//!
//! The Python oracle builds a single, flat `argparse.ArgumentParser`: subcommands are
//! represented as free-text positional `targets` and disambiguated later by
//! [the dispatch module](mod@crate::dispatch). This module preserves that exact shape so that `--help`,
//! `--version`, parsing, and usage errors stay byte-compatible with the oracle.
//!
//! Option groups in the oracle that write to the same namespace field
//! (`--require-ac`/`--no-require-ac`, `--success`/`--failed`, `--http3`/`--no-http3`,
//! `--anti-throttle`/`--no-anti-throttle`) are represented as separate booleans here
//! with effective-value accessors, which is lossless and avoids clap argument
//! conflicts.
//!
//! ```
//! use paradl_cli::{Cli, ParseOutcome, VERSION_TEXT};
//! let outcome = Cli::parse_command_line(vec!["--version".into()]).unwrap();
//! assert!(matches!(outcome, ParseOutcome::Version));
//! assert_eq!(VERSION_TEXT, "parallel-dl (paradl) v1.0.0\n");
//! ```

use clap::{ArgAction, CommandFactory, FromArgMatches, Parser, ValueEnum};

use crate::{Error, ThrottleCli};

/// Exact `--help` text captured from the Python argparse oracle.
///
/// The text is embedded verbatim from `build_parser(PDLConfig()).print_help()` and
/// installed via `clap::Command::override_help` so clap's own wrapping never alters
/// the byte stream the reference CLI emits.
pub const HELP_TEXT: &str = include_str!("../tests/fixtures/help.txt");

/// Exact `--version` line emitted by the oracle's `action="version"` argument.
pub const VERSION_TEXT: &str = "parallel-dl (paradl) v1.0.0\n";

/// Transfer backend selected by `--engine` (parity row L19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Engine {
    /// Legacy aria2 JSON-RPC fallback backend.
    Aria2,
    /// Default in-process Rust backend.
    Rust,
}

/// Media audio codec/format choices (`--audio-codec`, parity row F19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AudioCodec {
    /// CoreAudio lossless mastering.
    Lossless,
    /// Preserve the original stream without re-encoding.
    Native,
    /// MP3 export.
    Mp3,
    /// FLAC export.
    Flac,
    /// Opus export.
    Opus,
    /// Uncompressed WAV export.
    Wav,
    /// AAC export.
    Aac,
}

/// Video quality presets (`-Q/--quality`, parity row F20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Quality {
    /// Best available resolution.
    Best,
    /// 4K.
    K4,
    /// 1440p.
    P1440,
    /// 1080p.
    P1080,
    /// 720p.
    P720,
    /// 480p.
    P480,
}

/// History `--type` filter choices (parity row F74).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HistoryType {
    /// Direct HTTP(S) download.
    Direct,
    /// Streaming media target.
    Stream,
    /// Search query target.
    Search,
    /// GitHub release/repository target.
    Github,
    /// Hugging Face target.
    Huggingface,
    /// Apple IPSW target.
    Ipsw,
    /// PyPI package target.
    Pypi,
    /// Homebrew target.
    Brew,
    /// Torrent/magnet target.
    Torrent,
}

/// Outcome of parsing a command line at the process boundary.
///
/// The size difference is intentional: a flat namespace mirrors argparse exactly,
/// and boxing it would force every `paradl-app` caller through an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ParseOutcome {
    /// A fully parsed main CLI ready for dispatch.
    Command(Cli),
    /// The `throttle` fast path (mirrors `dispatch.py:34-37`): the standalone
    /// throttle parser owns these flags, so the main parser never sees them.
    Throttle(ThrottleCli),
    /// `--help` / `-h` was requested; the caller should print [`HELP_TEXT`].
    Help,
    /// `pdl throttle --help` was requested; print [`crate::THROTTLE_HELP_TEXT`].
    ThrottleHelp,
    /// `--version` was requested; the caller should print [`VERSION_TEXT`].
    Version,
}

/// The complete flat CLI namespace, a faithful port of argparse's `Namespace`.
///
/// Every field is public so `paradl-app` can read exactly the data W4-1 needs without
/// a second parsing layer.
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(name = "parallel-dl")]
pub struct Cli {
    /// Subcommand name (install/info/hash/…) or download target(s).
    #[arg(value_name = "targets")]
    pub targets: Vec<String>,

    /// Extract audio track for media streams (default: True).
    #[arg(short = 'a', long, action = ArgAction::SetTrue, default_value_t = true)]
    pub audio: bool,
    /// Download complete high-res video instead of audio.
    #[arg(short = 'v', long, action = ArgAction::SetTrue)]
    pub video: bool,
    /// CoreAudio M4A/ALAC stream packaging (default: True).
    #[arg(short = 'l', long, action = ArgAction::SetTrue, default_value_t = true)]
    pub lossless: bool,
    /// Disable ALAC mastering, keep standard AAC.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_lossless: bool,
    /// Shortcut for `--audio-codec mp3`.
    #[arg(long, action = ArgAction::SetTrue)]
    pub mp3: bool,
    /// Audio codec/format to export.
    #[arg(long, value_enum)]
    pub audio_codec: Option<AudioCodec>,
    /// Video quality resolution preset.
    #[arg(short = 'Q', long, value_enum)]
    pub quality: Option<Quality>,
    /// Download an entire playlist instead of a single media item.
    #[arg(long, action = ArgAction::SetTrue)]
    pub playlist: bool,
    /// Download and embed subtitles.
    #[arg(long, action = ArgAction::SetTrue)]
    pub subs: bool,
    /// Do not embed or write a thumbnail image.
    #[arg(long = "no-thumbnail", action = ArgAction::SetFalse, default_value_t = true)]
    pub thumbnail: bool,
    /// Extract cookies from a browser for yt-dlp.
    #[arg(long)]
    pub cookies_from_browser: Option<String>,
    /// Remove SponsorBlock segments via yt-dlp.
    #[arg(long, action = ArgAction::SetTrue)]
    pub sponsorblock: bool,
    /// Opt in to remote yt-dlp EJS challenge solver scripts.
    #[arg(long, action = ArgAction::SetTrue)]
    pub ejs_remote: bool,

    /// Parallel connection streams (1-16; clamped by `paradl-engine`).
    #[arg(short = 'c', long, default_value_t = 16)]
    pub connections: i64,
    /// HTTP(S) transfer backend (`rust` or `aria2`).
    #[arg(long, value_enum, default_value = "rust")]
    pub engine: Engine,
    /// Output destination directory.
    #[arg(short = 'o', long, alias = "output-dir")]
    pub output: Option<String>,
    /// File containing URLs/queries, one per line.
    #[arg(short = 'i', long)]
    pub input_file: Option<String>,
    /// Automatically unpack downloaded archives.
    #[arg(short = 'x', long, action = ArgAction::SetTrue)]
    pub extract: bool,
    /// Expected checksum in `[algo:]<hash>` form.
    #[arg(long)]
    pub checksum: Option<String>,
    /// Discover and verify companion checksum files.
    #[arg(long, action = ArgAction::SetTrue)]
    pub verify: bool,
    /// Custom yt-dlp format specification.
    #[arg(short = 'f', long)]
    pub format: Option<String>,
    /// Suppress banner and progress display.
    #[arg(short = 'q', long, action = ArgAction::SetTrue)]
    pub quiet: bool,
    /// Emit structured JSONL events on stdout.
    #[arg(long, action = ArgAction::SetTrue)]
    pub json: bool,
    /// Forbid all interactive prompts.
    #[arg(long, action = ArgAction::SetTrue)]
    pub non_interactive: bool,
    /// Automatic yes to confirmation prompts.
    #[arg(short = 'y', long, action = ArgAction::SetTrue)]
    pub yes: bool,
    /// Enable verbose informational logging on stderr.
    #[arg(long, action = ArgAction::SetTrue)]
    pub verbose: bool,
    /// Enable debug logging on stderr.
    #[arg(long, action = ArgAction::SetTrue)]
    pub debug: bool,
    /// Maximum download speed (e.g. `10MB/s`, `80Mbps`, `500K`).
    #[arg(long)]
    pub limit_rate: Option<String>,
    /// Auto-recycle connections below the speed threshold.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "4MBps",
        default_value = "4MBps"
    )]
    pub anti_throttle: Option<String>,
    /// Disable anti-throttle connection recycling.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_anti_throttle: bool,
    /// Grace period in seconds before checking the speed threshold.
    #[arg(long, default_value_t = 5)]
    pub anti_throttle_grace: i64,
    /// Watchdog version: `v1`, `v2`, `off`, or a comma list for `bench`.
    #[arg(long)]
    pub watchdog: Option<String>,
    /// Disable the macOS idle-sleep assertion.
    #[arg(long = "no-caffeinate", action = ArgAction::SetFalse, default_value_t = true)]
    pub caffeinate: bool,
    /// Disable the macOS network-change watcher.
    #[arg(long = "no-net-watch", action = ArgAction::SetFalse, default_value_t = true)]
    pub net_watch: bool,
    /// Seconds without byte progress before a stall recycle (0 disables).
    #[arg(long, default_value_t = 30.0)]
    pub stall_timeout: f64,
    /// Fail early unless connected to AC power.
    #[arg(long, action = ArgAction::SetTrue)]
    pub require_ac: bool,
    /// Disable the AC power requirement.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_require_ac: bool,
    /// Pause or defer while macOS Low Power Mode is active.
    #[arg(long, action = ArgAction::SetTrue)]
    pub pause_on_low_power: bool,
    /// Alternate mirror URL (repeatable).
    #[arg(long = "mirror", action = ArgAction::Append)]
    pub mirrors: Vec<String>,
    /// Start only when a short speed test exceeds this rate.
    #[arg(long, num_args = 0..=1, default_missing_value = "DEFAULT")]
    pub when_fast: Option<String>,
    /// Schedule start time in 24-hour `HH:MM` form.
    #[arg(long = "at")]
    pub start_at: Option<String>,
    /// Queue priority (higher runs first).
    #[arg(long, default_value_t = 0)]
    pub priority: i64,
    /// Apply the action to all jobs.
    #[arg(long = "all", action = ArgAction::SetTrue)]
    pub all_flag: bool,
    /// LaunchAgent `StartInterval` in seconds.
    #[arg(long, default_value_t = 900)]
    pub interval: i64,
    /// Duration in seconds for `netprobe` or `bench`.
    #[arg(long, default_value_t = 60.0)]
    pub duration: f64,
    /// Probe optional HTTP/3 support during `netprobe`.
    #[arg(long, action = ArgAction::SetTrue)]
    pub http3_probe: bool,
    /// Enable HTTP/3 (default: on; accepted for adapter parity).
    #[arg(long, action = ArgAction::SetTrue, default_value_t = true)]
    pub http3: bool,
    /// Disable HTTP/3 (accepted for adapter parity).
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_http3: bool,
    /// Proxy URL for transfers.
    #[arg(long, alias = "via-proxy", value_name = "URL")]
    pub proxy: Option<String>,
    /// Push completed files to an Android phone via ADB.
    #[arg(long, alias = "adb", action = ArgAction::SetTrue)]
    pub push_adb: bool,

    /// Filter release/HF assets by pattern or extension.
    #[arg(long)]
    pub asset: Option<String>,
    /// Download all assets concurrently.
    #[arg(long, action = ArgAction::SetTrue)]
    pub all_assets: bool,
    /// Interactive asset/model picker (TTY only).
    #[arg(long, action = ArgAction::SetTrue)]
    pub select: bool,
    /// Force downloading the repository source archive.
    #[arg(long, action = ArgAction::SetTrue)]
    pub source: bool,
    /// Branch to use for GitHub directory/archive downloads.
    #[arg(long)]
    pub branch: Option<String>,

    /// Download, verify, extract, and install a GitHub release binary.
    #[arg(long = "install")]
    pub install_target: Option<String>,
    /// Inspect target metadata without downloading.
    #[arg(long = "info")]
    pub info_target: Option<String>,
    /// Compute cryptographic hashes for a file.
    #[arg(long = "hash")]
    pub hash_file: Option<String>,
    /// Display recent download history.
    #[arg(long, action = ArgAction::SetTrue)]
    pub history: bool,
    /// Number of history entries to show.
    #[arg(long, default_value_t = 15)]
    pub history_limit: i64,
    /// Filter history by a substring.
    #[arg(long)]
    pub search: Option<String>,
    /// Filter history by target type.
    #[arg(long = "type", value_enum)]
    pub history_type: Option<HistoryType>,
    /// Show only successful history entries.
    #[arg(long, action = ArgAction::SetTrue)]
    pub success: bool,
    /// Show only failed history entries.
    #[arg(long, action = ArgAction::SetTrue)]
    pub failed: bool,
    /// Clear the download history log.
    #[arg(long, action = ArgAction::SetTrue)]
    pub clear_history: bool,
    /// Re-download a previous history entry by ID.
    #[arg(long, value_name = "ID")]
    pub from_history: Option<i64>,
    /// Clean orphaned temporary download files.
    #[arg(long, action = ArgAction::SetTrue)]
    pub clean: bool,
    /// Print system toolchain diagnostics.
    #[arg(long, action = ArgAction::SetTrue)]
    pub diagnose: bool,
    /// Run comprehensive system diagnostics.
    #[arg(long, action = ArgAction::SetTrue)]
    pub doctor: bool,
    /// Apply an ad-hoc code signature to installed binaries.
    #[arg(long, action = ArgAction::SetTrue)]
    pub sign: bool,
    /// Disable Finder `kMDItemWhereFroms` extended attributes.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_xattr: bool,
    /// Display native macOS completion notifications.
    #[arg(long, action = ArgAction::SetTrue)]
    pub notify: bool,
    /// Download from a pasted cURL command, file, or stdin.
    #[arg(long, num_args = 0..=1, default_missing_value = "-")]
    pub from_curl: Option<String>,
    /// Read pasted cURL input from the macOS clipboard.
    #[arg(long, action = ArgAction::SetTrue)]
    pub clipboard: bool,
    /// Print the program version and exit (handled explicitly for byte parity).
    #[arg(long, action = ArgAction::SetTrue)]
    pub version: bool,

    /// API server bind host.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// API server bind port.
    #[arg(long, default_value_t = 8000)]
    pub port: i64,
    /// Enable auto-reload for API development.
    #[arg(long, action = ArgAction::SetTrue)]
    pub reload: bool,

    // ---- Dropped/legacy flags kept compatible (plan Global Constraints) ----
    /// DNS-over-HTTPS endpoint (implemented by `paradl-net`'s `doh` feature).
    #[arg(long = "doh-endpoint", value_name = "URL")]
    pub doh_endpoint: Option<String>,
    /// Accepted and discarded for adapter parity.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub accept_ranges: Option<String>,
    /// Accepted and discarded for adapter parity.
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub total: Option<String>,

    // ---- Target-Rust acceptance aliases (parity note below F92) ----
    /// Custom request header (repeatable).
    #[arg(long = "header", action = ArgAction::Append)]
    pub headers: Vec<String>,
    /// Cookie header value.
    #[arg(long = "cookies")]
    pub cookies: Option<String>,
    /// Audio-only media shortcut.
    #[arg(long, action = ArgAction::SetTrue)]
    pub audio_only: bool,
    /// Embed metadata into exported media.
    #[arg(long, action = ArgAction::SetTrue)]
    pub embed_metadata: bool,
    /// Embed a thumbnail into exported media.
    #[arg(long, action = ArgAction::SetTrue)]
    pub embed_thumbnail: bool,
}

impl Cli {
    /// Build the clap `Command` with the oracle's exact help text installed.
    ///
    /// Callers that need a clap `Command` (for shell completions or help rendering)
    /// should use this instead of [`CommandFactory::command`].
    pub fn command() -> clap::Command {
        let mut cmd = <Self as CommandFactory>::command();
        cmd = cmd.override_help(HELP_TEXT).disable_version_flag(true);
        cmd
    }

    /// Parse an iterator of arguments using the oracle-configured command.
    ///
    /// Like clap's own `Parser::try_parse_from`, the iterator holds *only* real
    /// arguments; the binary name is injected here because
    /// `Command::try_get_matches_from` otherwise consumes the first argument as
    /// `argv[0]`.
    pub fn try_parse_from<I, T>(itr: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut argv = Vec::with_capacity(8);
        argv.push(std::ffi::OsString::from("parallel-dl"));
        argv.extend(itr.into_iter().map(Into::into));
        let matches = Self::command().try_get_matches_from(argv)?;
        <Self as FromArgMatches>::from_arg_matches(&matches)
    }

    /// Parse a process-style argument vector into a [`ParseOutcome`].
    ///
    /// `--help` and `--version` are handled explicitly so their output is byte-exact
    /// with the Python oracle. Every other parse failure becomes
    /// [`Error::Usage`], which maps to [`crate::ExitCode::Usage`] (`2`).
    pub fn parse_command_line(argv: Vec<String>) -> Result<ParseOutcome, Error> {
        // Python's run_cli checks `throttle` before building the main parser.
        if argv
            .first()
            .is_some_and(|first| first.eq_ignore_ascii_case("throttle"))
        {
            if argv.iter().skip(1).any(|a| a == "--help" || a == "-h") {
                return Ok(ParseOutcome::ThrottleHelp);
            }
            let throttle = ThrottleCli::try_parse_from(argv[1..].iter().cloned())
                .map_err(|e| Error::usage(e.to_string()))?;
            return Ok(ParseOutcome::Throttle(throttle));
        }
        if argv.iter().any(|arg| arg == "--version") {
            return Ok(ParseOutcome::Version);
        }
        if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
            return Ok(ParseOutcome::Help);
        }
        match Self::try_parse_from(argv) {
            Ok(cli) => Ok(ParseOutcome::Command(cli)),
            Err(err) => Err(Error::usage(err.to_string())),
        }
    }

    /// Effective anti-throttle rate: `None` when `--no-anti-throttle` was passed.
    pub fn effective_anti_throttle(&self) -> Option<&str> {
        if self.no_anti_throttle {
            None
        } else {
            self.anti_throttle.as_deref()
        }
    }

    /// Effective AC-power requirement (`None` means "unspecified"; queued jobs
    /// default to `true` in `paradl-app`).
    pub fn effective_require_ac(&self) -> Option<bool> {
        if self.no_require_ac {
            Some(false)
        } else if self.require_ac {
            Some(true)
        } else {
            None
        }
    }

    /// Effective history success filter.
    pub fn effective_history_success(&self) -> Option<bool> {
        if self.failed {
            Some(false)
        } else if self.success {
            Some(true)
        } else {
            None
        }
    }

    /// Effective HTTP/3 setting (`--no-http3` wins over `--http3`).
    pub fn effective_http3(&self) -> bool {
        self.http3 && !self.no_http3
    }

    /// Return the exact oracle help text.
    ///
    /// `clap::Command::render_help` trims the oracle's trailing whitespace-only
    /// recipe line, so the byte-exact text is served directly from [`HELP_TEXT`].
    pub fn render_help() -> String {
        HELP_TEXT.to_string()
    }
}
