//! Non-media lane: the main cutover gate. The intact Python CLI and the Rust
//! binary are run with the **same arguments against the same loopback HTTP
//! server**; stdout, stderr, JSONL, produced files (including `.pdl.part` /
//! `.pdl.json` sidecars), and exit codes are compared byte-for-byte.
//!
//! `PARADL_ASSERT_NO_PYTHON=1` is set on the Rust side together with a poisoned
//! `PATH` so any `python`/`python3` spawn is detectable and fails the case
//! (parity matrix X05/X06). The lane exits nonzero on ANY difference.
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use paradl_app::{App, Capture};
use paradl_cli::{Cli, Command as AppCommand, Dispatch, ExitCode, ParseOutcome};
use paradl_core::{Error, PDLConfig};
use paradl_engine::ResumeState;

use crate::diff::{
    DiffReport, Difference, diff_snapshots, normalize_jsonl, normalize_stdout_fields, snapshot_dir,
};

/// A deterministic loopback HTTP server shared by both sides of a case.
pub struct LoopbackServer {
    url: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    /// Start a server serving `body` immediately on an ephemeral loopback port.
    ///
    /// The response includes stable `Accept-Ranges`/`ETag` headers so routing
    /// and range probing are reproducible without the public internet.
    pub fn start(body: Vec<u8>) -> std::io::Result<Self> {
        Self::start_with_delay(body, 0)
    }

    /// Start a server that delays each response by `delay_ms`.
    ///
    /// The delay makes a cooperative interrupt deterministic: it gives the
    /// harness time to set the engine cancel flag while the transfer is live.
    pub fn start_with_delay(body: Vec<u8>, delay_ms: u64) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let bytes = body.clone();
                        thread::spawn(move || serve_one(stream, bytes, delay_ms));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            url: format!("http://127.0.0.1:{port}/origin.bin"),
            stop,
            handle: Some(handle),
        })
    }

    /// The URL both binaries are pointed at.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Serve one loopback request with stable `200`/`206` semantics and no network.
fn serve_one(mut stream: std::net::TcpStream, body: Vec<u8>, delay_ms: u64) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while request.len() < 16 * 1024 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&request);
    let first_line = text.lines().next().unwrap_or("");
    let is_head = first_line.starts_with("HEAD");
    let range = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("range")
            .then(|| value.trim().to_owned())
    });
    let (status, start, end) = match range
        .as_deref()
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(parse_range)
    {
        Some((start, end)) => ("206 Partial Content", start, end.min(body.len().saturating_sub(1))),
        None => ("200 OK", 0, body.len().saturating_sub(1)),
    };
    let slice = if start <= end && end < body.len() {
        &body[start..=end]
    } else {
        &[][..]
    };
    if delay_ms > 0 {
        thread::sleep(Duration::from_millis(delay_ms));
    }
    let mut headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"pdl-parity-golden\"\r\n\
         Connection: close\r\n",
        slice.len(),
    );
    if range.is_some() && start <= end {
        headers.push_str(&format!(
            "Content-Range: bytes {start}-{end}/{}\r\n",
            body.len()
        ));
    }
    headers.push_str("\r\n");
    if stream.write_all(headers.as_bytes()).is_ok() && !is_head {
        let _ = stream.write_all(slice);
    }
    let _ = stream.flush();
}

fn parse_range(value: &str) -> Option<(usize, usize)> {
    let (start, end) = value.split_once('-')?;
    let start = start.trim().parse::<usize>().ok()?;
    let end = match end.trim() {
        "" => usize::MAX,
        text => text.parse::<usize>().ok()?,
    };
    Some((start, end))
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A canary that makes any `python`/`python3` invocation on `PATH` write a
/// marker and exit nonzero, proving the Rust side spawns no Python.
pub struct PythonGuard {
    /// Directory holding the poison shims.
    pub dir: PathBuf,
    /// Marker file written when a poison shim is invoked.
    pub marker: PathBuf,
}

impl PythonGuard {
    /// Create poison shims inside `dir`.
    pub fn new(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let marker = dir.join("python-was-spawned");
        for name in ["python", "python3", "python3.12"] {
            let shim = dir.join(name);
            let script = format!(
                "#!/bin/sh\necho \"$0\" >> \"{}\"\nexit 97\n",
                marker.display()
            );
            std::fs::write(&shim, script)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            marker,
        })
    }

    /// Return a `PATH` value with the poison directory prepended.
    pub fn prepend_path(&self, existing: &str) -> String {
        format!("{}:{existing}", self.dir.display())
    }

    /// Whether a poison shim was invoked.
    pub fn triggered(&self) -> bool {
        self.marker.exists()
    }
}

