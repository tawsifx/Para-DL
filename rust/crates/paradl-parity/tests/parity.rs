//! TDD contract tests for the three-lane differential harness.
//!
//! These tests were authored against canned fixtures BEFORE the lane logic was
//! implemented; the initial run was RED (stub outcomes), then GREEN.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use paradl_parity::{
    BoundaryCase, DiffReport, LoopbackServer, PythonGuard, boundary_process_exit_code, diff_bytes,
    diff_snapshots, interrupt_payload, normalize_jsonl, normalize_stdout_fields,
    normalize_stdout_timestamps, run_boundary_case, run_full_nonmedia_gate,
    run_interrupt_resume_gate, run_media_replay,
};
use serde_json::Value;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    crate_dir().join("fixtures")
}

fn worktree() -> PathBuf {
    crate_dir().join("../..").canonicalize().unwrap()
}

/// Correct repository root (the legacy `worktree()` helper resolves to `rust/`,
/// a deferred item deliberately left unchanged this round). Used only by the
/// full-gate test, which needs `.venv/bin/python` and `parallel-dl.py`.
fn repo_root() -> PathBuf {
    crate_dir().join("../../..").canonicalize().unwrap()
}

fn python() -> PathBuf {
    if let Ok(value) = std::env::var("PARADL_PYTHON") {
        return PathBuf::from(value);
    }
    let candidate = worktree().join(".venv/bin/python");
    if candidate.is_file() {
        return candidate;
    }
    PathBuf::from("python3")
}

fn golden() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(fixtures_dir().join("ytdlp_j_golden.json")).unwrap(),
    )
    .unwrap()
}

// --- Boundary lane -----------------------------------------------------------

#[test]
fn boundary_ok_decodes_golden_data_and_emits_progress() {
    let outcome = run_boundary_case(BoundaryCase::Ok, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 0, "{}", outcome.diagnostic);
    assert_eq!(outcome.data["filename"], "Artist - Song");
    assert_eq!(outcome.data["http_headers"]["X-Count"], "42");
    assert_eq!(outcome.data["http_headers"]["X-None"], "None");
    assert_eq!(
        outcome.data["audio_format_produced"],
        "passthrough AAC (from mp4a source, no re-encode)"
    );
    assert_eq!(outcome.caps, ["ejs", "mutagen", "ffmpeg"]);
    assert_eq!(outcome.jsonl_lines.len(), 2, "two progress frames expected");
    assert!(outcome.jsonl_lines[0].contains("\"event\": \"progress\""));
}

#[test]
fn boundary_error_kind_maps_to_exit_1() {
    let outcome = run_boundary_case(BoundaryCase::Error, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.diagnostic.contains("canned download failure"),
        "{}",
        outcome.diagnostic
    );
}

#[test]
fn boundary_unknown_kind_fails_closed_to_exit_1() {
    let outcome = run_boundary_case(BoundaryCase::Unsupported, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.diagnostic.contains("bogus_kind"));
}

#[test]
fn boundary_malformed_frame_fails_closed_without_jsonl() {
    let outcome = run_boundary_case(BoundaryCase::Malformed, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.jsonl_lines.is_empty(),
        "a chatty worker must not corrupt --json"
    );
}

#[test]
fn boundary_version_mismatch_fails_closed_to_exit_4() {
    // This is the exact negative test CI runs (`--case version-mismatch`).
    let outcome = run_boundary_case(BoundaryCase::VersionMismatch, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 4);
    assert!(
        outcome.diagnostic.contains("protocol"),
        "{}",
        outcome.diagnostic
    );
}

#[test]
fn boundary_diff_with_zero_mapped_code_still_exits_1() {
    // Review round 1 (critical): the harness must never print DIFF and exit 0.
    // Take a genuinely clean `ok` case (mapped code 0) and record one synthetic
    // difference; the process decision must become 1.
    let outcome = run_boundary_case(BoundaryCase::Ok, &python(), None).unwrap();
    assert_eq!(outcome.exit_code, 0);
    let mut report = DiffReport::new("boundary", BoundaryCase::Ok.as_str());
    assert!(report.is_clean());
    assert_eq!(boundary_process_exit_code(outcome.case, !report.is_clean()), 0);
    report.compare("synthetic:data", b"expected", b"actual");
    assert!(!report.is_clean());
    assert_eq!(boundary_process_exit_code(outcome.case, !report.is_clean()), 1);
}

