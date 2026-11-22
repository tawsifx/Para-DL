//! RED-first tests for the W4-1 dispatch wiring.
//!
//! Every `paradl_cli::Command` variant must be routed to a real `paradl-app`
//! handler. The tests use `App::new_offline()` (no network, no Python) and an
//! injected output sink so they can assert which single-emitter JSONL event a
//! handler produced without touching the process stdout.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use paradl_app::App;
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};
use paradl_core::PDLConfig;
use paradl_net::Result as NetResult;
use paradl_net::sites::{JsonResponse, RedirectInfo, SiteHttp};
use paradl_package::{
    ArtifactDownloader, CommandSpec, DownloadOutcome, ProcessOutcome, ProcessRunner,
};

struct FakeRunner {
    fail_install: bool,
    calls: Arc<Mutex<Vec<String>>>,
}
impl ProcessRunner for FakeRunner {
    fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, paradl_core::Error> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{} {}", spec.program, spec.args.join(" ")));
        let stdout = if spec.args.first().map(String::as_str) == Some("--cache") {
            format!(
                "{}",
                std::env::temp_dir()
                    .join("pdl-dispatch-test")
                    .join("wget-1.21.4.arm64_sequoia.bottle.tar.gz")
                    .display()
            )
        } else {
            String::new()
        };
        let status = if self.fail_install && spec.args.first().map(String::as_str) == Some("install")
        {
            1
        } else {
            0
        };
        Ok(ProcessOutcome {
            status,
            stdout,
            stderr: String::new(),
        })
    }
}

struct FakeHttp;
#[async_trait]
impl SiteHttp for FakeHttp {
    async fn fetch_json(
        &self,
        _url: &str,
        _headers: &HashMap<String, String>,
        _timeout: std::time::Duration,
    ) -> NetResult<JsonResponse> {
        Ok(JsonResponse {
            status: 200,
            headers: HashMap::new(),
            body: serde_json::json!({
                "versions": { "stable": "1.21.4" },
                "dependencies": [],
                "bottle": { "stable": { "files": {
                    "arm64_sequoia": {
                        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "url": "https://example.com/wget.bottle.tar.gz"
                    }
                }}}
            }),
        })
    }
    async fn redirect(
        &self,
        _url: &str,
        _headers: &HashMap<String, String>,
        _timeout: std::time::Duration,
    ) -> NetResult<Option<RedirectInfo>> {
        Ok(None)
    }
}

struct FakeDownloader;
#[async_trait]
impl ArtifactDownloader for FakeDownloader {
    async fn download(
        &self,
        _url: &str,
        _dest: &Path,
        _expected: Option<&str>,
        _connections: u32,
    ) -> Result<DownloadOutcome, paradl_core::Error> {
        Ok(DownloadOutcome {
            success: true,
            bytes: 6,
            error: None,
            exit_code: ExitCode::Ok,
        })
    }
}

fn package_dispatch(argv: &[&str], fail_install: bool) -> (ExitCode, String, Vec<String>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runner = FakeRunner {
        fail_install,
        calls: Arc::clone(&calls),
    };
    let argv: Vec<String> = argv.iter().map(|s| (*s).to_string()).collect();
    let outcome = Cli::parse_command_line(argv.clone()).expect("parse succeeds");
    let ParseOutcome::Command(cli) = outcome else { panic!("expected command") };
    let command = Command::from_cli(&cli).expect("classification succeeds");
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = {
        let buffer = Arc::clone(&buffer);
        Arc::new(move |line: &str| buffer.lock().unwrap().push_str(line))
    };
    let mut app = App::with_config(PDLConfig::default())
        .with_output(sink)
        .with_package_context(Arc::new(runner), Arc::new(FakeHttp), Arc::new(FakeDownloader));
    let code = app.handle(&command, &cli);
    (code, buffer.lock().unwrap().clone(), calls.lock().unwrap().clone())
}

fn dispatch(argv: &[&str]) -> (ExitCode, String) {
    let argv: Vec<String> = argv.iter().map(|s| (*s).to_string()).collect();
    let outcome = Cli::parse_command_line(argv).expect("parse succeeds");
    let ParseOutcome::Command(cli) = outcome else {
        panic!("expected a command, got {outcome:?}");
    };
    let command = Command::from_cli(&cli).expect("classification succeeds");
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = {
        let buffer = Arc::clone(&buffer);
        Arc::new(move |line: &str| buffer.lock().unwrap().push_str(line))
    };
    let mut app = App::new_offline().with_output(sink);
    let code = app.handle(&command, &cli);
    (code, buffer.lock().unwrap().clone())
}

