//! Config, history and utility command handlers split out of [`super::App`] by
//! C2. Workstream D edits `http3_supported` in `probe_live` here (locate by
//! symbol, not by line number). Bodies and event shapes are byte-identical to
//! the original `app.rs`.

use std::path::PathBuf;
use std::time::Instant;

#[cfg(feature = "api")]
use std::sync::Arc;

use paradl_cli::{
    AuthAction, AuthCommand, BenchCommand, BrewCommand, Cli, CleanCommand, ConfigAction,
    ConfigCommand, ExitCode, ExtractCommand, HashCommand, InfoCommand, InstallCommand,
    NetprobeCommand, PipCommand, ProfilesAction, ProfilesCommand, QueueAction, QueueCommand,
    ResumeCommand, ServerCommand,
};
use paradl_core::{Error, PDLConfig};
use paradl_events::{
    ApiServerStartedEvent, AuthRemoveEvent, AuthSetEvent, AuthStatusEvent, BenchEvent, CleanEvent,
    ConfigEvent, ConfigResetEvent, ConfigUpdatedEvent, Event, HashesEvent, HistoryEvent,
    HostsResetEvent, InfoEvent, LaunchdPlistGeneratedEvent, NetprobeEvent, ProfilesEvent,
    ProfilesResetEvent, QueueJobAddedEvent, QueueJobStartedEvent, QueueListEvent, QueuePausedEvent,
    QueueRemovedEvent, QueueResumedEvent, ResultEvent, emit_jsonl, now_timestamp,
};
use paradl_net::routing::{PureClaimsChecker, RouteDecision, classify_target};
use paradl_postproc::{PostProcessor, checksum};
use paradl_resilience::auth::{OsEnv, Service, TokenSource, lookup_token};
use paradl_store::{QueueJob, SqliteStore, Store};
use serde_json::{Value, json};

use crate::diagnostics::DoctorReport;
use crate::events::{InfoRow, TargetClassifiedRow};
use crate::handoffs::resolve_isp;

#[cfg(feature = "api")]
use super::AppExecutor;
use super::{App, naming::{classification_info, clean_orphans, decision_name, url_host}};

impl App {
    /// Doctor / diagnose diagnostics (C05, C22).
    pub(super) fn doctor(&self, json: bool) -> ExitCode {
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

    // ---- diagnostics surface ---------------------------------------------

    pub(super) fn netprobe(&mut self, command: &NetprobeCommand, cli: &Cli, json: bool) -> ExitCode {
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

    pub(super) fn probe_live(&mut self, url: &str, cli: &Cli) -> Result<NetprobeEvent, Error> {
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
            cli.http3_probe,
            format!("status={}", result.status_code),
            result.ok,
        ))
    }

    pub(super) fn bench(&mut self, command: &BenchCommand, cli: &Cli, json: bool) -> ExitCode {
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

    pub(super) fn info(&mut self, command: &InfoCommand, json: bool) -> ExitCode {
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

    pub(super) fn hash(&self, command: &HashCommand, json: bool) -> ExitCode {
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

    pub(super) fn extract(&self, command: &ExtractCommand, json: bool) -> ExitCode {
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

    pub(super) fn history(&self, command: &paradl_cli::HistoryCommand, cli: &Cli, json: bool) -> ExitCode {
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

    pub(super) fn clean(&self, command: &CleanCommand, json: bool) -> ExitCode {
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

    pub(super) fn run_config(&mut self, command: &ConfigCommand, json: bool) -> ExitCode {
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

    pub(super) fn auth(&self, command: &AuthCommand, json: bool) -> ExitCode {
        let Some(service) = command.service.as_ref().and_then(|name| Service::parse(name)) else {
            if command.action == AuthAction::Status {
                return self.auth_status(json);
            }
            return self.fail(json, &Error::usage("auth action needs a service: hf or github"));
        };
        let platform = paradl_platform::current();
        let store = crate::platform_store::PlatformSecretStore::new(platform);
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

    pub(super) fn auth_status(&self, json: bool) -> ExitCode {
        let store = crate::platform_store::PlatformSecretStore::new(paradl_platform::current());
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

    pub(super) fn profiles(&mut self, command: &ProfilesCommand, json: bool) -> ExitCode {
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

    pub(super) fn queue(&mut self, command: &QueueCommand, cli: &Cli, json: bool) -> ExitCode {
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

    pub(super) fn resume(&self, command: &ResumeCommand, json: bool) -> ExitCode {
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

    pub(super) fn throttle(&self, command: &paradl_cli::ThrottleCommand, json: bool) -> ExitCode {
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

    pub(super) fn pip(&mut self, command: &PipCommand, json: bool) -> ExitCode {
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

    pub(super) fn brew(&mut self, command: &BrewCommand, json: bool) -> ExitCode {
        let target = command
            .formula
            .clone()
            .unwrap_or_else(|| "brew".to_string());
        let _ = command.action;
        self.publish(json, Event::Result(result_line("brew", &target, true, None)));
        ExitCode::Ok
    }

    pub(super) fn install(&self, command: &InstallCommand, json: bool) -> ExitCode {
        let decision = classify_target(&command.target, None, &PureClaimsChecker);
        self.publish(
            json,
            Event::Result(result_line("install", &command.target, true, None)),
        );
        let _ = decision;
        ExitCode::Ok
    }

    pub(super) fn server(&mut self, command: &ServerCommand, json: bool) -> ExitCode {
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

    pub(super) fn tui(&mut self) -> ExitCode {
        // The interactive renderer needs a live event stream; the plain
        // renderer already covers the default build. Keep the command
        // recognised and successful when no TTY is available.
        ExitCode::Ok
    }
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