#[test]
fn boundary_process_exit_matrix() {
    // Any difference wins for every case, including version-mismatch.
    for case in [
        BoundaryCase::Ok,
        BoundaryCase::Error,
        BoundaryCase::Unsupported,
        BoundaryCase::Malformed,
        BoundaryCase::VersionMismatch,
    ] {
        assert_eq!(boundary_process_exit_code(case, true), 1, "{case:?}");
    }
    // Clean cases: 4 only for `version-mismatch`, 0 for the rest.
    for case in [
        BoundaryCase::Ok,
        BoundaryCase::Error,
        BoundaryCase::Unsupported,
        BoundaryCase::Malformed,
    ] {
        assert_eq!(boundary_process_exit_code(case, false), 0, "{case:?}");
    }
    assert_eq!(
        boundary_process_exit_code(BoundaryCase::VersionMismatch, false),
        4
    );
}

#[test]
fn protocol_version_matrix_matches_oracle() {
    use paradl_media::check_protocol;
    let cases: [(Option<&str>, i32); 9] = [
        (Some("paradl-media/1"), 0),
        (Some("paradl-media/1.1"), 0),
        (Some("paradl-media/1.999"), 0),
        (Some("paradl-media/0"), 4),
        (Some("paradl-media/2"), 4),
        (Some("paradl-media/99"), 4),
        (None, 4),
        (Some("garbage"), 4),
        (Some("paradl-media/"), 4),
    ];
    for (version, expected_code) in cases {
        let code = match check_protocol(version) {
            Ok(_) => 0,
            Err(error) => error.exit_code().as_i32(),
        };
        assert_eq!(code, expected_code, "protocol={version:?}");
    }
}

// --- Media lane --------------------------------------------------------------

#[test]
fn media_golden_replay_python_and_rust_agree() {
    let golden = golden();
    let out_dir = std::env::temp_dir().join(format!(
        "paradl-parity-media-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out_dir);
    let outcome = run_media_replay(&golden, &fixtures_dir(), &python(), &out_dir).unwrap();
    assert!(
        outcome.report.is_clean(),
        "{}",
        outcome.report.render()
    );
    assert_eq!(outcome.python_frames.len(), 4, "3 progress + 1 result frame");
    assert_eq!(outcome.rust_data, golden["expected_data"]);
    assert_eq!(outcome.rust_jsonl.len(), 3);
    for (line, expected) in outcome.rust_jsonl.iter().zip(golden["expected_progress"].as_array().unwrap()) {
        let mut value: Value = serde_json::from_str(line).unwrap();
        value.as_object_mut().unwrap().remove("timestamp");
        assert_eq!(&value, expected);
    }
}

// --- Non-media lane mechanics ------------------------------------------------

#[test]
fn diff_engine_detects_byte_differences() {
    assert!(diff_bytes("stdout", b"abc\n", b"abc\n").is_none());
    let difference = diff_bytes("stderr", b"ok", b"bad").unwrap();
    assert_eq!(difference.field, "stderr");

    let mut expected = BTreeMap::new();
    expected.insert("clip.m4a.pdl.part".to_owned(), "01ff".to_owned());
    let mut actual = BTreeMap::new();
    actual.insert("clip.m4a.pdl.part".to_owned(), "01fe".to_owned());
    let differences = diff_snapshots(&expected, &actual);
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].field, "file:clip.m4a.pdl.part");

    expected.insert("clip.m4a.pdl.json".to_owned(), "7b7d".to_owned());
    assert_eq!(diff_snapshots(&expected, &actual).len(), 2, "missing sidecar");
}

#[test]
fn raw_stdout_diff_ignores_only_timestamps() {
    let python = b"{\"event\": \"progress\", \"timestamp\": 1795200000.123, \"percent\": 50.0}\n";
    let rust = b"{\"event\": \"progress\", \"timestamp\": 1795200999.987, \"percent\": 50.0}\n";
    let mut clean = DiffReport::new("non-media", "timestamp-only");
    clean.compare(
        "stdout",
        &normalize_stdout_timestamps(python),
        &normalize_stdout_timestamps(rust),
    );
    assert!(clean.is_clean(), "{}", clean.render());

    let rust_different = b"{\"event\": \"progress\", \"timestamp\": 1795200999.987, \"percent\": 49.0}\n";
    let mut dirty = DiffReport::new("non-media", "payload-diff");
    dirty.compare(
        "stdout",
        &normalize_stdout_timestamps(python),
        &normalize_stdout_timestamps(rust_different),
    );
    assert!(!dirty.is_clean(), "a non-timestamp difference must fail");
    assert_eq!(dirty.differences[0].field, "stdout");
}

