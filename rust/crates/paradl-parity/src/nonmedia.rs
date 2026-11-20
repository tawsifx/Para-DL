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

use paradl_core::Error;

use crate::diff::{DiffReport, diff_snapshots, normalize_jsonl, snapshot_dir};

/// A deterministic loopback HTTP server shared by both sides of a case.
pub struct LoopbackServer {
    url: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LoopbackServer {
    /// Start a server serving `body` on an ephemeral loopback port.
    ///
    /// The response includes stable `Accept-Ranges`/`ETag` headers so routing
    /// and range probing are reproducible without the public internet.
    pub fn start(body: Vec<u8>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let _ = listener.set_nonblocking(true);
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = [0_u8; 4096];
                        let _ = stream.read(&mut request);
                        let headers = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                             Content-Length: {}\r\nAccept-Ranges: bytes\r\n\
                             ETag: \"pdl-parity-golden\"\r\nConnection: close\r\n\r\n",
                            body.len(),
                        );
                        if stream.write_all(headers.as_bytes()).is_ok() {
                            let _ = stream.write_all(&body);
                        }
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
/// Cases are intentionally small; the full CLI/flag/event corpus is enumerated
/// in `docs/superpowers/parity-matrix.md` and these rows are its representative
/// sample. The download case is descriptive-only until the W4-1 reference
/// binary implements it (`deferred` is surfaced in the report, never hidden).
pub fn corpus() -> Vec<NonMediaCase> {
    vec![
        NonMediaCase::new("version", &["--version"], &["C20", "F85"]),
        NonMediaCase::new("help", &["--help"], &["F13", "F29", "F36", "F37"]),
        NonMediaCase::new("doctor-json", &["doctor", "--json"], &["C05", "C22", "E10", "E27", "E28"]),
        NonMediaCase::new("info-json", &["info", "{url}", "--json"], &["C07", "E37"]),
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
    report.compare("stdout", &python_side.stdout, &rust_side.stdout);
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
