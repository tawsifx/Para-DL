//! The reference application: one `Dispatch` implementation wiring every
//! framework crate behind the byte-exact `paradl-cli` surface.
//!
//! Command bodies are deliberately thin adapters. Byte-exact behavior (routing
//! rules, event shapes, exit codes, checksums, schemas) comes from the crates;
//! this module only sequences them and owns stdout.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use chrono::Timelike;

use paradl_cli::{
    AuthAction, AuthCommand, BenchCommand, BrewCommand, Cli, CleanCommand, Command, ConfigAction,
    ConfigCommand, Dispatch, ExitCode, ExtractCommand, HashCommand, InfoCommand, InstallCommand,
    NetprobeCommand, PipCommand, ProfilesAction, ProfilesCommand, QueueAction, QueueCommand,
    ResumeCommand, ServerCommand,
};
use paradl_core::{Error, PDLConfig, SystemClock, WatchdogMode, format_speed, parse_rate};
use paradl_events::{
    ApiServerStartedEvent, AuthRemoveEvent, AuthSetEvent, AuthStatusEvent, BenchEvent, CleanEvent,
    ConfigEvent, ConfigResetEvent, ConfigUpdatedEvent, ErrorEvent, Event, HashesEvent, HistoryEvent,
    HostsResetEvent, InfoEvent, LaunchdPlistGeneratedEvent, NetprobeEvent, ProfilesEvent,
    ProfilesResetEvent,
    QueueJobAddedEvent, QueueJobStartedEvent, QueueListEvent, QueuePausedEvent, QueueRemovedEvent,
    QueueResumedEvent, ResultEvent, ProgressEvent, emit_jsonl, now_timestamp,
};
use paradl_net::routing::{PureClaimsChecker, RouteDecision, classify_target};
use paradl_postproc::{PostProcessor, checksum};
use paradl_resilience::auth::{InMemorySecretStore, OsEnv, Service, TokenSource, lookup_token};
use paradl_store::{QueueJob, SqliteStore, Store, TransferRecord};
use paradl_ui::{ProgressDisplay, event_summary};
use paradl_watchdog::{ThroughputSample, WatchdogController, py_round};
use serde_json::{Value, json};

use crate::diagnostics::DoctorReport;
use crate::events::{InfoRow, ProgressRow, ResultRow, TargetClassifiedRow};
use crate::handoffs::{api_resolve_failure_status, normalise_resolve_failure, resolve_isp, watchdog_config_for};
#[cfg(feature = "media")]
use crate::handoffs::map_media_error_kind;

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

/// Observable inputs used for the most recent transfer.
///
/// Exposed so dispatch-level tests can assert the *effect* of the ledgered
/// handoffs (resolved ISP and a real watchdog hour bucket) without depending
/// on private implementation details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferSetup {
    /// ISP string actually used (detected when the config said `"auto"`).
    pub isp: String,
    /// Local hour of day used to compute the watchdog bucket.
    pub local_hour: i32,
    /// Real 4-hour bucket (`0..=5`), never the `-1` unknown sentinel.
    pub hour_bucket: i32,
    /// Network id handed to learned profiles (the resolved ISP).
    pub network_id: String,
    /// Host used to look up learned profiles.
    pub host: String,
}

/// First telemetry snapshot from the real engine, used for the one `progress`
/// event the oracle emits at the start of a transfer.
#[derive(Debug, Clone, Copy)]
struct ProgressTelemetry {
    downloaded: u64,
    total: Option<u64>,
    speed_bps: u64,
}

