//! Boundary lane: a stub Python worker emits canned frames and the **real**
//! Rust media bridge parses, re-emits, maps exit codes, and fails closed.
//!
//! No extractor logic and no live network are involved. The stub is launched
//! through [`WorkerLaunch::new`] so the exact production path in
//! `paradl-media` (handshake, correlation, single-emitter JSONL, `error.kind`
//! mapping) is exercised. The "version-mismatch" case is the negative test
//! referenced by CI and must exit `4`.
#![warn(missing_docs)]

use std::path::Path;

use paradl_core::{Error, ExitCode};
use paradl_media::{
    MediaData, MediaRequest, MediaResolver, PROTOCOL_ID, WorkerFrame, WorkerLaunch,
    WorkerVersions, WorkerMediaResolver,
};
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("../fixtures/ytdlp_j_golden.json");

/// Canned worker response exercised by the boundary lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryCase {
    /// Valid progress + result frames (expect exit `0`).
    Ok,
    /// Terminal `error.kind=download` frame (expect exit `1`).
    Error,
    /// Unknown `error.kind` (must fail closed to exit `1`).
    Unsupported,
    /// Chatty/non-JSON stdout (must fail closed without emitting JSONL).
    Malformed,
    /// Response frame carrying an unknown protocol major (must exit `4`).
    VersionMismatch,
}

impl BoundaryCase {
    /// Stable CLI name of this case (matches the CI `--case` argument).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Unsupported => "unsupported",
            Self::Malformed => "malformed",
            Self::VersionMismatch => "version-mismatch",
        }
    }

    /// Parse a CLI case name.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "ok" => Some(Self::Ok),
            "error" => Some(Self::Error),
            "unsupported" => Some(Self::Unsupported),
            "malformed" => Some(Self::Malformed),
            "version-mismatch" => Some(Self::VersionMismatch),
            _ => None,
        }
    }
}

/// Outcome of one boundary case.
#[derive(Debug, Clone)]
pub struct BoundaryOutcome {
    /// Case that was run.
    pub case: BoundaryCase,
    /// Exit code the Rust bridge produced.
    pub exit_code: i32,
    /// Human diagnostic (worker message or parser fault).
    pub diagnostic: String,
    /// Redacted progress JSONL lines re-emitted by Rust.
    pub jsonl_lines: Vec<String>,
    /// Decoded terminal `data` (null when the case did not reach a result).
    pub data: Value,
    /// Worker/tool versions from the terminal frame.
    pub versions: Value,
    /// Capabilities advertised by the worker.
    pub caps: Vec<String>,
}

/// Generate a Python stub that reads one request line and replays `frames`,
/// substituting the bridge-generated `request_id` into every frame.
///
/// The `claims` sub-op (which the bridge always issues before `resolve` /
/// `download_managed`) receives a minimal successful result so the case under
/// test is reached; that claims frame carries the same protocol override, so a
/// version mismatch still fails closed on the first frame.
///
/// The returned script is reused by the media lane to replay golden frames
/// through the same bridge path.
pub fn frames_worker_script(frames: &[Value], protocol: &str, versions: &WorkerVersions) -> String {
    let frames_json = serde_json::to_string(frames).expect("frames are serializable");
    let claims_json = serde_json::to_string(&[json!({
        "protocol": protocol,
        "frame": "result",
        "ok": true,
        "versions": versions,
        "caps": [],
        "data": {"claims": true},
    })])
    .expect("claims frame is serializable");
    // Rust's `{:?}` escapes a string into a double-quoted literal that Python
    // parses identically; `json.loads` then restores true/null safely.
    format!(
        "import json,sys\n\
         request=json.loads(sys.stdin.readline() or '{{}}')\n\
         rid=request.get('request_id','unknown')\n\
         if request.get('op')=='claims':\n\
         \x20   frames=json.loads({claims_json:?})\n\
         else:\n\
         \x20   frames=json.loads({frames_json:?})\n\
         for frame in frames:\n\
         \x20   frame=dict(frame); frame['request_id']=rid\n\
         \x20   sys.stdout.write(json.dumps(frame,separators=(',',':'))+'\\n'); sys.stdout.flush()\n"
    )
}

/// Generate a Python stub that writes raw (possibly non-JSON) stdout lines.
fn raw_worker_script(lines: &[&str]) -> String {
    let mut script = String::from("import sys\n");
    for line in lines {
        script.push_str(&format!(
            "sys.stdout.write({:?}+'\\n'); sys.stdout.flush()\n",
            line
        ));
    }
    script
}

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("embedded golden is valid JSON")
}

fn golden_versions() -> WorkerVersions {
    serde_json::from_value(golden()["versions"].clone()).expect("golden versions are well formed")
}

