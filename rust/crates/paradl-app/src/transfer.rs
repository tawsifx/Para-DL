//! Transfer handlers split out of [`super::App`] by C2.
//!
//! All JSONL emission, ordering and field shapes are unchanged from the
//! original `app.rs`; this module only owns the call sites. Workstream D edits
//! `http3` in `run_engine` here (locate by symbol, not by line number).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(feature = "media")]
use std::path::PathBuf;

use chrono::Timelike;

use paradl_cli::{Cli, ExitCode};
use paradl_core::{Error, WatchdogMode, format_speed, parse_rate};
use paradl_events::{Event, ProgressEvent, ResultEvent, emit_jsonl, now_timestamp};
use paradl_net::routing::{PureClaimsChecker, RouteDecision, classify_target};
use paradl_store::{SqliteStore, Store, TransferRecord};
use paradl_ui::ProgressDisplay;
use paradl_engine::ProgressSink;
use paradl_watchdog::{WatchdogAction, WatchdogActionType, WatchdogConfig, py_round};

use crate::events::{ProgressRow, ResultRow, TargetClassifiedRow};
use crate::handoffs::{resolve_isp, watchdog_config_for};
#[cfg(feature = "media")]
use crate::handoffs::map_media_error_kind;

use super::{App, naming::{classification_info, decision_filename, decision_name, destination, url_host}};

/// Progress sink used by real transfers: renders the bar/percentage in plain
/// mode and forwards watchdog decisions to the single JSONL emitter.
pub(crate) struct AppProgressSink {
    pub(crate) title: String,
    pub(crate) output: crate::app::OutputSink,
    pub(crate) captured: Arc<Mutex<Option<ProgressTelemetry>>>,
    pub(crate) known_total: Option<u64>,
    pub(crate) render: bool,
    pub(crate) json: bool,
}

impl AppProgressSink {
    pub(crate) fn new(
        title: String,
        output: crate::app::OutputSink,
        captured: Arc<Mutex<Option<ProgressTelemetry>>>,
        known_total: Option<u64>,
        render: bool,
        json: bool,
    ) -> Self {
        Self { title, output, captured, known_total, render, json }
    }
}

impl ProgressSink for AppProgressSink {
    fn on_progress(&mut self, snapshot: &paradl_engine::Snapshot) {
        {
            let mut guard = self.captured.lock().unwrap();
            if guard.is_none() {
                *guard = Some(ProgressTelemetry {
                    downloaded: snapshot.downloaded,
                    total: snapshot.total,
                    speed_bps: snapshot.speed_bps,
                });
            }
        }
        if !self.render {
            return;
        }
        let downloaded = i64::try_from(snapshot.downloaded).unwrap_or(i64::MAX);
        let total = snapshot
            .total
            .or(self.known_total)
            .and_then(|value| i64::try_from(value).ok());
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
            self.title.clone(),
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
        (self.output)(&format!("{rendered}\n"));
    }

