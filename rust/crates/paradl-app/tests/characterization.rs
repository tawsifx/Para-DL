//! Characterization snapshot for the C2 `app.rs` split.
//!
//! Every `dispatch.rs` CASES argv is dispatched in `--json` mode through an
//! injected sink. Portable cases are compared byte-for-byte after scrubbing
//! clock-derived values, so moving a handler between modules cannot silently
//! reorder events, drop explicit nulls, or change a field's name.
//!
//! Cases whose payload embeds live host state (doctor/diagnose tool probes and
//! keychain, `config show`'s home dir, `launchd-plist`'s logs dir) are captured
//! only as exit code + ordered event names. Their byte-exact field golden lives
//! in `tests/doctor.rs` with a hand-built `DoctorReport`, not a live probe.
//! This snapshot was captured on the post-C1 tree and must never be re-blessed
//! after the split.

use std::sync::{Arc, Mutex};

use paradl_app::App;
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};

/// Same argv set as `tests/dispatch.rs::CASES` — the full wired surface.
const CASES: &[(&str, &[&str])] = &[
    ("C01 download", &["https://example.com/f.zip"]),
    ("C02 auth set", &["auth", "set", "github"]),
    ("C03 auth status", &["auth", "status"]),
    ("C05 doctor", &["doctor"]),
    ("C06 install", &["install", "astral-sh/uv"]),
    ("C07 info", &["info", "https://example.com/f.zip"]),
    ("C08 hash", &["hash", "Cargo.toml"]),
    ("C11 history", &["history", "5"]),
    ("C12 profiles", &["profiles"]),
    ("C13 queue list", &["queue", "list"]),
    ("C13 queue add", &["queue", "add", "https://example.com/f.zip"]),
    ("C15 clean", &["clean", "."]),
    ("C16 config show", &["config", "show"]),
    ("C16 config set", &["config", "set", "connections", "8"]),
    ("C17 netprobe", &["netprobe", "https://example.com/f.zip"]),
    ("C18 bench", &["bench", "https://example.com/f.zip"]),
    ("C22 diagnose", &["diagnose"]),
    ("C23 pip", &["pip", "install", "requests"]),
    ("C24 brew", &["brew", "install", "wget"]),
    ("C27 server", &["server"]),
    ("C28 tui", &["tui"]),
    ("C38 queue pause", &["queue", "pause", "1"]),
    ("C39 queue resume", &["queue", "resume", "1"]),
    ("C40 queue remove", &["queue", "remove", "1"]),
    ("C43 queue run", &["queue", "run"]),
    ("C44 queue launchd", &["queue", "launchd-plist"]),
    ("extract", &["extract", "missing.zip"]),
    ("resume", &["resume", "--all"]),
];

/// Length of the JSON scalar at the start of `rest`: a quoted string (with
/// escapes) or a bare number/boolean/null token.
fn value_len(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    if bytes.first() == Some(&b'"') {
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => return i + 1,
                _ => i += 1,
            }
        }
        return bytes.len();
    }
    rest.bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        .count()
}

/// Replace the value of every `": "`-separated key while preserving every
/// other byte. The emitter uses Python-style separators, so the key includes
/// the trailing space (otherwise the numeric value would survive).
fn scrub_field(line: &str, key: &str, token: &str) -> String {
    let mut out = line.to_owned();
    let mut from = 0;
    while let Some(rel) = out[from..].find(key) {
        let i = from + rel;
        let rest = &out[i + key.len()..];
        let n = value_len(rest);
        assert!(n > 0, "no JSON value after {key} in {line}");
        out = format!("{}{token}{}", &out[..i + key.len()], &rest[n..]);
        from = i + key.len() + token.len();
    }
    out
}

/// Cases that must not be byte-snapshotted: their JSON embeds the host's tool
/// paths/versions, keychain state or home directory. Still dispatched, but only
/// exit code and ordered event names are recorded.
const ENV_SENSITIVE: &[&str] = &[
    "C05 doctor", "C22 diagnose", "C16 config show", "C44 queue launchd",
];