/// One representative non-media corpus case.
#[derive(Debug, Clone)]
pub struct NonMediaCase {
    /// Stable case name.
    pub name: String,
    /// Arguments passed to both binaries; `{url}`/`{out}` are substituted.
    pub args: Vec<String>,
    /// Parity-matrix rows this case covers.
    pub rows: Vec<String>,
    /// Reason the case is descriptive-only (not yet gated on the Rust side).
    pub deferred: Option<String>,
}

impl NonMediaCase {
    /// Build one case with the given name, args, and row coverage.
    pub fn new(name: &str, args: &[&str], rows: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            rows: rows.iter().map(|row| (*row).to_owned()).collect(),
            deferred: None,
        }
    }
}

/// The representative corpus used by the live gate.
///
/// The full CLI/flag/event corpus is enumerated in
/// `docs/superpowers/parity-matrix.md`; these rows are its representative
/// sample. Since W4-1 landed the engine-backed binary, `download` is a **gated**
/// case: real bytes, `.pdl.part` / `.pdl.json` sidecars, stdout, stderr, JSONL
/// and exit code are all compared byte-for-byte against the Python oracle.
pub fn corpus() -> Vec<NonMediaCase> {
    vec![
        NonMediaCase::new("version", &["--version"], &["C20", "F85"]),
        NonMediaCase::new("help", &["--help"], &["F13", "F29", "F36", "F37"]),
        NonMediaCase::new("doctor-json", &["doctor", "--json"], &["C05", "C22", "E10", "E27", "E28"]),
        NonMediaCase::new("info-json", &["info", "{url}", "--json"], &["C07", "E37"]),
        NonMediaCase::new(
            "download",
            &["{url}", "-o", "{out}", "--json", "--non-interactive"],
            &["C01", "E46", "E56", "E59", "L14"],
        ),
    ]
}

/// Outcome of one non-media case.
#[derive(Debug, Clone)]
pub struct NonMediaOutcome {
    /// Byte-diff report.
    pub report: DiffReport,
    /// Python CLI exit code.
    pub python_exit: i32,
    /// Rust binary exit code.
    pub rust_exit: i32,
}

fn rust_binary(worktree: &Path) -> Result<PathBuf, Error> {
    let binary = worktree.join("rust/target/debug/paradl");
    if binary.is_file() {
        return Ok(binary);
    }
    let python = worktree.join(".venv/bin/python");
    let status = Command::new("cargo")
        .args(["build", "-p", "paradl", "--bin", "paradl"])
        .current_dir(worktree.join("rust"))
        .env("PYO3_PYTHON", &python)
        .status()
        .map_err(|error| Error::failure(format!("cannot build paradl binary: {error}")))?;
    if !status.success() {
        return Err(Error::failure("cargo build -p paradl failed"));
    }
    Ok(binary)
}

/// Captured observable state of one side of a non-media case.
struct SideObservation {
    /// Raw stdout bytes.
    stdout: Vec<u8>,
    /// Raw stderr bytes.
    stderr: Vec<u8>,
    /// Process exit code.
    exit: i32,
    /// Snapshot of produced files and sidecars.
    files: BTreeMap<String, String>,
}

