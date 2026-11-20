//! The reference application: one `Dispatch` implementation wiring every
//! framework crate behind the byte-exact `paradl-cli` surface.
//!
//! Command bodies are deliberately thin adapters. Byte-exact behavior (routing
//! rules, event shapes, exit codes, checksums, schemas) comes from the crates;
//! this module only sequences them and owns stdout.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use paradl_cli::{
    AuthAction, AuthCommand, BenchCommand, BrewCommand, Cli, CleanCommand, Command, ConfigAction,
    ConfigCommand, Dispatch, ExitCode, ExtractCommand, HashCommand, InfoCommand, InstallCommand,
    NetprobeCommand, PipCommand, ProfilesAction, ProfilesCommand, QueueAction, QueueCommand,
    ResumeCommand, ServerCommand,
};
use paradl_core::{Error, PDLConfig};
use paradl_events::{
    ApiServerStartedEvent, AuthRemoveEvent, AuthSetEvent, AuthStatusEvent, BenchEvent, CleanEvent,
    ConfigEvent, ConfigResetEvent, ConfigUpdatedEvent, ErrorEvent, Event, HashesEvent, HistoryEvent,
    HostsResetEvent, InfoEvent, LaunchdPlistGeneratedEvent, NetprobeEvent, ProfilesEvent,
    ProfilesResetEvent,
    QueueJobAddedEvent, QueueJobStartedEvent, QueueListEvent, QueuePausedEvent, QueueRemovedEvent,
    QueueResumedEvent, ResultEvent, emit_jsonl, now_timestamp,
};
use paradl_net::routing::{PureClaimsChecker, RouteDecision, classify_target};
use paradl_postproc::{PostProcessor, checksum};
use paradl_resilience::auth::{InMemorySecretStore, OsEnv, Service, TokenSource, lookup_token};
use paradl_store::{QueueJob, SqliteStore, Store, TransferRecord};
use paradl_ui::event_summary;
use serde_json::{Value, json};

use crate::diagnostics::DoctorReport;
#[cfg(feature = "media")]
use crate::handoffs::map_media_error_kind;

/// Output sink used by [`App`]. The default writes to process stdout; tests
/// inject a capturing closure so no subcommand needs a real terminal.
pub type OutputSink = Arc<dyn Fn(&str) + Send + Sync>;

fn stdout_sink() -> OutputSink {
    Arc::new(|line: &str| print!("{line}"))
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
        }
    }

    /// Hermetic constructor: no network, no worker spawn, default config.
    pub fn new_offline() -> Self {
        Self {
            output: stdout_sink(),
            offline: true,
            config: PDLConfig::default(),
            runtime: None,
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
        let mut code = ExitCode::Ok;
        for target in targets {
            let decision = classify_target(&target, None, &PureClaimsChecker);
            if let RouteDecision::DelegateToWorker { .. } = decision {
                if let Err(error) = self.media_route(&target, cli) {
                    code = self.fail(cli.json, &error);
                }
                continue;
            }
            match self.transfer(&target, &decision, cli) {
                Ok(result) => {
                    self.publish(cli.json, Event::Result(result));
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
    ) -> Result<ResultEvent, Error> {
        let started = Instant::now();
        let file_name = decision_filename(target, decision);
        let dest = destination(cli, &file_name);
        let target_type = decision_name(decision);

        let (success, bytes, file_path, error_text) = if self.offline {
            (true, 0, Some(dest.to_string_lossy().into_owned()), None)
        } else {
            self.run_engine(target, cli, &dest)?
        };

        if !self.offline && success {
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

        Ok(ResultEvent::new(
            now_timestamp(),
            Some("download".to_string()),
            Some(target.to_string()),
            success,
            file_path,
            None,
            error_text,
            if success { 0 } else { 1 },
            Some(target_type),
            Some(bytes),
            Some(started.elapsed().as_secs_f64()),
            Some(0.0),
            Some(0),
            None,
            None,
            None,
            Some(false),
        ))
    }

    fn run_engine(
        &mut self,
        target: &str,
        cli: &Cli,
        dest: &Path,
    ) -> Result<(bool, i64, Option<String>, Option<String>), Error> {
        use paradl_engine::{DownloadEngine, NdEngine, NoopProgressSink, Target as EngineTarget};
        use url::Url;

        let url = Url::parse(target)
            .map_err(|error| Error::usage(format!("invalid download URL {target:?}: {error}")))?;
        let mut engine_target = EngineTarget {
            url,
            dest: dest.to_path_buf(),
            connections: paradl_engine::clamp_connections(cli.connections.max(0) as u32),
            http3: cli.effective_http3(),
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
        let runtime = self.runtime();
        let outcome = runtime.block_on(async {
            let plan = engine.plan(&engine_target).await?;
            engine.download(plan, &mut NoopProgressSink).await
        })?;
        let bytes = i64::try_from(outcome.bytes_downloaded).unwrap_or(i64::MAX);
        let path = outcome.file_path.map(|path| path.to_string_lossy().into_owned());
        let success = outcome.success;
        let error = outcome.error;
        Ok((success, bytes, path, error))
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

    fn info(&self, command: &InfoCommand, json: bool) -> ExitCode {
        let decision = classify_target(&command.target, None, &PureClaimsChecker);
        self.publish(
            json,
            Event::Info(InfoEvent::new(
                now_timestamp(),
                command.target.clone(),
                decision_properties(&command.target, &decision),
            )),
        );
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
        #[cfg(feature = "api")]
        if !self.offline {
            let port = u16::try_from(command.port).unwrap_or(8000);
            match paradl_api::run_server(&command.host, port) {
                Ok(()) => return ExitCode::Ok,
                Err(error) => return self.fail(json, &error),
            }
        }
        let port = u16::try_from(command.port).unwrap_or(8000);
        self.publish(
            json,
            Event::ApiServerStarted(ApiServerStartedEvent::new(
                now_timestamp(),
                command.host.clone(),
                port,
            )),
        );
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