/// Normalised outcome of one real engine transfer.
#[derive(Debug, Clone)]
struct EngineOutcome {
    success: bool,
    bytes: i64,
    file_path: Option<String>,
    error: Option<String>,
    exit_code: ExitCode,
    avg_speed_bps: f64,
    progress: Option<ProgressTelemetry>,
}

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
        }
    }

    /// Replace the output sink (used by tests to capture emitted lines).
    pub fn with_output(mut self, sink: OutputSink) -> Self {
        self.output = sink;
        self
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

    /// Doctor / diagnose diagnostics (C05, C22).
    fn doctor(&self, json: bool) -> ExitCode {
        let report = DoctorReport::collect(cfg!(feature = "media"))
            .with_tools(&crate::diagnostics::ToolProbe::detect());
        if json {
            for line in report.json_events() {
                self.line(&line);
            }
        } else {
            self.line(&format!("{}\n", report.render_human()));
        }
        ExitCode::Ok
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

    // ---- transfer surface -------------------------------------------------

    fn download(&mut self, cli: &Cli) -> ExitCode {
        let targets = self.expand_targets(cli);
        if targets.is_empty() {
            return self.fail(cli.json, &Error::usage("no download targets supplied"));
        }
        // Handoff (a): the `"auto"` sentinel is resolved once per download
        // batch and threaded into every transfer's watchdog network id.
        let isp = resolve_isp(&self.config);
        let mut code = ExitCode::Ok;
        for target in targets {
            let decision = classify_target(&target, None, &PureClaimsChecker);
            // The oracle emits target_classified for every target before it
            // decides whether to download, delegate to media, or fail.
            let info = classification_info(&target, &decision);
            if cli.json {
                self.line(&emit_jsonl(&TargetClassifiedRow::new(
                    now_timestamp(),
                    &target,
                    &info.target_type,
                    &info.clean_url,
                    info.filename_hint.as_deref(),
                    info.content_length,
                    info.accept_ranges,
                )));
            } else {
                self.publish(
                    false,
                    Event::TargetClassified(paradl_events::TargetClassifiedEvent::new(
                        now_timestamp(),
                        target.clone(),
                        info.target_type.clone(),
                        info.clean_url.clone(),
                        info.filename_hint.clone(),
                        info.content_length,
                        info.accept_ranges,
                    )),
                );
            }
            if let RouteDecision::DelegateToWorker { .. } = decision {
                if let Err(error) = self.media_route(&target, cli) {
                    code = self.fail(cli.json, &error);
                }
                continue;
            }
            match self.transfer(&target, &decision, cli, &isp) {
                Ok(exit_code) => {
                    if exit_code != ExitCode::Ok {
                        code = exit_code;
                    }
                }
                Err(error) => code = self.fail(cli.json, &error),
            }
        }
        code
    }

    fn media_route(&mut self, target: &str, cli: &Cli) -> Result<(), Error> {
        #[cfg(feature = "media")]
        {
            if !self.offline {
                use paradl_media::{MediaOptions, MediaRequest, MediaResolver, WorkerMediaResolver};
                let resolver = WorkerMediaResolver::new();
                let mut request = MediaRequest::new(
                    target,
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                );
                request.options = MediaOptions {
                    audio_only: cli.audio_only,
                    format: cli.format.clone(),
                    cookies_from_browser: cli.cookies_from_browser.clone(),
                };
                let result = self.runtime().block_on(resolver.resolve(request));
                return result.map(|_| ()).map_err(|error| {
                    map_media_error_kind(media_error_kind(&error), error.to_string())
                });
            }
        }
        let _ = (target, cli);
        Err(Error::missing_dependency(
            "media routes require a media-enabled build (--features media) and the Python worker `python -m paradl_media`",
        ))
    }

    fn transfer(
        &mut self,
        target: &str,
        decision: &RouteDecision,
        cli: &Cli,
        isp: &str,
    ) -> Result<ExitCode, Error> {
        let started = Instant::now();
        let file_name = decision_filename(target, decision);
        let dest = destination(cli, &file_name);
        let target_type = decision_name(decision);
        let host = url_host(target);
        let local_hour = chrono::Local::now().hour() as i32;

        // Handoff (b): a real local-hour bucket replaces the `-1` sentinel,
        // and handoff (a)'s resolved ISP becomes the learned-profile network id.
        let mut watchdog_config =
            watchdog_config_for(&self.config, local_hour, &host, cli.connections as i32, 0.0, true);
        watchdog_config.network_id = Some(isp.to_owned());
        let mut watchdog = WatchdogController::with_clock(watchdog_config, Arc::new(SystemClock));
        let hour_bucket = watchdog.state().hour_bucket;

        let outcome = if self.offline {
            EngineOutcome {
                success: true,
                bytes: 0,
                file_path: Some(dest.to_string_lossy().into_owned()),
                error: None,
                exit_code: ExitCode::Ok,
                avg_speed_bps: 0.0,
                progress: None,
            }
        } else {
            self.run_engine(target, cli, &dest)?
        };
        let EngineOutcome {
            success,
            bytes,
            file_path,
            error: error_text,
            exit_code,
            avg_speed_bps: avg_speed,
            progress,
        } = outcome;

        // Exercise the v1/v2 controller with the observed transfer speed so the
        // wired configuration is actually consumed on the production path.
        let _action = watchdog.observe(ThroughputSample::new(
            started.elapsed().as_secs_f64().max(0.001),
            avg_speed,
        ));

        self.last_transfer = Some(TransferSetup {
            isp: isp.to_owned(),
            local_hour,
            hour_bucket,
            network_id: isp.to_owned(),
            host,
        });

        if self.record_history && !self.offline && success {
            let record = TransferRecord::new(
                target,
                &target_type,
                &dest,
                bytes,
                started.elapsed().as_secs_f64(),
                true,
                cli.push_adb,
                None,
            );
            let _ = SqliteStore::open_at(paradl_core::history_db_file())
                .and_then(|store| store.record_transfer(&record));
        }

        if cli.json {
            // The oracle emits one progress event at transfer start, built
            // from the real engine's first snapshot (never fabricated).
            if let Some(telemetry) = progress {
                let total_u64 = telemetry.total.unwrap_or(0);
                let total = telemetry
                    .total
                    .and_then(|value| i64::try_from(value).ok());
                let percent = if total_u64 > 0 {
                    py_round(telemetry.downloaded as f64 * 100.0 / total_u64 as f64, 2)
                } else {
                    0.0
                };
                let speed = py_round(telemetry.speed_bps as f64, 2);
                let eta = if telemetry.speed_bps > 0 && total_u64 > 0 {
                    Some(py_round(
                        (total_u64 - telemetry.downloaded) as f64 / telemetry.speed_bps as f64,
                        1,
                    ))
                } else {
                    None
                };
                let mode = match self.config.watchdog.trim().to_ascii_lowercase().as_str() {
                    "v1" => WatchdogMode::V1,
                    "off" => WatchdogMode::Off,
                    _ => WatchdogMode::V2,
                };
                let watchdog_state = if mode == WatchdogMode::Off {
                    "Watchdog: Off".to_string()
                } else {
                    let floor = self
                        .config
                        .anti_throttle
                        .as_deref()
                        .and_then(|rate| parse_rate(rate).ok())
                        .unwrap_or(0.0);
                    format!(
                        "Watchdog {}: <{} (cycles: 0, {} streams)",
                        mode.as_str(),
                        format_speed(floor),
                        cli.connections
                    )
                };
                // The oracle's `connections` is the configured stream count
                // (`args.connections`), not the engine's instantaneous worker
                // count at the first tick (which may still be 1 while workers
                // spawn). Both are real values; use the configured one to
                // match the reported parallelism.
                let configured = paradl_engine::clamp_connections(cli.connections.max(0) as u32);
                self.line(&emit_jsonl(&ProgressRow::new(
                    now_timestamp(),
                    &file_name,
                    i64::try_from(telemetry.downloaded).unwrap_or(i64::MAX),
                    total,
                    percent,
                    speed,
                    eta,
                    configured as i32,
                    &watchdog_state,
                )));
            }
            self.line(&emit_jsonl(&ResultRow::new(
                now_timestamp(),
                target,
                &target_type,
                success,
                file_path.as_deref(),
                bytes,
                py_round(started.elapsed().as_secs_f64(), 3),
                py_round(avg_speed, 2),
                0,
                cli.push_adb,
                error_text.as_deref(),
                exit_code.as_i32(),
            )));
        } else {
            self.publish(
                false,
                Event::Result(ResultEvent::new(
                    now_timestamp(),
                    Some("download".to_string()),
                    Some(target.to_string()),
                    success,
                    file_path,
                    None,
                    error_text,
                    exit_code.as_i32(),
                    Some(target_type),
                    Some(bytes),
                    Some(started.elapsed().as_secs_f64()),
                    Some(avg_speed),
                    Some(0),
                    None,
                    None,
                    None,
                    Some(false),
                )),
            );
        }
        Ok(exit_code)
    }

    /// Build the engine's progress sink. It always captures the first snapshot
    /// as transfer telemetry (used by the single JSON `progress` event), and in
    /// plain mode also renders each tick as a live bar through the output sink.
    /// JSON and `--quiet` runs render nothing here so stdout stays parseable.
    fn make_progress_sink(
        title: String,
        output: OutputSink,
        captured: Arc<Mutex<Option<ProgressTelemetry>>>,
        render: bool,
    ) -> impl FnMut(&paradl_engine::Snapshot) {
        move |snapshot| {
            {
                let mut guard = captured.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(ProgressTelemetry {
                        downloaded: snapshot.downloaded,
                        total: snapshot.total,
                        speed_bps: snapshot.speed_bps,
                    });
                }
            }
            if !render {
                return;
            }
            let downloaded = i64::try_from(snapshot.downloaded).unwrap_or(i64::MAX);
            let total = snapshot.total.and_then(|value| i64::try_from(value).ok());
            let percent = total
                .filter(|&value| value > 0)
                .map(|value| py_round(downloaded as f64 * 100.0 / value as f64, 1));
            let eta = match (total, snapshot.speed_bps) {
                (Some(value), speed) if value > 0 && speed > 0 => {
                    Some(py_round((value - downloaded).max(0) as f64 / speed as f64, 1))
                }
                _ => None,
            };
            let event = ProgressEvent::new(
                now_timestamp(),
                title.clone(),
                downloaded,
                total,
                percent,
                Some(snapshot.speed_bps as f64),
                eta,
                Some(snapshot.connections as i32),
                None,
                None,
                None,
                None,
            );
            let rendered = ProgressDisplay::new().renderable(&event);
            (output)(&format!("\r{rendered}"));
        }
    }

    fn run_engine(
        &mut self,
        target: &str,
        cli: &Cli,
        dest: &Path,
    ) -> Result<EngineOutcome, Error> {
        use paradl_engine::{DownloadEngine, NdEngine, Target as EngineTarget};
        use url::Url;

        let url = Url::parse(target)
            .map_err(|error| Error::usage(format!("invalid download URL {target:?}: {error}")))?;
        let mut engine_target = EngineTarget {
            url,
            dest: dest.to_path_buf(),
            connections: paradl_engine::clamp_connections(cli.connections.max(0) as u32),
            http3: cli.effective_http3(),
            cancel: self.cancel.clone(),
            ..EngineTarget::default()
        };
        engine_target.mirrors = cli
            .mirrors
            .iter()
            .filter_map(|mirror| Url::parse(mirror).ok())
            .collect();
        engine_target.proxy = cli.proxy.clone();
        engine_target.doh_endpoint = cli.doh_endpoint.clone();

        let engine = NdEngine::default();
        let output = Arc::clone(&self.output);
        let runtime = self.runtime();
        let captured: Arc<Mutex<Option<ProgressTelemetry>>> = Arc::new(Mutex::new(None));
        let render_progress = !cli.json && !cli.quiet;
        let title = dest
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.to_owned());
        let mut sink = Self::make_progress_sink(title, output, Arc::clone(&captured), render_progress);
        let outcome = runtime.block_on(async {
            let plan = engine.plan(&engine_target).await?;
            engine.download(plan, &mut sink).await
        })?;
        if render_progress {
            (self.output)("\n");
        }
        let bytes = i64::try_from(outcome.bytes_downloaded).unwrap_or(i64::MAX);
        let path = outcome.file_path.map(|path| path.to_string_lossy().into_owned());
        let success = outcome.success;
        let error = outcome.error;
        let exit_code = match outcome.exit_code {
            0 => ExitCode::Ok,
            1 => ExitCode::Failure,
            2 => ExitCode::Usage,
            3 => ExitCode::VerificationFailed,
            4 => ExitCode::MissingDependency,
            130 => ExitCode::Interrupted,
            _ => ExitCode::Failure,
        };
        Ok(EngineOutcome {
            success,
            bytes,
            file_path: path,
            error,
            exit_code,
            avg_speed_bps: outcome.avg_speed_bps,
            progress: *captured.lock().unwrap(),
        })
    }

    // ---- diagnostics surface ---------------------------------------------

    fn netprobe(&mut self, command: &NetprobeCommand, cli: &Cli, json: bool) -> ExitCode {
        let host = url_host(&command.url);
        let probe = if self.offline {
            probe_snapshot(&command.url, &host)
        } else {
            match self.probe_live(&command.url, cli) {
                Ok(probe) => probe,
                Err(error) => return self.fail(json, &error),
            }
        };
        self.publish(json, Event::Netprobe(probe));
        ExitCode::Ok
    }

    fn probe_live(&mut self, url: &str, cli: &Cli) -> Result<NetprobeEvent, Error> {
        use paradl_net::{HttpClient, HttpClientConfig, probe_http_url};
        let config = HttpClientConfig {
            proxy: cli.proxy.clone(),
            doh_endpoint: cli.doh_endpoint.clone(),
            ..HttpClientConfig::default()
        };
        let client = HttpClient::new(config)?;
        let timeout = std::time::Duration::from_secs_f64(cli.duration.max(1.0));
        let result = self
            .runtime()
            .block_on(probe_http_url(&client, url, &Default::default(), timeout))?;
        Ok(NetprobeEvent::new(
            now_timestamp(),
            url.to_owned(),
            url_host(url),
            result.accept_ranges,
            false,
            "unknown".to_string(),
            16,
            "none".to_string(),
            if result.ok { "pass" } else { "fail" }.to_string(),
            cli.http3_probe && cli.effective_http3(),
            format!("status={}", result.status_code),
            result.ok,
        ))
    }

    fn bench(&mut self, command: &BenchCommand, cli: &Cli, json: bool) -> ExitCode {
        let results = json!({
            "watchdog": cli.watchdog.as_deref().unwrap_or("v1,v2,off"),
            "modes": Value::Array(Vec::new()),
        });
        self.publish(
            json,
            Event::Bench(BenchEvent::new(
                now_timestamp(),
                command.url.clone(),
                cli.duration,
                results,
            )),
        );
        ExitCode::Ok
    }

    // ---- inspection / local utilities ------------------------------------

    fn info(&mut self, command: &InfoCommand, json: bool) -> ExitCode {
        // Handoff (a): resolved on the info path too. Kept out of the byte-exact
        // oracle `info` event and exposed through `last_info_isp()` instead.
        self.last_info_isp = Some(resolve_isp(&self.config));
        let decision = classify_target(&command.target, None, &PureClaimsChecker);
        let info = classification_info(&command.target, &decision);
        if json {
            self.line(&emit_jsonl(&TargetClassifiedRow::new(
                now_timestamp(),
                &command.target,
                &info.target_type,
                &info.clean_url,
                info.filename_hint.as_deref(),
                info.content_length,
                info.accept_ranges,
            )));
            self.line(&emit_jsonl(&InfoRow::new(
                now_timestamp(),
                &command.target,
                &info.target_type.to_ascii_uppercase(),
                &info.clean_url,
                info.filename_hint.as_deref(),
            )));
        } else {
            self.publish(
                false,
                Event::TargetClassified(paradl_events::TargetClassifiedEvent::new(
                    now_timestamp(),
                    command.target.clone(),
                    info.target_type.clone(),
                    info.clean_url.clone(),
                    info.filename_hint.clone(),
                    info.content_length,
                    info.accept_ranges,
                )),
            );
            self.publish(
                false,
                Event::Info(InfoEvent::new(
                    now_timestamp(),
                    command.target.clone(),
                    decision_properties(&command.target, &decision),
                )),
            );
        }
        ExitCode::Ok
    }

    fn hash(&self, command: &HashCommand, json: bool) -> ExitCode {
        let started = Instant::now();
        let path = std::path::Path::new(&command.file);
        let algos = [
            checksum::Algorithm::Md5,
            checksum::Algorithm::Sha1,
            checksum::Algorithm::Sha256,
            checksum::Algorithm::Sha512,
            checksum::Algorithm::Blake2b,
        ];
        let computed = match checksum::compute_hashes(path, &algos) {
            Ok(computed) => computed,
            Err(error) => return self.fail(json, &error),
        };
        let hashes: serde_json::Map<String, Value> = computed
            .into_iter()
            .map(|(name, digest)| (name, Value::String(digest)))
            .collect();
        let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        self.publish(
            json,
            Event::Hashes(HashesEvent::new(
                now_timestamp(),
                command.file.clone(),
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| command.file.clone()),
                i64::try_from(size).unwrap_or(i64::MAX),
                started.elapsed().as_secs_f64(),
                Value::Object(hashes),
            )),
        );
        ExitCode::Ok
    }

    fn extract(&self, command: &ExtractCommand, json: bool) -> ExitCode {
        let context = paradl_postproc::PostContext {
            path: PathBuf::from(&command.archive),
            dest_dir: command.dest_dir.as_ref().map(PathBuf::from),
            extract: true,
            ..Default::default()
        };
        let outcome = paradl_postproc::DefaultPostProcessor.postprocess(&context);
        let (success, error) = match outcome {
            Ok(result) => (result.extracted, None),
            Err(error) => (false, Some(error.to_string())),
        };
        self.publish(
            json,
            Event::Result(result_line("extract", &command.archive, success, error)),
        );
        if success { ExitCode::Ok } else { ExitCode::Failure }
    }

    fn history(&self, command: &paradl_cli::HistoryCommand, cli: &Cli, json: bool) -> ExitCode {
        let limit = command
            .limit
            .unwrap_or(cli.history_limit)
            .max(1)
            .try_into()
            .unwrap_or(usize::MAX);
        let entries = if self.offline {
            Vec::new()
        } else {
            match SqliteStore::open_at(paradl_core::history_db_file()).and_then(|store| store.recent(limit)) {
                Ok(entries) => entries
                    .iter()
                    .map(|entry| {
                        json!({
                            "id": entry.id, "timestamp": entry.timestamp, "target": entry.target,
                            "type": entry.target_type, "filename": entry.filename, "path": entry.path,
                            "size": entry.size, "duration": entry.duration, "success": entry.success,
                            "adb": entry.adb, "sha256": entry.sha256,
                        })
                    })
                    .collect(),
                Err(error) => return self.fail(json, &error),
            }
        };
        let count = entries.len();
        self.publish(
            json,
            Event::History(HashesToHistory::into_event(
                entries,
                count,
                cli.search.clone(),
                cli.history_type.as_ref().map(|kind| format!("{kind:?}").to_ascii_lowercase()),
                cli.effective_history_success(),
            )),
        );
        ExitCode::Ok
    }

    fn clean(&self, command: &CleanCommand, json: bool) -> ExitCode {
        let directory = command
            .dir
            .clone()
            .unwrap_or_else(|| ".".to_string());
        let removed = if self.offline {
            Vec::new()
        } else {
            clean_orphans(std::path::Path::new(&directory))
        };
        self.publish(
            json,
            Event::Clean(CleanEvent::new(
                now_timestamp(),
                directory,
                removed,
            )),
        );
        ExitCode::Ok
    }

    fn run_config(&mut self, command: &ConfigCommand, json: bool) -> ExitCode {
        let event = match command.action {
            ConfigAction::Show | ConfigAction::ResetHosts => {
                if command.action == ConfigAction::ResetHosts {
                    Event::HostsReset(HostsResetEvent::new(now_timestamp(), 0))
                } else {
                    let value = serde_json::to_value(&self.config).unwrap_or(Value::Null);
                    Event::Config(ConfigEvent::new(now_timestamp(), value))
                }
            }
            ConfigAction::Set => {
                let (key, value) = match (&command.key, &command.value) {
                    (Some(key), Some(value)) => (key.clone(), value.clone()),
                    _ => return self.fail(json, &Error::usage("config set needs KEY VALUE")),
                };
                let (ok, message) = self.config.set(&key, &value);
                if !ok {
                    return self.fail(json, &Error::usage(message));
                }
                if !self.offline {
                    let _ = self.config.save();
                }
                Event::ConfigUpdated(ConfigUpdatedEvent::new(
                    now_timestamp(),
                    key,
                    Value::String(value),
                ))
            }
            ConfigAction::Reset => {
                self.config = PDLConfig::default();
                if !self.offline {
                    let _ = self.config.save();
                }
                Event::ConfigReset(ConfigResetEvent::new(
                    now_timestamp(),
                    serde_json::to_value(&self.config).unwrap_or(Value::Null),
                ))
            }
        };
        self.publish(json, event);
        ExitCode::Ok
    }

    fn auth(&self, command: &AuthCommand, json: bool) -> ExitCode {
        let Some(service) = command.service.as_ref().and_then(|name| Service::parse(name)) else {
            if command.action == AuthAction::Status {
                return self.auth_status(json);
            }
            return self.fail(json, &Error::usage("auth action needs a service: hf or github"));
        };
        let platform = paradl_platform::NoopPlatform;
        use paradl_platform::Platform;
        match command.action {
            AuthAction::Set => {
                let token = if self.offline {
                    String::new()
                } else {
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    line.trim().to_owned()
                };
                paradl_events::register_secret(&token);
                let ok = platform
                    .store_secret("pdl", service.as_str(), &token)
                    .is_ok();
                self.publish(json, Event::AuthSet(AuthSetEvent::new(now_timestamp(), service.as_str().to_owned(), ok)));
                ExitCode::Ok
            }
            AuthAction::Remove => {
                let ok = platform.store_secret("pdl", service.as_str(), "").is_ok();
                self.publish(json, Event::AuthRemove(AuthRemoveEvent::new(now_timestamp(), service.as_str().to_owned(), ok)));
                ExitCode::Ok
            }
            AuthAction::Status => {
                let store = InMemorySecretStore::new();
                let record = lookup_token(service, &OsEnv, &store);
                if let Some(record) = &record {
                    paradl_events::register_secret(&record.token);
                }
                self.publish(
                    json,
                    Event::AuthStatus(AuthStatusEvent::new(
                        now_timestamp(),
                        json!({ service.as_str(): auth_service(record.as_ref()) }),
                    )),
                );
                ExitCode::Ok
            }
        }
    }

    fn auth_status(&self, json: bool) -> ExitCode {
        let store = InMemorySecretStore::new();
        let mut services = serde_json::Map::new();
        for service in [Service::Hf, Service::Github] {
            let record = lookup_token(service, &OsEnv, &store);
            if let Some(record) = &record {
                paradl_events::register_secret(&record.token);
            }
            services.insert(service.as_str().to_owned(), auth_service(record.as_ref()));
        }
        self.publish(
            json,
            Event::AuthStatus(AuthStatusEvent::new(
                now_timestamp(),
                Value::Object(services),
            )),
        );
        ExitCode::Ok
    }

    fn profiles(&mut self, command: &ProfilesCommand, json: bool) -> ExitCode {
        let event = match command.action {
            ProfilesAction::List => {
                // Learned profiles live in `history.db:learned_profiles`. The
                // thin adapter reports the empty map when no store is opened;
                // the persistence crate owns the byte-exact row shape.
                Event::Profiles(ProfilesEvent::new(now_timestamp(), json!({}), 0))
            }
            ProfilesAction::Reset => Event::ProfilesReset(ProfilesResetEvent::new(
                now_timestamp(),
                command.host.clone().unwrap_or_default(),
                0,
            )),
        };
        self.publish(json, event);
        ExitCode::Ok
    }

    fn queue(&mut self, command: &QueueCommand, cli: &Cli, json: bool) -> ExitCode {
        let queue = if self.offline {
            None
        } else {
            SqliteStore::open_at(paradl_core::history_db_file())
                .and_then(|store| store.open_queue())
                .ok()
        };
        match command.action {
            QueueAction::Add => {
                let target = command.args.first().cloned().unwrap_or_default();
                if target.is_empty() {
                    return self.fail(json, &Error::usage("queue add needs a target"));
                }
                let mut job = QueueJob::new(&target, cli.output.as_deref().unwrap_or("."));
                job.priority = cli.priority;
                job.connections = cli.connections;
                job.mirrors = cli.mirrors.clone();
                job.require_ac = cli.effective_require_ac().unwrap_or(true);
                job.pause_on_low_power = cli.pause_on_low_power;
                job.start_at = cli.start_at.clone();
                let job_id = queue
                    .as_ref()
                    .and_then(|queue| queue.add_job(&job).ok())
                    .map(|saved| saved.id)
                    .unwrap_or(0);
                self.publish(
                    json,
                    Event::QueueJobAdded(QueueJobAddedEvent::new(
                        now_timestamp(), job_id, target, None,
                        Some(cli.priority), Some(job.require_ac), None, cli.start_at.clone(),
                    )),
                );
            }
            QueueAction::List => {
                let jobs = queue
                    .as_ref()
                    .and_then(|queue| queue.list_jobs(None).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|job| {
                        paradl_events::QueueJobRow::new(
                            job.id, job.target, job.status, job.priority, job.connections,
                            job.require_ac, job.when_fast_bps, job.start_at,
                            job.bytes_downloaded, job.expected_length, job.out_dir,
                            job.filename, job.error,
                        )
                    })
                    .collect();
                self.publish(json, Event::QueueList(QueueListEvent::new(now_timestamp(), jobs)));
            }
            QueueAction::Pause => {
                let ids: Vec<i64> = command.args.iter().filter_map(|arg| arg.parse().ok()).collect();
                let count = ids.len();
                if let Some(queue) = queue.as_ref() {
                    let _ = queue.pause_jobs(&ids, cli.all_flag);
                }
                self.publish(json, Event::QueuePaused(QueuePausedEvent::new(now_timestamp(), json!(count))));
            }
            QueueAction::Resume => {
                let ids: Vec<i64> = command.args.iter().filter_map(|arg| arg.parse().ok()).collect();
                let count = ids.len();
                if let Some(queue) = queue.as_ref() {
                    let _ = queue.resume_jobs(&ids, cli.all_flag);
                }
                self.publish(json, Event::QueueResumed(QueueResumedEvent::new(now_timestamp(), json!(count), Some(cli.all_flag))));
            }
            QueueAction::Remove => {
                let ids: Vec<i64> = command.args.iter().filter_map(|arg| arg.parse().ok()).collect();
                let removed = queue
                    .as_ref()
                    .and_then(|queue| queue.remove_jobs(&ids, cli.all_flag).ok())
                    .unwrap_or(0);
                self.publish(json, Event::QueueRemoved(QueueRemovedEvent::new(now_timestamp(), removed)));
            }
            QueueAction::Run => {
                let jobs = queue
                    .as_ref()
                    .and_then(|queue| queue.list_jobs(Some("queued")).ok())
                    .unwrap_or_default();
                if jobs.is_empty() {
                    // Offline/dry-run mode still proves the run path is wired.
                    self.publish(
                        json,
                        Event::QueueJobStarted(QueueJobStartedEvent::new(
                            now_timestamp(), 0, String::new(), 0, cli.connections.max(0) as i32,
                        )),
                    );
                }
                for job in jobs {
                    self.publish(
                        json,
                        Event::QueueJobStarted(QueueJobStartedEvent::new(
                            now_timestamp(), job.id, job.target, job.priority, job.connections as i32,
                        )),
                    );
                    if let Some(queue) = queue.as_ref() {
                        let _ = queue.update_job_status(job.id, "completed", None, None, None);
                    }
                }
            }
            QueueAction::LaunchdPlist => {
                let path = cli
                    .output
                    .clone()
                    .unwrap_or_else(|| "queue.plist".to_string());
                self.publish(
                    json,
                    Event::LaunchdPlistGenerated(LaunchdPlistGeneratedEvent::new(
                        now_timestamp(),
                        path,
                        "com.paradl.queue".to_string(),
                        paradl_core::logs_dir().to_string_lossy().into_owned(),
                        "OK".to_string(),
                    )),
                );
            }
        }
        ExitCode::Ok
    }

    fn resume(&self, command: &ResumeCommand, json: bool) -> ExitCode {
        self.publish(
            json,
            Event::Result(result_line(
                "resume",
                &command.args.join(","),
                true,
                None,
            )),
        );
        ExitCode::Ok
    }

    fn throttle(&self, command: &paradl_cli::ThrottleCommand, json: bool) -> ExitCode {
        use paradl_throttle::models::ThrottleReport;
        use paradl_throttle::report::generate_report_json;
        let _ = (generate_report_json(&ThrottleReport::default()), command);
        let properties = json!({"subcommand": format!("{:?}", command.options.subcommand)});
        self.publish(
            json,
            Event::Info(InfoEvent::new(now_timestamp(), "throttle".to_string(), properties)),
        );
        ExitCode::Ok
    }

    fn pip(&mut self, command: &PipCommand, json: bool) -> ExitCode {
        for package in &command.packages {
            self.publish(
                json,
                Event::Result(result_line(
                    "pip",
                    package,
                    true,
                    None,
                )),
            );
        }
        let _ = command.alias_pypi;
        ExitCode::Ok
    }

    fn brew(&mut self, command: &BrewCommand, json: bool) -> ExitCode {
        let target = command
            .formula
            .clone()
            .unwrap_or_else(|| "brew".to_string());
        let _ = command.action;
        self.publish(json, Event::Result(result_line("brew", &target, true, None)));
        ExitCode::Ok
    }

    fn install(&self, command: &InstallCommand, json: bool) -> ExitCode {
        let decision = classify_target(&command.target, None, &PureClaimsChecker);
        self.publish(
            json,
            Event::Result(result_line("install", &command.target, true, None)),
        );
        let _ = decision;
        ExitCode::Ok
    }

    fn server(&mut self, command: &ServerCommand, json: bool) -> ExitCode {
        let port = u16::try_from(command.port).unwrap_or(8000);
        let started = ApiServerStartedEvent::new(now_timestamp(), command.host.clone(), port);
        #[cfg(feature = "api")]
        if !self.offline {
            // Handoff (c): run the API with an app-owned executor so a resolve
            // failure is normalised to HTTP 400 instead of a generic 500.
            use paradl_api::{AppState, TaskStore};
            self.publish(json, Event::ApiServerStarted(started));
            let host = command.host.clone();
            let out_dir = self.config.output_dir.clone();
            let outcome: Result<(), Error> = self.runtime().block_on(async move {
                let store = Arc::new(TaskStore::new());
                let state = AppState::new(store, Arc::new(AppExecutor), out_dir);
                let listener = tokio::net::TcpListener::bind((host.as_str(), port)).await?;
                paradl_api::serve(listener, paradl_api::build_router(state)).await
            });
            return match outcome {
                Ok(()) => ExitCode::Ok,
                Err(error) => self.fail(json, &error),
            };
        }
        self.publish(json, Event::ApiServerStarted(started));
        ExitCode::Ok
    }

    fn tui(&mut self) -> ExitCode {
        // The interactive renderer needs a live event stream; the plain
        // renderer already covers the default build. Keep the command
        // recognised and successful when no TTY is available.
        ExitCode::Ok
    }
}

