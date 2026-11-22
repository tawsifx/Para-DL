//! The reference application: one `Dispatch` implementation wiring every
//! framework crate behind the byte-exact `paradl-cli` surface.
//!
//! Command bodies are deliberately thin adapters. Byte-exact behavior (routing
//! rules, event shapes, exit codes, checksums, schemas) comes from the crates;
//! this module only sequences them and owns stdout.
//!
//! ## Platform dispatch (spec C1)
//!
//! Only [`paradl_platform::Platform`] is held behind `dyn` (through
//! `PlatformSecretStore`); the other charter traits stay static-dispatch bounds.
//!
//! The C2 child modules live at `src/*.rs` (not `src/app/*.rs`), so each is
//! declared here with an explicit `#[path]` while remaining a child of `app`.

#[path = "commands.rs"]
mod commands;
#[path = "naming.rs"]
mod naming;
#[path = "transfer.rs"]
mod transfer;

use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;

use paradl_cli::{Cli, Command, Dispatch, ExitCode};
use paradl_core::{Error, PDLConfig};
use paradl_events::{ErrorEvent, Event, emit_jsonl, now_timestamp};
use paradl_net::routing::{PureClaimsChecker, classify_target};
use paradl_net::{HttpClient, HttpClientConfig, HttpSiteHttp, SiteHttp};
use paradl_package::{PackageContext, ProcessRunner, SystemProcessRunner};
use paradl_ui::event_summary;

use crate::handoffs::{api_resolve_failure_status, normalise_resolve_failure};

use naming::{decision_clean_url, decision_name};

/// Output sink used by [`App`]. The default writes to process stdout; tests
/// inject a capturing closure so no subcommand needs a real terminal.
pub type OutputSink = Arc<dyn Fn(&str) + Send + Sync>;

fn stdout_sink() -> OutputSink {
    Arc::new(|line: &str| print!("{line}"))
}

/// Feature-independent result of classifying a target for `POST /resolve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResolve {
    /// The original target string.
    pub target: String,
    /// The classified target type string (`direct`, `stream`, `github`, …).
    pub target_type: String,
    /// Normalized URL after shortcut expansion.
    pub clean_url: String,
}

/// Feature-independent bridge between pure routing and the HTTP executor.
///
/// Both the `api`-gated `CommandExecutor` and dispatch-level tests use this
/// bridge, so the 400-vs-500 mapping is exercised on the production path even
/// when the binary is built without the `api` feature.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiBridge;

impl ApiBridge {
    /// Classify `target`, normalising unresolvable targets to usage errors.
    pub fn resolve(target: &str) -> Result<ApiResolve, Error> {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            return Err(normalise_resolve_failure(&Error::failure(
                "target could not be resolved: empty target",
            )));
        }
        let decision = classify_target(trimmed, None, &PureClaimsChecker);
        Ok(ApiResolve {
            target: trimmed.to_owned(),
            target_type: decision_name(&decision),
            clean_url: decision_clean_url(&decision, trimmed),
        })
    }
}

/// The app-owned executor wired into `paradl-api` instead of its no-op default.
#[cfg(feature = "api")]
struct AppExecutor;

#[cfg(feature = "api")]
#[async_trait::async_trait]
impl paradl_api::CommandExecutor for AppExecutor {
    async fn resolve(
        &self,
        target: &str,
    ) -> Result<paradl_api::ResolveResponse, paradl_core::Error> {
        let resolved = ApiBridge::resolve(target).map_err(|error| normalise_resolve_failure(&error))?;
        // The HTTP status is decided by `ApiError::from_core` from the exit
        // code; this assertion pins the oracle's 400 contract in debug builds.
        debug_assert_eq!(api_resolve_failure_status(&Error::failure("resolve")), 400);
        Ok(paradl_api::ResolveResponse {
            target: resolved.target,
            target_type: resolved.target_type,
            clean_url: Some(resolved.clean_url),
            ..paradl_api::ResolveResponse::default()
        })
    }

    async fn download(
        &self,
        task: paradl_api::ApiTask,
    ) -> Result<paradl_api::DownloadOutcome, paradl_core::Error> {
        Ok(paradl_api::DownloadOutcome {
            file_path: task.filename,
            bytes_downloaded: 0,
        })
    }
}

pub use transfer::TransferSetup;

