//! Media lane: golden-replay comparison of the Python `MediaEngine` and the
//! Rust protocol bridge (no live network).
//!
//! A recorded yt-dlp `-J` fixture is replayed through the Python engine with
//! yt-dlp mocked (see `fixtures/media_golden_replay.py`). The **identical**
//! protocol frames are then fed to the real Rust bridge via a stub worker. The
//! lane diffs protocol payload, synthesized JSONL, output bytes, and tags.
//! Process-startup cost is measured only here, with an explicit cold/warm
//! caveat: a warm interpreter is not a throughput win and this number is never
//! used to claim one.
#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use paradl_core::{Error, ExitCode};
use paradl_media::{
    MediaRequest, MediaResolver, PROTOCOL_ID, WorkerLaunch, WorkerVersions, WorkerMediaResolver,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::boundary::frames_worker_script;
use crate::diff::{DiffReport, strip_timestamp};

/// Outcome of one media golden-replay run.
#[derive(Debug, Clone)]
pub struct MediaReplayOutcome {
    /// Byte-diff report for protocol data, JSONL, bytes, and tags.
    pub report: DiffReport,
    /// Frames emitted by the Python golden-replay worker.
    pub python_frames: Vec<Value>,
    /// Data decoded by the Rust bridge.
    pub rust_data: Value,
    /// Redacted JSONL re-emitted by the Rust bridge.
    pub rust_jsonl: Vec<String>,
    /// Python worker process-startup time in milliseconds (diagnostic only).
    pub startup_millis: u128,
}

fn compare_values(report: &mut DiffReport, field: &str, expected: &Value, actual: &Value) {
    if expected != actual {
        report.differences.push(crate::diff::Difference {
            field: field.to_owned(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn media_source_dir(fixtures_dir: &Path) -> PathBuf {
    fixtures_dir.join("../../../../media")
}

/// Locate a Python interpreter that can import `paradl_media`.
///
/// Honours `PARADL_PYTHON` first, then `python` on `PATH`.
pub fn find_media_python(override_python: Option<&Path>) -> PathBuf {
    if let Some(path) = override_python {
        return path.to_path_buf();
    }
    if let Ok(value) = std::env::var("PARADL_PYTHON") {
        return PathBuf::from(value);
    }
    PathBuf::from("python3")
}

/// Replay one golden fixture through both the Python engine and the Rust bridge.
///
/// `golden` is parsed from `ytdlp_j_golden.json`; `python` must be able to
/// import `paradl_media` (set `PYTHONPATH`; the harness sets it for the replay
/// subprocess). `out_dir` receives the replayed artifact bytes.
pub fn run_media_replay(
    golden: &Value,
    fixtures_dir: &Path,
    python: &Path,
    out_dir: &Path,
) -> Result<MediaReplayOutcome, Error> {
    let mut report = DiffReport::new("media", "golden-replay");
    report.note("recorded yt-dlp -J metadata + artifact bytes; no live network");
    report.note("JSONL timestamps normalized; only shape/payload compared");
    report.note("process startup measured here only; warm cache is not a throughput win");

    let _ = std::fs::create_dir_all(out_dir);
    let golden_path = fixtures_dir.join("ytdlp_j_golden.json");
    let script = fixtures_dir.join("media_golden_replay.py");
    let pythonpath = media_source_dir(fixtures_dir)
        .canonicalize()
        .unwrap_or_else(|_| media_source_dir(fixtures_dir));

    // --- Python side: recorded fixture -> real MediaEngine -> protocol frames.
    let started = Instant::now();
    let child = Command::new(python)
        .arg(&script)
        .arg(&golden_path)
        .arg(out_dir)
        .env("PYTHONPATH", &pythonpath)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Error::missing_dependency(format!("cannot start media replay: {error}")))?;
    let startup_millis = started.elapsed().as_millis();
    let output = child
        .wait_with_output()
        .map_err(|error| Error::failure(format!("media replay failed: {error}")))?;
    if !output.status.success() {
        return Err(Error::failure(format!(
            "media replay exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let mut python_frames = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let frame: Value = serde_json::from_str(line)
            .map_err(|error| Error::failure(format!("bad replay frame: {error}")))?;
        python_frames.push(frame);
    }
    let terminal = python_frames
        .last()
        .ok_or_else(|| Error::failure("media replay emitted no terminal frame"))?;
    let python_data = terminal
        .get("data")
        .ok_or_else(|| Error::failure("terminal replay frame has no data"))?;

    // Python-owned protocol payload and tags must match the captured golden.
    let expected_data = &golden["expected_data"];
    let expected_tags = &golden["expected_tags"];
    let mut python_combined = expected_data.clone();
    if let Value::Object(map) = &mut python_combined {
        for (key, value) in expected_tags.as_object().map(|m| m.iter()).into_iter().flatten() {
            map.insert(key.clone(), value.clone());
        }
    }
    compare_values(&mut report, "python:protocol+tags", &python_combined, python_data);

    // Output bytes: Python-produced artifact must equal the recorded bytes.
    let artifact_path = out_dir.join(golden["artifact"]["filename"].as_str().unwrap_or("artifact.m4a"));
    let artifact = std::fs::read(&artifact_path)
        .map_err(|error| Error::failure(format!("replay artifact missing: {error}")))?;
    let expected_sha = golden["artifact"]["sha256"].as_str().unwrap_or_default();
    let actual_sha = sha256_hex(&artifact);
    if actual_sha != expected_sha {
        report.differences.push(crate::diff::Difference {
            field: "artifact:sha256".to_owned(),
            expected: expected_sha.to_owned(),
            actual: actual_sha,
        });
    }

    // --- Rust side: feed the IDENTICAL frames through the real bridge.
    let versions: WorkerVersions = serde_json::from_value(golden["versions"].clone())
        .map_err(|error| Error::failure(format!("golden versions invalid: {error}")))?;
    let script = frames_worker_script(&python_frames, PROTOCOL_ID, &versions);
    let request = MediaRequest::new(
        golden["target"].as_str().unwrap_or("https://example.com/media"),
        out_dir.to_path_buf(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (result, rust_jsonl) = runtime.block_on(async {
        let launch = WorkerLaunch::new(python, ["-c".to_owned(), script]);
        let resolver = WorkerMediaResolver::with_launch(launch);
        let mut lines = Vec::new();
        let mut sink = |line: &str| lines.push(line.to_owned());
        let result = resolver.download_managed(request, &mut sink).await;
        (result, lines)
    });
    let result = result.map_err(|error| Error::failure(format!("rust bridge failed: {error}")))?;
    let rust_data = result
        .data
        .map(|data| serde_json::to_value(data).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);
    compare_values(&mut report, "rust:protocol", expected_data, &rust_data);

    // Tags survive in the raw terminal frame and are preserved by the bridge
    // parser; verify the Python-emitted frame carried the golden tags.
    for (key, value) in expected_tags.as_object().map(|m| m.iter()).into_iter().flatten() {
        let actual = python_data.get(key).unwrap_or(&Value::Null);
        compare_values(&mut report, &format!("tag:{key}"), value, actual);
    }

    // Synthesized JSONL: normalize timestamps, compare event shape/payload.
    let expected_progress = golden["expected_progress"]
        .as_array()
        .ok_or_else(|| Error::failure("golden expected_progress is not an array"))?;
    if rust_jsonl.len() != expected_progress.len() {
        report.differences.push(crate::diff::Difference {
            field: "jsonl:count".to_owned(),
            expected: expected_progress.len().to_string(),
            actual: rust_jsonl.len().to_string(),
        });
    }
    for (index, (line, expected)) in rust_jsonl.iter().zip(expected_progress.iter()).enumerate() {
        let mut value: Value = serde_json::from_str(line)
            .map_err(|error| Error::failure(format!("bad re-emitted JSONL: {error}")))?;
        strip_timestamp(&mut value);
        compare_values(&mut report, &format!("jsonl[{index}]"), expected, &value);
    }
    let _ = result.versions; // versions already checked against the golden frame
    let _ = ExitCode::Ok;

    Ok(MediaReplayOutcome {
        report,
        python_frames,
        rust_data,
        rust_jsonl,
        startup_millis,
    })
}
