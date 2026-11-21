//! Characterization snapshot for the C2 `app.rs` split.
//!
//! Every `dispatch.rs` CASES argv is dispatched in `--json` mode through an
//! injected sink. The ordered JSONL lines are compared byte-for-byte after
//! scrubbing only the wall-clock timestamp, so moving a handler between modules
//! cannot silently reorder events, drop explicit nulls, or change a field's
//! name. This snapshot was captured on the post-C1 tree and must never be
//! re-blessed after the split.

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

/// Replace the value of one `": "`-separated key while preserving every other
/// byte. The emitter uses Python-style separators, so the key includes the
/// trailing space (otherwise the numeric value would survive).
fn scrub_field(line: &str, key: &str, token: &str) -> String {
    let Some(i) = line.find(key) else { return line.to_owned() };
    let rest = &line[i + key.len()..];
    let n = rest.bytes().take_while(|b| b.is_ascii_digit() || *b == b'.' || *b == b'-' || *b == b'e' || *b == b'+').count();
    format!("{}{token}{}", &line[..i + key.len()], &rest[n..])
}

/// Normalize values that come from the clock rather than the oracle contract.
/// `timestamp` is the wall clock; `duration` is an `Instant` elapsed time;
/// `age_days` is derived from today's date in the doctor probe. Every other
/// byte — field names, order, explicit nulls, digests, paths — is snapshotted.
fn scrub(line: &str) -> String {
    let line = scrub_field(line, "\"timestamp\": ", "<TS>");
    let line = scrub_field(&line, "\"duration\": ", "<DUR>");
    scrub_field(&line, "\"age_days\": ", "<AGE>")
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
        for line in output.lines() {
            out.push_str(&scrub(line));
            out.push('\n');
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
{"event": "auth_status", "timestamp": <TS>, "services": {"github": {"configured": false, "source": "none"}, "hf": {"configured": false, "source": "none"}}}
=== C05 doctor [doctor] -> 0
{"event": "doctor", "timestamp": <TS>, "version": "1.0.0", "ytdlp": {"version": "2026.10.26", "age_days": <AGE>, "outdated": false, "path": "/opt/homebrew/bin/yt-dlp"}, "ytdlp_ejs": {"version": null}, "rust_engine": {"available": false, "version": null}, "npm": {"version": "12.2.2", "path": "/Users/tawsifx/.npm-global/bin/npm"}, "auth": {"hf": {"service": "hf", "label": "Hugging Face Hub", "configured": true, "source": "keychain"}, "github": {"service": "github", "label": "GitHub API", "configured": false, "source": "none"}}, "platform_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "macos_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "core_tools": {"GitHub PowerSuite (httpx)": {"status": "ready", "detail": "ready", "fix": null}, "Hugging Face Hub Engine": {"status": "ready", "detail": "ready (Aria2 RPC)", "fix": null}, "Rust Engine (native rapids)": {"status": "optional_missing", "detail": "not built (HTTP(S) falls back to --engine aria2)", "fix": null}, "aria2c (legacy JSON-RPC fallback)": {"status": "ready", "detail": "/opt/homebrew/bin/aria2c", "fix": null}, "yt-dlp (Media Stream Resolver)": {"status": "ready", "detail": "v2026.10.26 (26d old)", "fix": null}, "yt-dlp-ejs (JS Challenge Pkg)": {"status": "missing", "detail": "not found", "fix": null}, "JS Runtime (for EJS)": {"status": "ready", "detail": "deno (/opt/homebrew/bin/deno)", "fix": null}, "npm (Node Package Manager)": {"status": "ready", "detail": "v12.2.2 (/Users/tawsifx/.npm-global/bin/npm)", "fix": null}, "ffmpeg (AV Transcoder)": {"status": "ready", "detail": "/opt/homebrew/bin/ffmpeg", "fix": null}, "afconvert (CoreAudio)": {"status": "ready", "detail": "/usr/bin/afconvert", "fix": null}}, "fixes": ["paradl auth set github  # or: export GITHUB_TOKEN=<token>"]}
{"event": "diagnose", "timestamp": <TS>, "version": "1.0.0", "ytdlp": {"version": "2026.10.26", "age_days": <AGE>, "outdated": false, "path": "/opt/homebrew/bin/yt-dlp"}, "ytdlp_ejs": {"version": null}, "rust_engine": {"available": false, "version": null}, "npm": {"version": "12.2.2", "path": "/Users/tawsifx/.npm-global/bin/npm"}, "auth": {"hf": {"service": "hf", "label": "Hugging Face Hub", "configured": true, "source": "keychain"}, "github": {"service": "github", "label": "GitHub API", "configured": false, "source": "none"}}, "platform_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "macos_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "core_tools": {"GitHub PowerSuite (httpx)": {"status": "ready", "detail": "ready", "fix": null}, "Hugging Face Hub Engine": {"status": "ready", "detail": "ready (Aria2 RPC)", "fix": null}, "Rust Engine (native rapids)": {"status": "optional_missing", "detail": "not built (HTTP(S) falls back to --engine aria2)", "fix": null}, "aria2c (legacy JSON-RPC fallback)": {"status": "ready", "detail": "/opt/homebrew/bin/aria2c", "fix": null}, "yt-dlp (Media Stream Resolver)": {"status": "ready", "detail": "v2026.10.26 (26d old)", "fix": null}, "yt-dlp-ejs (JS Challenge Pkg)": {"status": "missing", "detail": "not found", "fix": null}, "JS Runtime (for EJS)": {"status": "ready", "detail": "deno (/opt/homebrew/bin/deno)", "fix": null}, "npm (Node Package Manager)": {"status": "ready", "detail": "v12.2.2 (/Users/tawsifx/.npm-global/bin/npm)", "fix": null}, "ffmpeg (AV Transcoder)": {"status": "ready", "detail": "/opt/homebrew/bin/ffmpeg", "fix": null}, "afconvert (CoreAudio)": {"status": "ready", "detail": "/usr/bin/afconvert", "fix": null}}, "fixes": ["paradl auth set github  # or: export GITHUB_TOKEN=<token>"]}
=== C06 install [install astral-sh/uv] -> 0
{"event": "result", "timestamp": <TS>, "subcommand": "install", "target": "astral-sh/uv", "success": true, "exit_code": 0}
=== C07 info [info https://example.com/f.zip] -> 0
{"event": "target_classified", "timestamp": <TS>, "target": "https://example.com/f.zip", "target_type": "direct", "clean_url": "https://example.com/f.zip", "filename_hint": "f.zip", "content_length": null, "accept_ranges": null}
{"event": "info", "timestamp": <TS>, "target": "https://example.com/f.zip", "properties": {"Classification": "DIRECT", "Resolved URL": "https://example.com/f.zip", "Filename Hint": "f.zip"}}
=== C08 hash [hash Cargo.toml] -> 0
{"event": "hashes", "timestamp": <TS>, "file": "Cargo.toml", "filename": "Cargo.toml", "size": 2087, "duration": <DUR>, "hashes": {"blake2b": "e71dbfaf82cbc83a80405ec80c6d814328e51c919e4992bd30fc81e408447268bbd33809e9bde59285994e8037f52649b0e24d70e4db610acf8f2e2d264483be", "md5": "c0ff92654abb782ea0c46ac68e357cd6", "sha1": "d69f2faa145621fb6b3af587ce5a220488b15dac", "sha256": "5b5b3a9f4e9e97f71964bd4dcbb09dd954cdc596e5e47fb4d070f370152441a4", "sha512": "f3ff4e476b9c88167b95405dae4802bd9a5924469b244a427e61e76784ec75523c56d1311f2f4d2c8d92911a00df5b6056deb55222b6787e226229b5f70c2e44"}}
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
{"event": "config", "timestamp": <TS>, "config": {"adb_download_dir": "/storage/emulated/0/Download", "adb_music_dir": "/storage/emulated/0/Music", "anti_throttle": "4MBps", "anti_throttle_grace": 5, "audio_codec": "aac", "auto_adb_push": false, "auto_extract": false, "bin_dir": "/Users/tawsifx/.local/bin", "caffeinate": true, "connections": 16, "doh_endpoint": null, "engine": "rust", "history_limit": 50, "isp": "auto", "max_download_limit": null, "net_watch": true, "output_dir": ".", "pause_on_low_power": false, "require_ac": false, "schema_version": 2, "stall_timeout": 30.0, "video_quality": "best", "watchdog": "v2", "ytdlp_player_client": null}}
=== C16 config set [config set connections 8] -> 0
{"event": "config_updated", "timestamp": <TS>, "key": "connections", "value": "8"}
=== C17 netprobe [netprobe https://example.com/f.zip] -> 0
{"event": "netprobe", "timestamp": <TS>, "url": "https://example.com/f.zip", "host": "example.com", "supports_range": true, "ipv6_DNS": false, "throttle_class": "unknown", "recommended_connections": 16, "recommended_action": "none", "verdict": "pass", "http3_supported": false, "http3_detail": "not probed (offline)", "passes": true}
=== C18 bench [bench https://example.com/f.zip] -> 0
{"event": "bench", "timestamp": <TS>, "url": "https://example.com/f.zip", "duration_limit_sec": 60.0, "results": {"modes": [], "watchdog": "v1,v2,off"}}
=== C22 diagnose [diagnose] -> 0
{"event": "doctor", "timestamp": <TS>, "version": "1.0.0", "ytdlp": {"version": "2026.10.26", "age_days": <AGE>, "outdated": false, "path": "/opt/homebrew/bin/yt-dlp"}, "ytdlp_ejs": {"version": null}, "rust_engine": {"available": false, "version": null}, "npm": {"version": "12.2.2", "path": "/Users/tawsifx/.npm-global/bin/npm"}, "auth": {"hf": {"service": "hf", "label": "Hugging Face Hub", "configured": true, "source": "keychain"}, "github": {"service": "github", "label": "GitHub API", "configured": false, "source": "none"}}, "platform_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "macos_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "core_tools": {"GitHub PowerSuite (httpx)": {"status": "ready", "detail": "ready", "fix": null}, "Hugging Face Hub Engine": {"status": "ready", "detail": "ready (Aria2 RPC)", "fix": null}, "Rust Engine (native rapids)": {"status": "optional_missing", "detail": "not built (HTTP(S) falls back to --engine aria2)", "fix": null}, "aria2c (legacy JSON-RPC fallback)": {"status": "ready", "detail": "/opt/homebrew/bin/aria2c", "fix": null}, "yt-dlp (Media Stream Resolver)": {"status": "ready", "detail": "v2026.10.26 (26d old)", "fix": null}, "yt-dlp-ejs (JS Challenge Pkg)": {"status": "missing", "detail": "not found", "fix": null}, "JS Runtime (for EJS)": {"status": "ready", "detail": "deno (/opt/homebrew/bin/deno)", "fix": null}, "npm (Node Package Manager)": {"status": "ready", "detail": "v12.2.2 (/Users/tawsifx/.npm-global/bin/npm)", "fix": null}, "ffmpeg (AV Transcoder)": {"status": "ready", "detail": "/opt/homebrew/bin/ffmpeg", "fix": null}, "afconvert (CoreAudio)": {"status": "ready", "detail": "/usr/bin/afconvert", "fix": null}}, "fixes": ["paradl auth set github  # or: export GITHUB_TOKEN=<token>"]}
{"event": "diagnose", "timestamp": <TS>, "version": "1.0.0", "ytdlp": {"version": "2026.10.26", "age_days": <AGE>, "outdated": false, "path": "/opt/homebrew/bin/yt-dlp"}, "ytdlp_ejs": {"version": null}, "rust_engine": {"available": false, "version": null}, "npm": {"version": "12.2.2", "path": "/Users/tawsifx/.npm-global/bin/npm"}, "auth": {"hf": {"service": "hf", "label": "Hugging Face Hub", "configured": true, "source": "keychain"}, "github": {"service": "github", "label": "GitHub API", "configured": false, "source": "none"}}, "platform_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "macos_utilities": {"caffeinate": {"available": true, "path": "/usr/bin/caffeinate", "fix": "Built-in macOS binary (/usr/bin/caffeinate)"}, "scutil": {"available": true, "path": "/usr/sbin/scutil", "fix": "Built-in macOS binary (/usr/sbin/scutil)"}, "pmset": {"available": true, "path": "/usr/bin/pmset", "fix": "Built-in macOS binary (/usr/bin/pmset)"}, "security": {"available": true, "path": "/usr/bin/security", "fix": "Built-in macOS binary (/usr/bin/security)"}, "codesign": {"available": true, "path": "/usr/bin/codesign", "fix": "Run 'xcode-select --install' to install macOS developer tools"}, "spctl": {"available": true, "path": "/usr/sbin/spctl", "fix": "Built-in macOS binary (/usr/sbin/spctl)"}, "xattr": {"available": true, "path": "/usr/bin/xattr", "fix": "Built-in macOS binary (/usr/bin/xattr)"}}, "core_tools": {"GitHub PowerSuite (httpx)": {"status": "ready", "detail": "ready", "fix": null}, "Hugging Face Hub Engine": {"status": "ready", "detail": "ready (Aria2 RPC)", "fix": null}, "Rust Engine (native rapids)": {"status": "optional_missing", "detail": "not built (HTTP(S) falls back to --engine aria2)", "fix": null}, "aria2c (legacy JSON-RPC fallback)": {"status": "ready", "detail": "/opt/homebrew/bin/aria2c", "fix": null}, "yt-dlp (Media Stream Resolver)": {"status": "ready", "detail": "v2026.10.26 (26d old)", "fix": null}, "yt-dlp-ejs (JS Challenge Pkg)": {"status": "missing", "detail": "not found", "fix": null}, "JS Runtime (for EJS)": {"status": "ready", "detail": "deno (/opt/homebrew/bin/deno)", "fix": null}, "npm (Node Package Manager)": {"status": "ready", "detail": "v12.2.2 (/Users/tawsifx/.npm-global/bin/npm)", "fix": null}, "ffmpeg (AV Transcoder)": {"status": "ready", "detail": "/opt/homebrew/bin/ffmpeg", "fix": null}, "afconvert (CoreAudio)": {"status": "ready", "detail": "/usr/bin/afconvert", "fix": null}}, "fixes": ["paradl auth set github  # or: export GITHUB_TOKEN=<token>"]}
=== C23 pip [pip install requests] -> 0
{"event": "result", "timestamp": <TS>, "subcommand": "pip", "target": "pip:requests", "success": true, "exit_code": 0}
=== C24 brew [brew install wget] -> 0
{"event": "result", "timestamp": <TS>, "subcommand": "brew", "target": "wget", "success": true, "exit_code": 0}
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
{"event": "launchd_plist_generated", "timestamp": <TS>, "path": "queue.plist", "label": "com.paradl.queue", "logs_dir": "/Users/tawsifx/.config/paradl/logs", "lint": "OK"}
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