/// Orchestrates all framework crates for the `paradl` reference binary.
///
/// [`App::new`] is the production constructor. [`App::new_offline`] disables
/// network and process side effects for hermetic tests and dry runs; both
/// constructors work with no Python on non-media paths.
pub struct App {
    output: OutputSink,
    offline: bool,
    config: PDLConfig,
    runtime: Option<tokio::runtime::Runtime>,
    cancel: Option<Arc<AtomicBool>>,
    last_transfer: Option<TransferSetup>,
    /// Resolved ISP observed on the info path (handoff (a)); observable without
    /// polluting the byte-exact oracle `info` event.
    last_info_isp: Option<String>,
    record_history: bool,
    /// Injectable process seam for package-manager installs.
    package_runner: Arc<dyn ProcessRunner>,
    /// Injectable HTTP seam for package-manager resolvers (`None` = real).
    package_http: Option<Arc<dyn SiteHttp>>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("offline", &self.offline)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Production constructor: loads `config.json` when present.
    pub fn new() -> Self {
        let config = PDLConfig::load().unwrap_or_else(|_| PDLConfig::default());
        Self {
            output: stdout_sink(),
            offline: false,
            config,
            runtime: None,
            cancel: None,
            last_transfer: None,
            last_info_isp: None,
            record_history: true,
            package_runner: Arc::new(SystemProcessRunner),
            package_http: None,
        }
    }

    /// Online constructor with an explicit config (used by loopback tests).
    pub fn with_config(config: PDLConfig) -> Self {
        Self {
            output: stdout_sink(),
            offline: false,
            config,
            runtime: None,
            cancel: None,
            last_transfer: None,
            last_info_isp: None,
            record_history: true,
            package_runner: Arc::new(SystemProcessRunner),
            package_http: None,
        }
    }

    /// Hermetic constructor: no network, no worker spawn, default config.
    pub fn new_offline() -> Self {
        Self::offline_with_config(PDLConfig::default())
    }

    /// Hermetic constructor with an explicit config (used by handoff tests).
    pub fn offline_with_config(config: PDLConfig) -> Self {
        Self {
            output: stdout_sink(),
            offline: true,
            config,
            runtime: None,
            cancel: None,
            last_transfer: None,
            last_info_isp: None,
            record_history: true,
            package_runner: Arc::new(SystemProcessRunner),
            package_http: None,
        }
    }

    /// Replace the output sink (used by tests to capture emitted lines).
    pub fn with_output(mut self, sink: OutputSink) -> Self {
        self.output = sink;
        self
    }

    /// Inject both package-manager seams (tests only).
    pub fn with_package_context(
        mut self,
        runner: Arc<dyn ProcessRunner>,
        http: Arc<dyn SiteHttp>,
    ) -> Self {
        self.package_runner = runner;
        self.package_http = Some(http);
        self
    }

    /// Build the context for a package-manager install from config, honouring
    /// an injected HTTP seam when present.
    pub(super) fn package_context(&self) -> Result<PackageContext, Error> {
        let http = match &self.package_http {
            Some(http) => Arc::clone(http),
            None => {
                let client = HttpClient::new(HttpClientConfig {
                    doh_endpoint: self.config.doh_endpoint.clone(),
                    ..HttpClientConfig::default()
                })?;
                Arc::new(HttpSiteHttp::new(client))
            }
        };
        Ok(PackageContext {
            runner: Arc::clone(&self.package_runner),
            http,
            proxy: None,
            doh_endpoint: self.config.doh_endpoint.clone(),
            connections: self.config.connections.max(1) as u32,
        })
    }

    /// Whether this app disables network and process side effects.
    pub fn is_offline(&self) -> bool {
        self.offline
    }

    /// The effective configuration used by adapters.
    pub fn configuration(&self) -> &PDLConfig {
        &self.config
    }

    /// Inputs used by the most recent transfer, if one was dispatched.
    pub fn last_transfer(&self) -> Option<&TransferSetup> {
        self.last_transfer.as_ref()
    }

    /// Resolved ISP observed on the most recent `info` dispatch.
    pub fn last_info_isp(&self) -> Option<&str> {
        self.last_info_isp.as_deref()
    }

    /// Disable writing transfer-history rows (used by loopback integration tests).
    pub fn disable_history_recording(&mut self) {
        self.record_history = false;
    }