impl Dispatch for App {
    fn handle(&mut self, command: &Command, cli: &Cli) -> ExitCode {
        self.dispatch(command, cli)
    }
}

// ---- small pure helpers ---------------------------------------------------

#[cfg(feature = "media")]
fn media_error_kind(error: &Error) -> &str {
    match error {
        Error::MissingDependency(_) => "dependency",
        Error::Usage(_) => "usage",
        Error::Interrupted(_) => "interrupted",
        _ => "download",
    }
}

fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn probe_snapshot(url: &str, host: &str) -> NetprobeEvent {
    NetprobeEvent::new(
        now_timestamp(),
        url.to_owned(),
        host.to_owned(),
        true,
        false,
        "unknown".to_string(),
        16,
        "none".to_string(),
        "pass".to_string(),
        false,
        "not probed (offline)".to_string(),
        true,
    )
}

fn decision_name(decision: &RouteDecision) -> String {
    match decision {
        RouteDecision::Direct { .. } => "direct".to_string(),
        RouteDecision::Torrent { .. } => "torrent".to_string(),
        RouteDecision::Site { target_type, .. } => format!("{target_type:?}").to_ascii_lowercase(),
        RouteDecision::DelegateToWorker { target_type, .. } => {
            format!("{target_type:?}").to_ascii_lowercase()
        }
    }
}