fn run_side(
    program: &Path,
    args: &[String],
    env_pythonpath: Option<&Path>,
    guard: Option<&PythonGuard>,
    home: &Path,
    out_dir: &Path,
) -> Result<SideObservation, Error> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("PARADL_HOME", home)
        .env("PARADL_ASSERT_NO_PYTHON", "1")
        .env_remove("PARADL_MEDIA_PY")
        .current_dir(out_dir);
    if let Some(path) = env_pythonpath {
        command.env("PYTHONPATH", path);
    }
    if let Some(guard) = guard {
        let path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", guard.prepend_path(&path));
    }
    let output = command
        .output()
        .map_err(|error| Error::failure(format!("cannot run {}: {error}", program.display())))?;
    let files = snapshot_dir(out_dir)
        .map_err(|error| Error::failure(format!("cannot snapshot {}: {error}", out_dir.display())))?;
    Ok(SideObservation {
        stdout: output.stdout,
        stderr: output.stderr,
        exit: output.status.code().unwrap_or(-1),
        files,
    })
}

/// Run one non-media case against the worktree oracle and the Rust binary.
pub fn run_nonmedia_case(
    case: &NonMediaCase,
    worktree: &Path,
    server: &LoopbackServer,
) -> Result<NonMediaOutcome, Error> {
    let mut report = DiffReport::new("non-media", case.name.clone());
    report.note(format!(
        "rows: {} (see docs/superpowers/parity-matrix.md)",
        case.rows.join(", ")
    ));
    report.note("both sides use the same loopback server; stdin/args/env matched");
    report.note("byte-exact except timestamp/duration/avg_speed_bps/file_path -> <TS>/<DURATION>/<SPEED>/<PATH>");
    report.note("Rust side: PARADL_ASSERT_NO_PYTHON=1 + poisoned PATH (X05/X06)");
    if let Some(reason) = &case.deferred {
        report.note(format!("deferred: {reason}"));
    }

    let root = std::env::temp_dir().join(format!(
        "paradl-parity-nonmedia-{}-{}",
        std::process::id(),
        case.name,
    ));
    let _ = std::fs::remove_dir_all(&root);
    let python_out = root.join("python");
    let rust_out = root.join("rust");
    let python_home = root.join("python-home");
    let rust_home = root.join("rust-home");
    let guard_dir = root.join("guard");
    for dir in [&python_out, &rust_out, &python_home, &rust_home] {
        std::fs::create_dir_all(dir)
            .map_err(|error| Error::failure(format!("cannot create {}: {error}", dir.display())))?;
    }
    let guard = PythonGuard::new(&guard_dir)
        .map_err(|error| Error::failure(format!("cannot install python guard: {error}")))?;

    let args: Vec<String> = case
        .args
        .iter()
        .map(|arg| arg.replace("{url}", server.url()).replace("{out}", &python_out.display().to_string()))
        .collect();
    let rust_args: Vec<String> = case
        .args
        .iter()
        .map(|arg| arg.replace("{url}", server.url()).replace("{out}", &rust_out.display().to_string()))
        .collect();

    let python_cli = worktree.join("parallel-dl.py");
    let src_dir = worktree.join("src");
    let python = worktree.join(".venv/bin/python");
    let mut python_args = vec![python_cli.display().to_string()];
    python_args.extend(args);
    let python_side = run_side(
        &python,
        &python_args,
        Some(&src_dir),
        None,
        &python_home,
        &python_out,
    )?;
    let rust_side = run_side(
        &rust_binary(worktree)?,
        &rust_args,
        None,
        Some(&guard),
        &rust_home,
        &rust_out,
    )?;
    let python_exit = python_side.exit;
    let rust_exit = rust_side.exit;

    report.compare(
        "exit_code",
        &python_exit.to_string().into_bytes(),
        &rust_exit.to_string().into_bytes(),
    );
    // Raw stdout stays byte-exact EXCEPT for the four named run-dependent
    // fields (timestamp/duration/avg_speed_bps/file_path), replaced by stable
    // tokens. For non-JSONL output the normalizer is a byte-for-byte no-op.
    let python_stdout = normalize_stdout_fields(&python_side.stdout);
    let rust_stdout = normalize_stdout_fields(&rust_side.stdout);
    report.compare("stdout", &python_stdout, &rust_stdout);
    report.compare("stderr", &python_side.stderr, &rust_side.stderr);
    if guard.triggered() {
        report.differences.push(crate::diff::Difference {
            field: "process:python_spawn".to_owned(),
            expected: "<none>".to_owned(),
            actual: "python was spawned on a non-media path".to_owned(),
        });
    }

    // JSONL is compared both raw (above) and structurally (timestamps ignored).
    match (
        normalize_jsonl(&String::from_utf8_lossy(&python_side.stdout)),
        normalize_jsonl(&String::from_utf8_lossy(&rust_side.stdout)),
    ) {
        (Ok(expected), Ok(actual)) if expected == actual => {}
        (Ok(expected), Ok(actual)) => {
            report.differences.push(crate::diff::Difference {
                field: "jsonl:normalized".to_owned(),
                expected: format!("{expected:?}"),
                actual: format!("{actual:?}"),
            });
        }
        _ => {} // non-JSONL stdout is already covered by the raw byte diff
    }
    report
        .differences
        .extend(diff_snapshots(&python_side.files, &rust_side.files));

    Ok(NonMediaOutcome {
        report,
        python_exit,
        rust_exit,
    })
}