/// (row, argv, exit-code upper bound, event substring emitted in --json mode).
/// `None` in the last slot means the handler is allowed to emit no event.
const CASES: &[(&str, &[&str], ExitCode, Option<&str>)] = &[
    ("C01 download", &["https://example.com/f.zip"], ExitCode::Ok, Some("\"event\": \"result\"")),
    ("C02 auth set", &["auth", "set", "github"], ExitCode::Ok, Some("\"event\": \"auth_set\"")),
    ("C03 auth status", &["auth", "status"], ExitCode::Ok, Some("\"event\": \"auth_status\"")),
    ("C05 doctor", &["doctor"], ExitCode::Ok, Some("\"event\": \"doctor\"")),
    // Offline install refuses to fake success: it emits an error and a non-Ok code.
    ("C06 install", &["install", "astral-sh/uv"], ExitCode::Failure, Some("\"event\": \"error\"")),
    ("C07 info", &["info", "https://example.com/f.zip"], ExitCode::Ok, Some("\"event\": \"info\"")),
    ("C08 hash", &["hash", "Cargo.toml"], ExitCode::Ok, Some("\"event\": \"hashes\"")),
    ("C11 history", &["history", "5"], ExitCode::Ok, Some("\"event\": \"history\"")),
    ("C12 profiles", &["profiles"], ExitCode::Ok, Some("\"event\": \"profiles\"")),
    ("C13 queue list", &["queue", "list"], ExitCode::Ok, Some("\"event\": \"queue_list\"")),
    ("C13 queue add", &["queue", "add", "https://example.com/f.zip"], ExitCode::Ok, Some("\"event\": \"queue_job_added\"")),
    ("C15 clean", &["clean", "."], ExitCode::Ok, Some("\"event\": \"clean\"")),
    ("C16 config show", &["config", "show"], ExitCode::Ok, Some("\"event\": \"config\"")),
    ("C16 config set", &["config", "set", "connections", "8"], ExitCode::Ok, Some("\"event\": \"config_updated\"")),
    ("C17 netprobe", &["netprobe", "https://example.com/f.zip"], ExitCode::Ok, Some("\"event\": \"netprobe\"")),
    ("C18 bench", &["bench", "https://example.com/f.zip"], ExitCode::Ok, Some("\"event\": \"bench\"")),
    ("C22 diagnose", &["diagnose"], ExitCode::Ok, Some("\"event\": \"diagnose\"")),
    // C23/C24: offline install is rejected explicitly, never faked as success.
    ("C23 pip", &["pip", "install", "requests"], ExitCode::Failure, Some("\"event\": \"error\"")),
    ("C24 brew", &["brew", "install", "wget"], ExitCode::Failure, Some("\"event\": \"error\"")),
    ("C27 server", &["server"], ExitCode::Ok, Some("\"event\": \"api_server_started\"")),
    ("C28 tui", &["tui"], ExitCode::Ok, None),
    ("C38 queue pause", &["queue", "pause", "1"], ExitCode::Ok, Some("\"event\": \"queue_paused\"")),
    ("C39 queue resume", &["queue", "resume", "1"], ExitCode::Ok, Some("\"event\": \"queue_resumed\"")),
    ("C40 queue remove", &["queue", "remove", "1"], ExitCode::Ok, Some("\"event\": \"queue_removed\"")),
    ("C43 queue run", &["queue", "run"], ExitCode::Ok, Some("\"event\": \"queue_job_started\"")),
    ("C44 queue launchd", &["queue", "launchd-plist"], ExitCode::Ok, Some("\"event\": \"launchd_plist_generated\"")),
    ("extract", &["extract", "missing.zip"], ExitCode::Ok, Some("\"event\": \"result\"")),
    ("resume", &["resume", "--all"], ExitCode::Ok, Some("\"event\": \"result\"")),
];

#[test]
fn netprobe_http3_probe_flag_is_retained_in_event() {
    let (code, output) = dispatch(&[
        "netprobe", "https://example.com/f", "--http3-probe", "--json",
    ]);
    assert_eq!(code, ExitCode::Ok);
    assert!(output.contains("\"http3_supported\":"), "F59 field must survive: {output}");
}

#[test]
fn every_subcommand_has_a_wired_handler() {
    for (name, argv, _bound, event) in CASES {
        let mut args = argv.to_vec();
        args.push("--json");
        let (code, output) = dispatch(&args);
        assert!(
            code.as_i32() >= 0 && code.as_i32() <= 130,
            "{name}: unexpected exit code {code}"
        );
        if let Some(event) = event {
            assert!(
                output.contains(event),
                "{name}: expected event {event} in JSONL output, got: {output}"
            );
            assert!(
                output.contains("\"timestamp\":"),
                "{name}: event must come from the single emitter: {output}"
            );
        }
    }
}

#[test]
fn non_json_mode_never_emits_jsonl() {
    for (name, argv, _, _) in CASES {
        let (_, output) = dispatch(argv);
        assert!(
            !output.contains("\"event\":\""),
            "{name}: plain mode must not emit JSONL: {output}"
        );
    }
}