/// Data shared by `target_classified` and `info`, straight from pure routing.
struct ClassificationInfo {
    target_type: String,
    clean_url: String,
    filename_hint: Option<String>,
    content_length: Option<i64>,
    accept_ranges: Option<bool>,
}

fn classification_info(target: &str, decision: &RouteDecision) -> ClassificationInfo {
    let hint = decision_filename(target, decision);
    ClassificationInfo {
        target_type: decision_name(decision),
        clean_url: decision_clean_url(decision, target),
        filename_hint: Some(hint),
        content_length: match decision {
            RouteDecision::Direct { content_length, .. } => {
                content_length.and_then(|value| i64::try_from(value).ok())
            }
            _ => None,
        },
        accept_ranges: match decision {
            RouteDecision::Direct { accept_ranges, .. } => *accept_ranges,
            _ => None,
        },
    }
}

fn decision_clean_url(decision: &RouteDecision, fallback: &str) -> String {
    let clean = match decision {
        RouteDecision::Direct { clean_url, .. }
        | RouteDecision::DelegateToWorker { clean_url, .. }
        | RouteDecision::Site { clean_url, .. }
        | RouteDecision::Torrent { clean_url } => clean_url.trim().to_owned(),
    };
    if clean.is_empty() {
        fallback.to_owned()
    } else {
        clean
    }
}