/// Deterministic payload used by the interrupt/resume gate.
pub fn interrupt_payload() -> Vec<u8> {
    (0..96 * 1024).map(|index| (index % 251) as u8).collect()
}

/// Interrupt/resume gate: drive the **real** engine through the online app
/// entry point, cancel it cooperatively mid-transfer, and assert that exit code
/// `130` and byte-exact `.pdl.part` / `.pdl.json` sidecars survive.
///
/// This is a Rust-side contract gate (a Python process cannot be cancelled
/// through the app's cooperative flag deterministically); it complements the
/// two-sided `download` corpus case rather than replacing any comparison.
pub fn run_interrupt_resume_gate(body: Vec<u8>) -> Result<DiffReport, Error> {
    let server = LoopbackServer::start_with_delay(body.clone(), 350)?;
    let out = std::env::temp_dir().join(format!(
        "paradl-parity-interrupt-{}-{}",
        std::process::id(),
        body.len(),
    ));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out)
        .map_err(|error| Error::failure(format!("cannot create {}: {error}", out.display())))?;

    let parsed = Cli::parse_command_line(vec![
        server.url().to_owned(),
        "-o".to_owned(),
        out.display().to_string(),
        "--json".to_owned(),
    ])
    .map_err(|error| Error::failure(format!("cannot parse interrupt command: {error}")))?;
    let ParseOutcome::Command(cli) = parsed else {
        return Err(Error::failure("interrupt command did not parse to a Cli"));
    };
    let command = AppCommand::from_cli(&cli)
        .map_err(|error| Error::failure(format!("cannot build download command: {error}")))?;

    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        flag.store(true, Ordering::Relaxed);
    });

    let capture = Capture::default();
    let mut app = App::with_config(PDLConfig::default()).with_output(capture.sink());
    app.disable_history_recording();
    app.set_cancel(cancel);
    let code = app.handle(&command, &cli);
    canceller
        .join()
        .map_err(|_| Error::failure("cancel thread panicked"))?;

    let mut report = DiffReport::new("non-media", "download-interrupt-resume");
    report.note("real paradl-engine transfer via App::with_config + cooperative cancel");
    report.note("no offline shim; no Python spawn on this non-media path");
    report.compare(
        "exit_code",
        ExitCode::Interrupted.as_i32().to_string().as_bytes(),
        code.as_i32().to_string().as_bytes(),
    );
    let events = capture.contents();
    if !events.contains("\"exit_code\": 130") {
        report.differences.push(Difference {
            field: "jsonl:exit_code".to_owned(),
            expected: "\"exit_code\": 130".to_owned(),
            actual: preview_events(&events),
        });
    }

    let part = out.join("origin.bin.pdl.part");
    let sidecar = out.join("origin.bin.pdl.json");
    let state = if sidecar.is_file() {
        let first = std::fs::read(&sidecar)
            .map_err(|error| Error::failure(format!("cannot read sidecar: {error}")))?;
        let second = std::fs::read(&sidecar)
            .map_err(|error| Error::failure(format!("cannot re-read sidecar: {error}")))?;
        if first != second {
            report.differences.push(Difference {
                field: "sidecar:pdl.json stability".to_owned(),
                expected: "byte-identical on repeated read".to_owned(),
                actual: "<unstable>".to_owned(),
            });
        }
        match ResumeState::load(&sidecar) {
            Ok(state) => {
                report.compare(
                    "sidecar:size",
                    body.len().to_string().as_bytes(),
                    state.size.to_string().as_bytes(),
                );
                report.compare(
                    "sidecar:url",
                    server.url().as_bytes(),
                    state.url.as_str().as_bytes(),
                );
                report.compare(
                    "sidecar:dest",
                    out.join("origin.bin").display().to_string().as_bytes(),
                    state.dest.display().to_string().as_bytes(),
                );
                Some(state)
            }
            Err(error) => {
                report.differences.push(Difference {
                    field: "sidecar:pdl.json parse".to_owned(),
                    expected: "valid ResumeState v1".to_owned(),
                    actual: error.to_string(),
                });
                None
            }
        }
    } else {
        report.differences.push(Difference {
            field: "sidecar:pdl.json".to_owned(),
            expected: "<present>".to_owned(),
            actual: "<missing>".to_owned(),
        });
        None
    };

    if !part.is_file() {
        report.differences.push(Difference {
            field: "sidecar:pdl.part".to_owned(),
            expected: "<present>".to_owned(),
            actual: "<missing>".to_owned(),
        });
    } else {
        let bytes = std::fs::read(&part)
            .map_err(|error| Error::failure(format!("cannot read part file: {error}")))?;
        if bytes.is_empty() {
            report.differences.push(Difference {
                field: "sidecar:pdl.part bytes".to_owned(),
                expected: "non-empty part file".to_owned(),
                actual: "<empty>".to_owned(),
            });
        }
        // The part file may be pre-allocated to the full size with out-of-order
        // segments. Verify every range the sidecar claims against the source,
        // exactly as the engine's owning tests do; an early interrupt is
        // allowed to leave the segment table empty.
        if let Some(state) = state {
            for segment in &state.segments {
                let (start, cursor) = (segment.start as usize, segment.cursor as usize);
                if cursor > start && (cursor > bytes.len() || bytes[start..cursor] != body[start..cursor]) {
                    report.differences.push(Difference {
                        field: format!("sidecar:pdl.part bytes[{start}..{cursor}]"),
                        expected: "source payload bytes".to_owned(),
                        actual: "part bytes differ".to_owned(),
                    });
                    break;
                }
            }
        }
    }
    Ok(report)
}