#[test]
fn result_event_run_dependent_fields_compare_clean_but_other_fields_fail() {
    // (a) Two otherwise-identical streams differing ONLY in the three named
    //     run-dependent fields compare clean in BOTH channels.
    let python = br#"{"event":"result","timestamp":1.1,"duration":0.382,"avg_speed_bps":121.17,"file_path":"/var/folders/x/python/origin.bin","success":true}"#;
    let rust = br#"{"event":"result","timestamp":9.9,"duration":0.042,"avg_speed_bps":1845.64,"file_path":"/private/var/folders/x/rust/origin.bin","success":true}"#;
    let mut clean = DiffReport::new("non-media", "run-dependent-only");
    clean.compare("stdout", &normalize_stdout_fields(python), &normalize_stdout_fields(rust));
    assert!(clean.is_clean(), "{}", clean.render());
    let a = normalize_jsonl(std::str::from_utf8(python).unwrap()).unwrap();
    let b = normalize_jsonl(std::str::from_utf8(rust).unwrap()).unwrap();
    assert_eq!(a, b, "structural JSONL must ignore the three fields too");

    // (b) A genuine difference in a NON-normalized field still fails.
    let rust_different = br#"{"event":"result","timestamp":9.9,"duration":0.042,"avg_speed_bps":1845.64,"file_path":"/private/var/folders/x/rust/origin.bin","bytes_downloaded":34}"#;
    let mut dirty = DiffReport::new("non-media", "non-normalized-diff");
    dirty.compare(
        "stdout",
        &normalize_stdout_fields(python),
        &normalize_stdout_fields(rust_different),
    );
    assert!(!dirty.is_clean(), "a bytes_downloaded difference must fail");
    let c = normalize_jsonl(std::str::from_utf8(rust_different).unwrap()).unwrap();
    assert_ne!(a, c, "structural JSONL must keep other-field differences");
}

#[test]
fn loopback_server_serves_deterministic_bytes() {
    let server = LoopbackServer::start(b"paradl-loopback-golden".to_vec()).unwrap();
    let authority = server.url().strip_prefix("http://").unwrap();
    let authority = authority.split('/').next().unwrap();
    let mut stream = TcpStream::connect(authority).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    write!(stream, "GET /origin.bin HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(response.contains("paradl-loopback-golden"), "{response}");
    assert!(response.contains("200 OK"));
}

// This differential test invoked the Python CLI oracle under `src/paradl/**`
// plus `parallel-dl.py`, both deleted in the W5-1 cutover. It is retired, not
// broken: the final GREEN evidence is preserved in the ledger — full non-media
// gate EXIT 0 with 6/6 cases CLEAN at commit c4e271a (lead independently
// verified, 2026-11-20). `#[ignore]` keeps the code as a historical record and
// allows bare `cargo test --all` to stay green post-cutover; the boundary lane
// (which needs no oracle) remains fully live.
#[test]
#[ignore = "Python CLI oracle deleted in W5-1 cutover; GREEN at c4e271a preserved in ledger"]
fn full_non_media_gate_is_clean() {
    // (c) The complete gate (5 corpus cases + interrupt/resume) must exit 0.
    let gate = run_full_nonmedia_gate(&repo_root()).unwrap();
    assert_eq!(gate.reports.len(), 6, "five corpus cases + interrupt case");
    assert!(gate.is_clean(), "full non-media gate is not clean:\n{}", gate.render());
}

#[test]
fn interrupt_resume_gate_preserves_part_and_sidecar() {
    let report = run_interrupt_resume_gate(interrupt_payload()).unwrap();
    assert!(report.is_clean(), "{}", report.render());
    assert_eq!(report.case, "download-interrupt-resume");
}

#[test]
fn no_python_guard_detects_a_spawn() {
    let dir = std::env::temp_dir().join(format!(
        "paradl-parity-guard-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let guard = PythonGuard::new(&dir).unwrap();

    let output = Command::new("sh")
        .args(["-c", "python3 -c 'pass' >/dev/null 2>&1 || true"])
        .env("PATH", guard.prepend_path(&std::env::var("PATH").unwrap_or_default()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(output.success());
    assert!(guard.triggered(), "the poison python3 shim must have been invoked");
    let _ = std::fs::remove_dir_all(&dir);
}