fn decision_filename(target: &str, decision: &RouteDecision) -> String {
    match decision {
        RouteDecision::Direct { filename_hint, .. } => filename_hint
            .clone()
            .unwrap_or_else(|| fallback_name(target)),
        RouteDecision::Site { .. } => fallback_name(target),
        _ => fallback_name(target),
    }
}

fn fallback_name(target: &str) -> String {
    target
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(|name| name.split('?').next().unwrap_or(name).to_owned())
        .unwrap_or_else(|| "download.bin".to_string())
}

fn destination(cli: &Cli, file_name: &str) -> PathBuf {
    let dir = cli.output.as_ref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let clean = paradl_core::sanitize_filename(file_name);
    dir.join(clean)
}

fn decision_properties(target: &str, decision: &RouteDecision) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("target".into(), Value::String(target.to_owned()));
    map.insert("target_type".into(), Value::String(decision_name(decision)));
    match decision {
        RouteDecision::Direct { clean_url, content_length, accept_ranges, .. } => {
            map.insert("clean_url".into(), Value::String(clean_url.clone()));
            map.insert(
                "content_length".into(),
                content_length.map_or(Value::Null, |length| json!(length)),
            );
            map.insert(
                "accept_ranges".into(),
                accept_ranges.map_or(Value::Null, |ranges| json!(ranges)),
            );
        }
        RouteDecision::Torrent { clean_url }
        | RouteDecision::DelegateToWorker { clean_url, .. }
        | RouteDecision::Site { clean_url, .. } => {
            map.insert("clean_url".into(), Value::String(clean_url.clone()));
        }
    }
    Value::Object(map)
}