fn progress_frame(protocol: &str, downloaded: u64, total: u64, speed: f64, eta: f64) -> Value {
    json!({
        "protocol": protocol,
        "frame": "progress",
        "downloaded_bytes": downloaded,
        "total_bytes": total,
        "speed_bps": speed,
        "eta_seconds": eta,
    })
}

/// Run one boundary case against the stub worker launched by `worker_python`.
///
/// `protocol_override` replaces the protocol string on emitted frames; when it
/// is `None`, cases use `paradl-media/1` except `VersionMismatch`, which uses
/// an unknown major (`paradl-media/9`).
///
/// ```
/// use std::path::Path;
/// use paradl_parity::{BoundaryCase, run_boundary_case};
/// let outcome = run_boundary_case(
///     BoundaryCase::VersionMismatch, Path::new("python3"), None,
/// ).unwrap();
/// assert_eq!(outcome.exit_code, 4);
/// ```
pub fn run_boundary_case(
    case: BoundaryCase,
    worker_python: &Path,
    protocol_override: Option<&str>,
) -> Result<BoundaryOutcome, Error> {
    let default_protocol = match case {
        BoundaryCase::VersionMismatch => "paradl-media/9",
        _ => PROTOCOL_ID,
    };
    let protocol = protocol_override.unwrap_or(default_protocol);
    let versions = golden_versions();

    let script = match case {
        BoundaryCase::Ok => {
            let frames = vec![
                progress_frame(protocol, 50, 100, 1000.0, 1.0),
                progress_frame(protocol, 100, 100, 1000.0, 0.0),
                json!({
                    "protocol": protocol,
                    "frame": "result",
                    "ok": true,
                    "versions": versions,
                    "caps": ["ejs", "mutagen", "ffmpeg"],
                    "data": golden()["expected_data"],
                }),
            ];
            frames_worker_script(&frames, protocol, &versions)
        }
        BoundaryCase::Error => frames_worker_script(
            &[json!({
                "protocol": protocol,
                "frame": "error",
                "ok": false,
                "error": {"kind": "download", "message": "canned download failure"},
                "versions": versions,
            })],
            protocol,
            &versions,
        ),
        BoundaryCase::Unsupported => frames_worker_script(
            &[json!({
                "protocol": protocol,
                "frame": "error",
                "ok": false,
                "error": {"kind": "bogus_kind", "message": "unsupported worker error kind: bogus_kind"},
                "versions": versions,
            })],
            protocol,
            &versions,
        ),
        BoundaryCase::Malformed => {
            raw_worker_script(&["[worker] human log that must stay off --json", "this-is-not-json"])
        }
        BoundaryCase::VersionMismatch => frames_worker_script(
            &[json!({
                "protocol": protocol,
                "frame": "result",
                "ok": true,
                "versions": versions,
                "caps": ["ejs"],
                "data": golden()["expected_data"],
            })],
            protocol,
            &versions,
        ),
    };

    let request = MediaRequest::new(
        "https://example.com/finalize",
        std::env::temp_dir().join(format!("paradl-parity-boundary-{}", std::process::id())),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (result, jsonl_lines) = runtime.block_on(async {
        let launch = WorkerLaunch::new(worker_python, ["-c".to_owned(), script]);
        let resolver = WorkerMediaResolver::with_launch(launch);
        let mut jsonl_lines = Vec::new();
        let mut sink = |line: &str| jsonl_lines.push(line.to_owned());
        let result = resolver.download_managed(request, &mut sink).await;
        (result, jsonl_lines)
    });

    Ok(match result {
        Ok(result) => BoundaryOutcome {
            case,
            exit_code: ExitCode::Ok.as_i32(),
            diagnostic: String::new(),
            jsonl_lines,
            data: result
                .data
                .map(|data| serde_json::to_value(data).unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
            versions: serde_json::to_value(result.versions).unwrap_or(Value::Null),
            caps: result.caps,
        },
        Err(error) => BoundaryOutcome {
            case,
            exit_code: error.exit_code().as_i32(),
            diagnostic: error.to_string(),
            jsonl_lines,
            data: Value::Null,
            versions: Value::Null,
            caps: Vec::new(),
        },
    })
}

/// Verify that one raw frame parses through the real bridge parser.
///
/// Returns the mapped exit code (`0` on success, otherwise the [`ExitCode`]
/// from the parse error); used by version-compatibility tests.
pub fn parse_frame_exit_code(line: &str, request_id: &str) -> i32 {
    match WorkerFrame::parse(line, request_id) {
        Ok(_) => ExitCode::Ok.as_i32(),
        Err(error) => error.exit_code().as_i32(),
    }
}

/// Decode a terminal `data` object with the bridge's byte-exact coercion.
pub fn decode_media_data(value: &Value) -> Result<MediaData, Error> {
    MediaData::from_value(value)
}