#[test]
fn info_json_emits_target_classified_then_info_with_oracle_fields() {
    let (_, output) = dispatch(&["info", "https://example.com/f.zip", "--json"]);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "{output}");
    assert!(lines[0].contains("\"event\": \"target_classified\""), "{}", lines[0]);
    for field in [
        "\"target\":", "\"target_type\":", "\"clean_url\":",
        "\"filename_hint\":", "\"content_length\":", "\"accept_ranges\":",
    ] {
        assert!(lines[0].contains(field), "target_classified missing {field}: {}", lines[0]);
    }
    assert!(lines[1].contains("\"event\": \"info\""), "{}", lines[1]);
    for field in ["\"Classification\":", "\"Resolved URL\":", "\"Filename Hint\":"] {
        assert!(lines[1].contains(field), "info missing {field}: {}", lines[1]);
    }
    assert!(!lines[1].contains("\"isp\""), "info must stay byte-exact: {}", lines[1]);
}

#[test]
fn download_json_emits_target_classified_before_result() {
    // Offline mode has no engine telemetry, so no progress row; the ordering
    // of the two terminal-facing events is still asserted here. The online
    // progress row is covered by the W4-2 non-media gate.
    let (_, output) = dispatch(&["https://example.com/f.zip", "--json"]);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "{output}");
    assert!(lines[0].contains("\"event\": \"target_classified\""), "{}", lines[0]);
    assert!(lines[1].contains("\"event\": \"result\""), "{}", lines[1]);
    for field in [
        "\"target\":", "\"target_type\":", "\"success\":", "\"file_path\":",
        "\"bytes_downloaded\":", "\"duration\":", "\"avg_speed_bps\":",
        "\"recycle_count\":", "\"audio_format_produced\":", "\"checksum_verified\":",
        "\"checksum_algo\":", "\"checksum_digest\":", "\"adb_synced\":",
        "\"error\":", "\"exit_code\":",
    ] {
        assert!(lines[1].contains(field), "result missing {field}: {}", lines[1]);
    }
}

#[test]
fn extensionless_direct_url_falls_back_without_media_worker() {
    // A plain file URL with no recognised extension (e.g. an APK endpoint) is
    // classified as a stream by pure routing. With no media feature/worker it
    // must fall back to a direct download rather than emit a missing-dependency
    // error (or silently do nothing).
    let (code, output) = dispatch(&[
        "https://d.apkpure.com/b/APK/com.android.chrome?version=latest",
        "--json",
    ]);
    assert_eq!(code, ExitCode::Ok, "{output}");
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "{output}");
    assert!(lines[0].contains("\"event\": \"target_classified\""), "{}", lines[0]);
    assert!(lines[1].contains("\"event\": \"result\""), "{}", lines[1]);
    assert!(!output.contains("missing dependency"), "{output}");
}

#[test]
fn brew_dispatch_success_emits_result() {
    let (code, output, calls) = package_dispatch(&["brew", "install", "wget", "--json"], false);
    assert_eq!(code, ExitCode::Ok, "{output}");
    assert!(output.contains("\"event\": \"result\""), "{output}");
    assert!(output.contains("\"success\": true"), "{output}");
    assert!(!output.contains("\"event\": \"error\""), "{output}");
    assert!(calls.contains(&"brew install wget".to_owned()), "{calls:?}");
}

#[test]
fn brew_dispatch_runner_failure_emits_error() {
    let (code, output, _calls) = package_dispatch(&["brew", "install", "wget", "--json"], true);
    assert_eq!(code, ExitCode::Failure, "{output}");
    assert!(output.contains("\"event\": \"error\""), "{output}");
    assert!(!output.contains("\"success\": true"), "{output}");
}

#[test]
fn targets_routes_are_explicit_errors() {
    for argv in [&["pip", "pip:foo", "--json"][..], &["brew", "brew:foo", "--json"][..]] {
        let (code, output) = dispatch(argv);
        assert_ne!(code, ExitCode::Ok, "{output}");
        assert!(output.contains("\"event\": \"error\""), "{output}");
    }
}

#[test]
fn real_media_target_still_reports_missing_media_worker() {
    // The direct-download fallback must not turn a YouTube page into a bogus
    // "successful" HTML download when the binary has no media support.
    let (code, output) = dispatch(&["https://www.youtube.com/watch?v=x", "--json"]);
    assert_eq!(code, ExitCode::MissingDependency, "{output}");
    assert!(output.contains("missing dependency"), "{output}");
}

#[test]
fn usage_errors_keep_exit_code_two() {
    for argv in [
        &["install"][..],
        &["info"],
        &["hash"],
        &["netprobe"],
        &["bench"],
    ] {
        let parsed = Cli::parse_command_line(argv.iter().map(|s| s.to_string()).collect())
            .expect("parser accepts the subcommand token");
        let ParseOutcome::Command(cli) = parsed else { panic!() };
        let err = Command::from_cli(&cli).expect_err("missing positional is a usage error");
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {argv:?}");
    }
}