    fn on_watchdog_action(&mut self, action: &WatchdogAction) {
        if action.action == WatchdogActionType::None {
            return;
        }
        if self.json {
            (self.output)(&emit_jsonl(&Event::WatchdogAction(action.to_event())));
        } else if self.render {
            (self.output)(&format!("watchdog: {} — {}\n", action.action, action.reason));
        }
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
pub(crate) struct ProgressTelemetry {
    downloaded: u64,
    total: Option<u64>,
    speed_bps: u64,
}

/// Normalised outcome of one real engine transfer.
#[derive(Debug, Clone)]
pub(crate) struct EngineOutcome {
    success: bool,
    bytes: i64,
    file_path: Option<String>,
    error: Option<String>,
    exit_code: ExitCode,
    avg_speed_bps: f64,
    progress: Option<ProgressTelemetry>,
}

impl App {
    pub(super) fn download(&mut self, cli: &Cli) -> ExitCode {
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

    pub(super) fn media_route(&mut self, target: &str, cli: &Cli) -> Result<(), Error> {
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

    pub(super) fn transfer(
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
        let hour_bucket = watchdog_config.hour_bucket.unwrap_or(-1);

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
            self.run_engine(target, cli, &dest, watchdog_config.clone())?
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

        // Best-effort origin metadata on the real transfer path only: never
        // offline, never on failure, and outside the JSON output block. The
        // seam emits no event and its result is discarded, so it cannot change
        // an exit code.
        if !self.offline && success {
            let _ = crate::platform_store::apply_origin_metadata(
                paradl_platform::current(),
                &dest,
                &[target.to_owned()],
            );
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

    pub(super) fn run_engine(
        &mut self,
        target: &str,
        cli: &Cli,
        dest: &Path,
        watchdog_config: WatchdogConfig,
    ) -> Result<EngineOutcome, Error> {
        use paradl_engine::{DownloadEngine, NdEngine, Target as EngineTarget};
        use url::Url;

        let url = Url::parse(target)
            .map_err(|error| Error::usage(format!("invalid download URL {target:?}: {error}")))?;
        let mut engine_target = EngineTarget {
            url,
            dest: dest.to_path_buf(),
            connections: paradl_engine::clamp_connections(cli.connections.max(0) as u32),
            cancel: self.cancel.clone(),
            watchdog: Some(watchdog_config),
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
        let plan = runtime.block_on(async { engine.plan(&engine_target).await })?;
        let mut sink = AppProgressSink::new(
            title,
            output,
            Arc::clone(&captured),
            plan.size,
            render_progress,
            cli.json,
        );
        let outcome = runtime.block_on(async { engine.download(plan, &mut sink).await })?;
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
}

#[cfg(feature = "media")]
fn media_error_kind(error: &Error) -> &str {
    match error {
        Error::MissingDependency(_) => "dependency",
        Error::Usage(_) => "usage",
        Error::Interrupted(_) => "interrupted",
        _ => "download",
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use paradl_engine::{Snapshot, State};

    fn snapshot(downloaded: u64, total: Option<u64>) -> Snapshot {
        Snapshot {
            state: State::Downloading,
            downloaded,
            total,
            speed_bps: 500,
            connections: 4,
            error: None,
            path: None,
        }
    }

    fn capturing_output() -> (crate::app::OutputSink, Arc<Mutex<String>>) {
        let lines = Arc::new(Mutex::new(String::new()));
        let sink_lines = Arc::clone(&lines);
        let sink: crate::app::OutputSink =
            Arc::new(move |text| sink_lines.lock().unwrap().push_str(text));
        (sink, lines)
    }

    #[test]
    fn plain_progress_renders_bar_and_percentage_from_plan_total() {
        let captured = Arc::new(Mutex::new(None));
        let (output, lines) = capturing_output();
        // First tick has no snapshot total; the plan's Content-Length supplies it.
        let mut sink = AppProgressSink::new("file.zip".to_owned(), output, Arc::clone(&captured), Some(1000), true, false);
        sink.on_progress(&snapshot(100, None));
        sink.on_progress(&snapshot(500, Some(1000)));
        let rendered = lines.lock().unwrap().clone();
        assert!(rendered.contains('█') || rendered.contains('░'), "no bar: {rendered:?}");
        assert!(rendered.contains("10.0%"), "first tick must show a real percentage: {rendered:?}");
        assert!(rendered.contains("50.0%"), "later ticks must update the percentage: {rendered:?}");
        assert!(!rendered.contains('\r'), "no carriage-return overwrites: {rendered:?}");
        assert!(rendered.ends_with('\n'), "each tick must end its own line: {rendered:?}");
        assert_eq!(captured.lock().unwrap().unwrap().downloaded, 100);
    }

    #[test]
    fn json_and_quiet_runs_render_no_plain_progress() {
        let captured = Arc::new(Mutex::new(None));
        let (output, lines) = capturing_output();
        let mut sink = AppProgressSink::new("file.zip".to_owned(), output, captured, Some(1000), false, true);
        sink.on_progress(&snapshot(500, Some(1000)));
        assert!(lines.lock().unwrap().is_empty());
    }
    fn watchdog_sink(json: bool) -> (AppProgressSink, Arc<Mutex<String>>) {
        let (output, lines) = capturing_output();
        (
            AppProgressSink::new(
                "file.zip".to_owned(),
                output,
                Arc::new(Mutex::new(None)),
                Some(1000),
                !json,
                json,
            ),
            lines,
        )
    }

    #[test]
    fn watchdog_action_is_emitted_as_jsonl_line() {
        use paradl_watchdog::{WatchdogAction, WatchdogActionType};
        let (mut sink, lines) = watchdog_sink(true);
        let mut action = WatchdogAction::new(WatchdogActionType::PauseUnpause);
        action.reason = "low speed".to_owned();
        action.timestamp = 1.0;
        sink.on_watchdog_action(&action);
        let text = lines.lock().unwrap().clone();
        assert!(text.contains("\"event\": \"watchdog_action\""), "{text:?}");
        assert!(text.contains("tier2_pause_unpause"), "{text:?}");
    }

    #[test]
    fn watchdog_action_is_rendered_in_plain_mode() {
        use paradl_watchdog::{WatchdogAction, WatchdogActionType};
        let (mut sink, lines) = watchdog_sink(false);
        let mut action = WatchdogAction::new(WatchdogActionType::PauseUnpause);
        action.reason = "low speed".to_owned();
        sink.on_watchdog_action(&action);
        let text = lines.lock().unwrap().clone();
        assert!(text.starts_with("watchdog: "), "{text:?}");
        assert!(text.contains("tier2_pause_unpause"), "{text:?}");
    }

    #[test]
    fn none_watchdog_action_is_not_emitted() {
        use paradl_watchdog::{WatchdogAction, WatchdogActionType};
        let (mut sink, lines) = watchdog_sink(true);
        sink.on_watchdog_action(&WatchdogAction::new(WatchdogActionType::None));
        assert!(lines.lock().unwrap().is_empty());
    }

}