fn preview_events(events: &str) -> String {
    events.chars().take(200).collect()
}

/// Result of the complete non-media gate (the five corpus cases plus the
/// interrupt/resume case). Clean only when every report is clean.
#[derive(Debug)]
pub struct NonMediaGate {
    /// One report per case, in execution order.
    pub reports: Vec<DiffReport>,
}

impl NonMediaGate {
    /// Whether every case is clean.
    pub fn is_clean(&self) -> bool {
        self.reports.iter().all(DiffReport::is_clean)
    }

    /// Render every case report into one concise string.
    pub fn render(&self) -> String {
        self.reports
            .iter()
            .map(DiffReport::render)
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Run the complete non-media gate against the worktree oracle and the real
/// Rust binary. This is the reusable entry point used by both the CLI and the
/// `full_non_media_gate_is_clean` regression test.
pub fn run_full_nonmedia_gate(root: &Path) -> Result<NonMediaGate, Error> {
    let server = LoopbackServer::start(b"paradl-parity-loopback-golden-bytes".to_vec())?;
    let mut reports = Vec::new();
    for case in corpus() {
        reports.push(run_nonmedia_case(&case, root, &server)?.report);
    }
    reports.push(run_interrupt_resume_gate(interrupt_payload())?);
    Ok(NonMediaGate { reports })
}