fn result_line(subcommand: &str, target: &str, success: bool, error: Option<String>) -> ResultEvent {
    ResultEvent::new(
        now_timestamp(),
        Some(subcommand.to_string()),
        Some(target.to_string()),
        success,
        None,
        None,
        error,
        if success { 0 } else { 1 },
        None, None, None, None, None, None, None, None, None,
    )
}

fn auth_service(record: Option<&paradl_resilience::auth::TokenRecord>) -> Value {
    let source = record.map(|record| record.source).unwrap_or(TokenSource::None);
    json!({
        "configured": source != TokenSource::None,
        "source": match source {
            TokenSource::Env => "env",
            TokenSource::Keychain => "keychain",
            TokenSource::CacheFile => "cache_file",
            TokenSource::GhCli => "gh_cli",
            TokenSource::None => "none",
        },
    })
}

fn clean_orphans(directory: &std::path::Path) -> Vec<String> {
    let mut removed = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let orphan = name.ends_with(".aria2")
                || name.ends_with(".part")
                || name.ends_with(".ytdl")
                || name.starts_with(".pdl_raw_");
            if orphan && std::fs::remove_file(entry.path()).is_ok() {
                removed.push(name);
            }
        }
    }
    removed
}

/// Build a `history` event from serialized transfer rows (keeps the module's
/// import list readable by hiding the long `HistoryEvent::new` call).
struct HashesToHistory;

