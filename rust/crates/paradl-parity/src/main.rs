//! CLI for the three-lane differential harness.
//!
//! ```text
//! paradl-parity boundary --case version-mismatch --worker <python>
//! paradl-parity non-media [--worktree <path>]
//! paradl-parity media [--python <path>]
//! paradl-parity all
//! ```
//!
//! Every lane prints a concise diff report and exits nonzero on any unexpected
//! difference. The `boundary` lane additionally returns the **mapped** exit
//! code, which is what the CI protocol-mismatch negative test asserts (`4`).
#![warn(missing_docs)]

use std::path::PathBuf;

use paradl_parity::{
    BoundaryCase, DiffReport, run_boundary_case, run_media_replay, run_nonmedia_case,
};
use paradl_parity::diff::Difference;
use paradl_parity::media::find_media_python;
use paradl_parity::nonmedia::corpus;

fn worktree() -> PathBuf {
    if let Ok(value) = std::env::var("PARADL_WORKTREE") {
        return PathBuf::from(value);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn python_path() -> PathBuf {
    find_media_python(Some(&worktree().join(".venv/bin/python")))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn expected_exit_code(case: BoundaryCase) -> i32 {
    match case {
        BoundaryCase::Ok => 0,
        BoundaryCase::Error => 1,
        BoundaryCase::Unsupported => 1,
        BoundaryCase::Malformed => 1,
        BoundaryCase::VersionMismatch => 4,
    }
}

fn run_boundary(args: &[String]) -> i32 {
    let case_name = flag(args, "--case").unwrap_or_else(|| "ok".to_owned());
    let Some(case) = BoundaryCase::parse(&case_name) else {
        eprintln!("unknown boundary case: {case_name}");
        return 2;
    };
    let worker = flag(args, "--worker")
        .map(PathBuf::from)
        .unwrap_or_else(python_path);
    let protocol = flag(args, "--protocol");
    match run_boundary_case(case, &worker, protocol.as_deref()) {
        Ok(outcome) => {
            let mut report = DiffReport::new("boundary", case.as_str());
            let expected = expected_exit_code(case);
            if outcome.exit_code != expected {
                report.differences.push(Difference {
                    field: "exit_code".to_owned(),
                    expected: expected.to_string(),
                    actual: outcome.exit_code.to_string(),
                });
            }
            if case == BoundaryCase::VersionMismatch
                && !outcome.diagnostic.contains("protocol")
            {
                report.differences.push(Difference {
                    field: "diagnostic".to_owned(),
                    expected: "protocol diagnostic".to_owned(),
                    actual: outcome.diagnostic.clone(),
                });
            }
            report.note(format!("mapped exit code={}; diagnostic={:?}", outcome.exit_code, outcome.diagnostic));
            print!("{}", report.render());
            // The CI negative test asserts the mapped exit code, not a pass/fail.
            outcome.exit_code
        }
        Err(error) => {
            eprintln!("boundary lane error: {error}");
            1
        }
    }
}

fn run_nonmedia(args: &[String]) -> i32 {
    let root = flag(args, "--worktree").map(PathBuf::from).unwrap_or_else(worktree);
    let body = b"paradl-parity-loopback-golden-bytes".to_vec();
    let Ok(server) = paradl_parity::nonmedia::LoopbackServer::start(body) else {
        eprintln!("cannot start loopback server");
        return 1;
    };
    let mut dirty = false;
    for case in corpus() {
        match run_nonmedia_case(&case, &root, &server) {
            Ok(outcome) => {
                print!("{}", outcome.report.render());
                if !outcome.report.is_clean() {
                    dirty = true;
                }
            }
            Err(error) => {
                eprintln!("[non-media/{}] error: {error}", case.name);
                dirty = true;
            }
        }
    }
    if dirty { 1 } else { 0 }
}

fn run_media(args: &[String]) -> i32 {
    let root = worktree();
    let fixtures = root.join("rust/crates/paradl-parity/fixtures");
    let python = flag(args, "--python")
        .map(PathBuf::from)
        .unwrap_or_else(python_path);
    let Ok(golden_text) = std::fs::read_to_string(fixtures.join("ytdlp_j_golden.json")) else {
        eprintln!("missing golden fixture");
        return 1;
    };
    let Ok(golden) = serde_json::from_str(&golden_text) else {
        eprintln!("invalid golden fixture");
        return 1;
    };
    let out_dir = std::env::temp_dir().join(format!("paradl-parity-media-cli-{}", std::process::id()));
    match run_media_replay(&golden, &fixtures, &python, &out_dir) {
        Ok(outcome) => {
            print!("{}", outcome.report.render());
            println!("  startup_ms={} (diagnostic only)", outcome.startup_millis);
            if outcome.report.is_clean() { 0 } else { 1 }
        }
        Err(error) => {
            eprintln!("media lane error: {error}");
            1
        }
    }
}

fn usage() {
    eprintln!("usage: paradl-parity <boundary|non-media|media|all> [options]");
    eprintln!("  boundary --case <ok|error|unsupported|malformed|version-mismatch> --worker <python>");
    eprintln!("  non-media [--worktree <path>]");
    eprintln!("  media [--python <path>]");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("boundary") => run_boundary(&args[1..]),
        Some("non-media") | Some("non_media") => run_nonmedia(&args[1..]),
        Some("media") => run_media(&args[1..]),
        Some("all") => {
            let boundary = run_boundary(&["--case".to_owned(), "version-mismatch".to_owned()]);
            let nonmedia = run_nonmedia(&[]);
            let media = run_media(&[]);
            // The boundary lane's 4 is the *expected* mapping, not a failure.
            let mut code = nonmedia.max(media);
            if boundary != 4 {
                code = 1;
            }
            code
        }
        _ => {
            usage();
            2
        }
    };
    std::process::exit(code);
}