    /// Install a cooperative cancel flag consumed by the real download engine.
    ///
    /// When the flag becomes `true` the engine stops workers, leaves the
    /// `.pdl.part` / `.pdl.json` sidecars on disk and returns exit code 130.
    pub fn set_cancel(&mut self, flag: Arc<AtomicBool>) {
        self.cancel = Some(flag);
    }

    /// Resolve a target for the API `/resolve` endpoint.
    ///
    /// Shared by the app-owned `CommandExecutor` (under the `api` feature) and
    /// by dispatch-level tests; failures are normalised so the API answers the
    /// oracle's HTTP 400 instead of a generic 500.
    pub fn resolve_for_api(&self, target: &str) -> Result<ApiResolve, Error> {
        ApiBridge::resolve(target)
    }

    /// HTTP status the API layer will produce for a resolution error.
    pub fn resolve_failure_status(&self, error: &Error) -> u16 {
        api_resolve_failure_status(error)
    }

    /// Lazily create a current-thread Tokio runtime for async crate calls.
    fn runtime(&mut self) -> &mut tokio::runtime::Runtime {
        if self.runtime.is_none() {
            self.runtime = Some(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build a Tokio runtime"),
            );
        }
        self.runtime.as_mut().expect("runtime just initialised")
    }

    fn line(&self, text: &str) {
        (self.output)(text);
    }

    /// Publish a typed event through the single emitter (or a plain summary).
    fn publish(&self, json: bool, event: Event) {
        if json {
            self.line(&emit_jsonl(&event));
        } else {
            self.line(&format!("{}\n", event_summary(&event)));
        }
    }

    /// Publish an error event and return its process exit code.
    fn fail(&self, json: bool, error: &Error) -> ExitCode {
        self.publish(
            json,
            Event::Error(ErrorEvent::new(
                now_timestamp(),
                error.to_string(),
                None,
                None,
                None,
                None,
                None,
            )),
        );
        error.exit_code()
    }

    /// Expand positional targets plus `--input-file` (F30).
    fn expand_targets(&self, cli: &Cli) -> Vec<String> {
        let mut targets = cli.targets.clone();
        if let Some(file) = &cli.input_file
            && let Ok(text) = std::fs::read_to_string(file)
        {
            targets.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(str::to_owned),
            );
        }
        targets
    }

    /// Root dispatch: every `Command` variant has an explicit handler.
    fn dispatch(&mut self, command: &Command, cli: &Cli) -> ExitCode {
        let json = cli.json;
        match command {
            Command::Download(_) => self.download(cli),
            Command::Doctor => self.doctor(json),
            Command::Info(command) => self.info(command, json),
            Command::Hash(command) => self.hash(command, json),
            Command::Extract(command) => self.extract(command, json),
            Command::History(command) => self.history(command, cli, json),
            Command::Clean(command) => self.clean(command, json),
            Command::Config(command) => self.run_config(command, json),
            Command::Auth(command) => self.auth(command, json),
            Command::Install(command) => self.install(command, json),
            Command::Profiles(command) => self.profiles(command, json),
            Command::Queue(command) => self.queue(command, cli, json),
            Command::Resume(command) => self.resume(command, json),
            Command::Netprobe(command) => self.netprobe(command, cli, json),
            Command::Bench(command) => self.bench(command, cli, json),
            Command::Throttle(command) => self.throttle(command, json),
            Command::Pip(command) => self.pip(command, json),
            Command::Brew(command) => self.brew(command, json),
            Command::Server(command) => self.server(command, json),
            Command::Tui => self.tui(),
            Command::Version => {
                self.line(paradl_cli::VERSION_TEXT);
                ExitCode::Ok
            }
        }
    }
}

impl Dispatch for App {
    fn handle(&mut self, command: &Command, cli: &Cli) -> ExitCode {
        self.dispatch(command, cli)
    }
}

// ---- small pure helpers ---------------------------------------------------

/// Capturing sink helper exported for downstream examples and tests.
#[derive(Debug, Default)]
pub struct Capture {
    /// All output lines received so far, concatenated.
    pub lines: Arc<Mutex<String>>,
}

impl Capture {
    /// Build an [`OutputSink`] that appends to this capture.
    pub fn sink(&self) -> OutputSink {
        let lines = Arc::clone(&self.lines);
        Arc::new(move |line: &str| lines.lock().unwrap().push_str(line))
    }

    /// Snapshot the captured output.
    pub fn contents(&self) -> String {
        self.lines.lock().unwrap().clone()
    }
}