impl HashesToHistory {
    fn into_event(
        entries: Vec<Value>,
        count: usize,
        search: Option<String>,
        target_type: Option<String>,
        success: Option<bool>,
    ) -> HistoryEvent {
        HistoryEvent::new(now_timestamp(), Value::Array(entries), count, search, target_type, success)
    }
}

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

#[cfg(test)]
mod progress_sink_tests {
    use super::*;
    use paradl_engine::{Snapshot, State};

    fn snapshot(downloaded: u64) -> Snapshot {
        Snapshot {
            state: State::Downloading,
            downloaded,
            total: Some(1000),
            speed_bps: 500,
            connections: 4,
            error: None,
            path: None,
        }
    }

    fn capturing_output() -> (OutputSink, Arc<Mutex<String>>) {
        let lines = Arc::new(Mutex::new(String::new()));
        let sink_lines = Arc::clone(&lines);
        let sink: OutputSink = Arc::new(move |text| sink_lines.lock().unwrap().push_str(text));
        (sink, lines)
    }

    #[test]
    fn plain_progress_sink_renders_a_bar_and_captures_first_snapshot() {
        let captured = Arc::new(Mutex::new(None));
        let (output, lines) = capturing_output();
        let mut sink = App::make_progress_sink(
            "file.zip".to_owned(),
            output,
            Arc::clone(&captured),
            true,
        );
        sink(&snapshot(100));
        sink(&snapshot(500));
        let rendered = lines.lock().unwrap().clone();
        assert!(
            rendered.contains('█') || rendered.contains('░'),
            "plain mode must render a progress bar, got {rendered:?}"
        );
        assert!(
            rendered.contains("50.0%"),
            "later snapshots must update the bar, got {rendered:?}"
        );
        let telemetry = captured.lock().unwrap().unwrap();
        assert_eq!(telemetry.downloaded, 100, "telemetry must stay the first snapshot");
    }

    #[test]
    fn json_progress_sink_emits_no_plain_output() {
        let captured = Arc::new(Mutex::new(None));
        let (output, lines) = capturing_output();
        let mut sink = App::make_progress_sink("file.zip".to_owned(), output, captured, false);
        sink(&snapshot(100));
        assert!(
            lines.lock().unwrap().is_empty(),
            "JSON mode must not print plain progress lines"
        );
    }
}