/// Normalize values that come from the clock or the host credential store
/// rather than the oracle contract. `timestamp` is the wall clock; `duration`
/// is an `Instant` elapsed time; `age_days` is date-derived; `configured` and
/// `source` reflect the live env/keychain token lookup. Every other byte —
/// field names, order, explicit nulls, digests, paths — is snapshotted.
fn scrub(line: &str) -> String {
    let line = scrub_field(line, "\"timestamp\": ", "<TS>");
    let line = scrub_field(&line, "\"duration\": ", "<DUR>");
    let line = scrub_field(&line, "\"age_days\": ", "<AGE>");
    let line = scrub_field(&line, "\"configured\": ", "<AUTH>");
    scrub_field(&line, "\"source\": ", "<AUTH>")
}

/// Pull the event name out of one emitter line without parsing its payload.
fn event_name(line: &str) -> &str {
    let key = "\"event\": \"";
    let start = line.find(key).expect("emitter line has an event field") + key.len();
    let rest = &line[start..];
    &rest[..rest.find('"').expect("event field has a closing quote")]
}

fn dispatch_case(argv: &[&str]) -> (ExitCode, String) {
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

/// Ordered, scrubbed snapshot of every dispatch case.
fn snapshot() -> String {
    let mut out = String::new();
    for (name, argv) in CASES {
        let mut args = argv.to_vec();
        args.push("--json");
        let (code, output) = dispatch_case(&args);
        out.push_str(&format!("=== {name} [{}] -> {code}\n", argv.join(" ")));
        if ENV_SENSITIVE.contains(name) {
            for line in output.lines() {
                out.push_str(&format!("event={}\n", event_name(line)));
            }
        } else {
            for line in output.lines() {
                out.push_str(&scrub(line));
                out.push('\n');
            }
        }
    }
    out
}

const EXPECTED: &str = r#"=== C01 download [https://example.com/f.zip] -> 0
{"event": "target_classified", "timestamp": <TS>, "target": "https://example.com/f.zip", "target_type": "direct", "clean_url": "https://example.com/f.zip", "filename_hint": "f.zip", "content_length": null, "accept_ranges": null}
{"event": "result", "timestamp": <TS>, "target": "https://example.com/f.zip", "target_type": "direct", "success": true, "file_path": "./f.zip", "bytes_downloaded": 0, "duration": <DUR>, "avg_speed_bps": 0.0, "recycle_count": 0, "audio_format_produced": null, "checksum_verified": null, "checksum_algo": null, "checksum_digest": null, "adb_synced": false, "error": null, "exit_code": 0}
=== C02 auth set [auth set github] -> 0
{"event": "auth_set", "timestamp": <TS>, "service": "github", "success": true}
=== C03 auth status [auth status] -> 0
{"event": "auth_status", "timestamp": <TS>, "services": {"github": {"configured": <AUTH>, "source": <AUTH>}, "hf": {"configured": <AUTH>, "source": <AUTH>}}}
=== C05 doctor [doctor] -> 0
event=doctor
event=diagnose
=== C06 install [install astral-sh/uv] -> 1
{"event": "error", "timestamp": <TS>, "message": "install requires network"}
=== C07 info [info https://example.com/f.zip] -> 0
{"event": "target_classified", "timestamp": <TS>, "target": "https://example.com/f.zip", "target_type": "direct", "clean_url": "https://example.com/f.zip", "filename_hint": "f.zip", "content_length": null, "accept_ranges": null}
{"event": "info", "timestamp": <TS>, "target": "https://example.com/f.zip", "properties": {"Classification": "DIRECT", "Resolved URL": "https://example.com/f.zip", "Filename Hint": "f.zip"}}
=== C08 hash [hash Cargo.toml] -> 0
{"event": "hashes", "timestamp": <TS>, "file": "Cargo.toml", "filename": "Cargo.toml", "size": 2202, "duration": <DUR>, "hashes": {"blake2b": "8aeded2d939703a0173d4998e4f51b89795febd4776e24a4a2a72ad4587eac146913851f976b015f7e10d4995e116861ce69b640b4bbca66d37ad7ba443a0257", "md5": "8bd0fa9aaab6dea4ea969e7f08626893", "sha1": "1ed9622680fcbcf56035519e91f8ae2bb0216fdb", "sha256": "47f0285f4c7ae8a438ec97753fe0a4bb5c168c5bb07d00292d4f483949242313", "sha512": "952b578e6d2c636489fdfea2dc2a86bb58419aafd378c2dd3b6cf7dde4dc36d8cee3145d579016333a3c201138b533b527dfc8df5f4bf18c6ae2d2d2c9bcece6"}}
=== C11 history [history 5] -> 0
{"event": "history", "timestamp": <TS>, "entries": [], "count": 0}
=== C12 profiles [profiles] -> 0
{"event": "profiles", "timestamp": <TS>, "profiles": {}, "count": 0}
=== C13 queue list [queue list] -> 0
{"event": "queue_list", "timestamp": <TS>, "jobs": []}
=== C13 queue add [queue add https://example.com/f.zip] -> 0
{"event": "queue_job_added", "timestamp": <TS>, "job_id": 0, "target": "https://example.com/f.zip", "priority": 0, "require_ac": true}
=== C15 clean [clean .] -> 0
{"event": "clean", "timestamp": <TS>, "directory": ".", "removed": []}
=== C16 config show [config show] -> 0
event=config
=== C16 config set [config set connections 8] -> 0
{"event": "config_updated", "timestamp": <TS>, "key": "connections", "value": "8"}
=== C17 netprobe [netprobe https://example.com/f.zip] -> 0
{"event": "netprobe", "timestamp": <TS>, "url": "https://example.com/f.zip", "host": "example.com", "supports_range": true, "ipv6_DNS": false, "throttle_class": "unknown", "recommended_connections": 16, "recommended_action": "none", "verdict": "pass", "http3_supported": false, "http3_detail": "not probed (offline)", "passes": true}
=== C18 bench [bench https://example.com/f.zip] -> 0
{"event": "bench", "timestamp": <TS>, "url": "https://example.com/f.zip", "duration_limit_sec": 60.0, "results": {"modes": [], "watchdog": "v1,v2,off"}}
=== C22 diagnose [diagnose] -> 0
event=doctor
event=diagnose
=== C23 pip [pip install requests] -> 1
{"event": "error", "timestamp": <TS>, "message": "pip install requires network"}
=== C24 brew [brew install wget] -> 1
{"event": "error", "timestamp": <TS>, "message": "brew install requires network"}
=== C27 server [server] -> 0
{"event": "api_server_started", "timestamp": <TS>, "host": "127.0.0.1", "port": 8000}
=== C28 tui [tui] -> 0
=== C38 queue pause [queue pause 1] -> 0
{"event": "queue_paused", "timestamp": <TS>, "paused": 1}
=== C39 queue resume [queue resume 1] -> 0
{"event": "queue_resumed", "timestamp": <TS>, "resumed": 1, "all": false}
=== C40 queue remove [queue remove 1] -> 0
{"event": "queue_removed", "timestamp": <TS>, "removed": 0}
=== C43 queue run [queue run] -> 0
{"event": "queue_job_started", "timestamp": <TS>, "job_id": 0, "target": "", "priority": 0, "connections": 16}
=== C44 queue launchd [queue launchd-plist] -> 0
event=launchd_plist_generated
=== extract [extract missing.zip] -> 1
{"event": "result", "timestamp": <TS>, "subcommand": "extract", "target": "missing.zip", "success": false, "error": "I/O error: No such file or directory (os error 2)", "exit_code": 1}
=== resume [resume --all] -> 0
{"event": "result", "timestamp": <TS>, "subcommand": "resume", "target": "", "success": true, "exit_code": 0}
"#;

#[test]
fn json_dispatch_is_byte_exact() {
    let actual = snapshot();
    assert_eq!(actual, EXPECTED, "dispatch output drifted from the pre-C2 snapshot");
}

#[test]
fn app_rs_stays_under_500_lines() {
    let n = include_str!("../src/app.rs").lines().count();
    assert!(n < 500, "app.rs is {n} lines after refactor");
}

/// Dump helper used once to author the pre-split snapshot. Keep it behind an
/// env flag so it never affects normal test runs.
#[test]
fn dump_snapshot_when_requested() {
    if std::env::var_os("C2_DUMP_SNAPSHOT").is_none() {
        return;
    }
    println!("<<<SNAPSHOT_START>>>\n{}<<<SNAPSHOT_END>>>", snapshot());
}
