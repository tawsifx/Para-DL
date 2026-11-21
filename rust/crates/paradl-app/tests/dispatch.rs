//! RED-first tests for the W4-1 dispatch wiring.
//!
//! Every `paradl_cli::Command` variant must be routed to a real `paradl-app`
//! handler. The tests use `App::new_offline()` (no network, no Python) and an
//! injected output sink so they can assert which single-emitter JSONL event a
//! handler produced without touching the process stdout.

use std::sync::{Arc, Mutex};

use paradl_app::App;
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};

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
    ("C23 pip", &["pip", "install", "requests"], ExitCode::Ok, Some("\"event\": \"result\"")),
    ("C24 brew", &["brew", "install", "wget"], ExitCode::Ok, Some("\"event\": \"result\"")),
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
