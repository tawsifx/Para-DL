//! Thin reference binary for the paraDL Rust framework.
//!
//! Wave 0 placeholder: `--version` and a minimal, Python-free `doctor` are
//! implemented. The full command set is wired in W4-1 via `paradl-app`. The
//! media subsystem is a Python worker invoked over `paradl-media/1`, never
//! linked into Rust.
#![warn(missing_docs)]

use std::path::Path;

/// Framework version, kept in sync with `[workspace.package].version`.
pub const VERSION: &str = "0.1.0";

/// Protocol spoken with the Python media worker.
pub const MEDIA_PROTOCOL: &str = "paradl-media/1";

/// Derive the command name from `argv[0]` so the `parallel-dl` symlink reports
/// its own name in `--version` output.
fn program_name(arg0: &str) -> String {
    // Handle both separators so the test is portable across the macOS and
    // Windows CI runners (Cargo also strips a `.exe` suffix only on Windows).
    let base = arg0.rsplit(['/', '\\']).next().unwrap_or(arg0);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    let base = Path::new(base)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(base);
    if base.is_empty() {
        "paradl".to_string()
    } else {
        base.to_string()
    }
}

/// Render the exact `--version` line.
fn version_line(program: &str) -> String {
    format!("{program} {VERSION}")
}

/// Render the minimal `doctor` report: no Python, no external tools, no I/O.
/// W4-1 expands this into the full diagnostics subcommand.
fn doctor_line() -> String {
    format!("ready version={VERSION} protocol={MEDIA_PROTOCOL}")
}

fn main() {
    let mut args = std::env::args();
    let arg0 = args.next().unwrap_or_else(|| "paradl".to_string());
    let program = program_name(&arg0);

    match args.next().as_deref() {
        Some("--version") | Some("-V") | None => println!("{}", version_line(&program)),
        Some("doctor") => println!("{}", doctor_line()),
        Some("--help") | Some("-h") => {
            println!("{program} {VERSION}");
            println!("Usage: {program} [COMMAND] [OPTIONS]");
            println!("  doctor          Print minimal diagnostics (no Python required)");
            println!("  -V, --version   Print version and exit");
            println!("  -h, --help      Print help and exit");
        }
        Some(other) => {
            eprintln!("{program}: '{other}' is not implemented yet (Wave 0 placeholder)");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_workspace_version() {
        assert_eq!(version_line("paradl"), "paradl 0.1.0");
        assert_eq!(version_line("parallel-dl"), "parallel-dl 0.1.0");
    }

    #[test]
    fn symlink_name_is_preserved() {
        assert_eq!(program_name("/opt/homebrew/bin/parallel-dl"), "parallel-dl");
        assert_eq!(program_name("C:\\bin\\paradl.exe"), "paradl");
        assert_eq!(program_name("/usr/local/bin/paradl"), "paradl");
    }

    #[test]
    fn protocol_constant_is_pinned() {
        assert_eq!(MEDIA_PROTOCOL, "paradl-media/1");
    }

    #[test]
    fn doctor_reports_ready_version_and_protocol_without_python() {
        let report = doctor_line();
        assert!(report.starts_with("ready "), "{report}");
        assert!(report.contains("version=0.1.0"), "{report}");
        assert!(report.contains("protocol=paradl-media/1"), "{report}");
    }
}
