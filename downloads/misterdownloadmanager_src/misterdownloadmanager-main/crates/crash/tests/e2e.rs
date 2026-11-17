//! A real freeze, panic and crash, end to end on each system: examples/crashy.rs does it, its
//! monitor writes the minidump, reads it with the example's own debug info, and the report names
//! the function where it happened.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

fn crashy() -> PathBuf {
    // target/<profile>/deps/e2e-… → target/<profile>/examples/crashy
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    p.pop();
    p.push("examples");
    p.push(if cfg!(windows) {
        "crashy.exe"
    } else {
        "crashy"
    });
    p
}

/// Run crashy in `mode` and wait (up to 90 s: reading the dump's symbols takes a while in a
/// debug build) for its report.
fn report(mode: &str) -> String {
    let data = tempfile::tempdir().unwrap();
    /// The program, ended and reaped however the test leaves (a failed assert too).
    struct Running(std::process::Child);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _child = Running(
        Command::new(crashy())
            .arg(mode)
            .arg(data.path())
            .spawn()
            .expect("examples/crashy is built with the tests"),
    );
    let crashes = data.path().join("crashes");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let txt = std::fs::read_dir(&crashes).ok().and_then(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .find(|p| p.extension().is_some_and(|x| x == "txt"))
        });
        if let Some(t) = txt {
            // Written in one go after the dump was read; give the write a moment.
            std::thread::sleep(Duration::from_millis(300));
            return std::fs::read_to_string(&t).unwrap();
        }
        if Instant::now() > deadline {
            panic!("no report for {mode} in {}", crashes.display());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_freeze_reports_the_frozen_function() {
    let r = report("freeze");
    assert!(r.contains("The window stopped responding"), "{r}");
    assert!(r.contains("crashy_frozen_here"), "{r}");
    assert!(
        r.contains("Signature: freeze in crashy::crashy_frozen_here"),
        "{r}"
    );
}

#[test]
fn a_panic_reports_its_message_and_function() {
    let r = report("panic");
    assert!(r.contains("crashy: a test panic"), "{r}");
    assert!(
        r.contains("Signature: panic in crashy::crashy_panics_here"),
        "{r}"
    );
}

#[test]
fn a_native_crash_reports_where_it_crashed() {
    let r = report("crash");
    assert!(r.contains("Description: MDM crashed"), "{r}");
    assert!(
        r.contains("Signature: crash in crashy::crashy_crashes_here"),
        "{r}"
    );
}
