//! `misterdownloadmanager-cli`: a launcher. The commands are in the app's program (lib.rs, #25:
//! one download engine shipped, not two); this runs `misterdownloadmanager --cli <arguments>`
//! from its own folder, on the same terminal, input and output (a file, a pipe), and ends with
//! its exit code. It links nothing of lib.rs but [`mdm_cli::FLAG`], so it stays small.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let Some(program) = program() else {
        fail(
            json,
            "the app's program (misterdownloadmanager) isn't next to misterdownloadmanager-cli",
        )
    };
    let e = run(&program, args);
    fail(json, &format!("can't start {} ({e})", program.display()))
}

/// The app's program in this launcher's folder (the installer's or a zip's, a tarball's,
/// /usr/bin, a Mac app's Contents/MacOS); started through a link, in the folder it points into.
fn program() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = format!("misterdownloadmanager{}", std::env::consts::EXE_SUFFIX);
    [Some(exe.clone()), std::fs::canonicalize(&exe).ok()]
        .into_iter()
        .flatten()
        .map(|e| e.with_file_name(&name))
        .find(|p| p.is_file())
}

/// Linux and macOS: this process becomes the program (exec), so a terminal's Ctrl+C, a `kill`,
/// the process id and the exit code are the program's own. Returns only if it can't.
#[cfg(unix)]
fn run(program: &std::path::Path, args: Vec<OsString>) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    Command::new(program).arg(mdm_cli::FLAG).args(args).exec()
}

/// Windows: the program runs as a child, on this console. Ctrl+C reaches both: the program pauses
/// its download and ends, and this launcher, which ignores it, waits for that and passes its exit
/// code on. Given this process's id (`--cli=<id>`), the program ends if the launcher is killed.
#[cfg(windows)]
fn run(program: &std::path::Path, args: Vec<OsString>) -> std::io::Error {
    // A handler that does nothing: Ctrl+C and Ctrl+Break don't end this process. Unlike
    // ignoring them outright, it isn't inherited by the program.
    let _ = ctrlc::set_handler(|| {});
    let launcher = format!("{}={}", mdm_cli::FLAG, std::process::id());
    match Command::new(program).arg(launcher).args(args).status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => e,
    }
}

/// What failed, as the commands report it (`Error: …`, or `{"error": …}` with `--json`); exit 1.
fn fail(json: bool, why: &str) -> ! {
    if json {
        let mut s = String::from("{\"error\":\"");
        for c in why.chars() {
            match c {
                '"' => s.push_str("\\\""),
                '\\' => s.push_str("\\\\"),
                c if c < ' ' => s.push_str(&format!("\\u{:04x}", c as u32)),
                c => s.push(c),
            }
        }
        println!("{s}\"}}");
    } else {
        eprintln!("Error: {why}");
    }
    std::process::exit(1)
}
