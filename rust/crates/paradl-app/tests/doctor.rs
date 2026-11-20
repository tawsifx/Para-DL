//! RED-first tests: `doctor` must work with NO Python on non-media paths.

use std::sync::{Arc, Mutex};

use paradl_app::{App, DoctorReport, MEDIA_PACKAGE_VERSION, MEDIA_PROTOCOL};
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};

#[test]
fn doctor_report_has_all_required_fields_without_probing_python() {
    // Even a broken media-Python override must not matter to a non-media path.
    // SAFETY: single-threaded test; no other test reads PARADL_MEDIA_PY.
    unsafe { std::env::set_var("PARADL_MEDIA_PY", "/definitely/not/a/python") };
    let report = DoctorReport::collect(false);
    unsafe { std::env::remove_var("PARADL_MEDIA_PY") };

    assert!(!report.python_probed, "non-media doctor must not probe Python");
    assert_eq!(report.binary_version, "0.1.0");
    assert_eq!(report.media_protocol, MEDIA_PROTOCOL);
    assert_eq!(MEDIA_PROTOCOL, "paradl-media/1");
    assert_eq!(report.media_package_version, MEDIA_PACKAGE_VERSION);
    assert_eq!(MEDIA_PACKAGE_VERSION, "0.1.0");

    let line = report.render_human();
    assert!(line.contains("version=0.1.0"), "{line}");
    assert!(line.contains("protocol=paradl-media/1"), "{line}");
    assert!(line.contains("media_package=0.1.0"), "{line}");
    assert!(line.contains("yt_dlp="), "{line}");
    assert!(line.contains("js_runtime="), "{line}");
    assert!(!line.contains("python="), "{line}");
}

#[test]
fn doctor_json_mode_emits_doctor_and_diagnose_through_single_emitter() {
    let report = DoctorReport::collect(false);
    let events = report.json_events();
    assert_eq!(events.len(), 2, "doctor and its backward-compat alias");
    assert!(events[0].contains("\"event\": \"doctor\""), "{}", events[0]);
    assert!(events[1].contains("\"event\": \"diagnose\""), "{}", events[1]);
    for line in &events {
        assert!(line.contains("\"timestamp\":"), "{line}");
        assert!(line.contains("\"version\": \"0.1.0\""), "{line}");
        assert!(line.ends_with('\n'), "{line}");
    }
}

#[test]
fn doctor_subcommand_dispatches_without_python() {
    let cli = Cli::parse_command_line(vec!["doctor".into(), "--json".into()]).unwrap();
    let ParseOutcome::Command(cli) = cli else { panic!() };
    let command = Command::from_cli(&cli).unwrap();

    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = {
        let buffer = Arc::clone(&buffer);
        Arc::new(move |line: &str| buffer.lock().unwrap().push_str(line))
    };
    let mut app = App::new_offline().with_output(sink);
    assert_eq!(app.handle(&command, &cli), ExitCode::Ok);
    let output = buffer.lock().unwrap().clone();
    assert!(output.contains("\"event\": \"doctor\""), "{output}");
    assert!(output.contains("\"event\": \"diagnose\""), "{output}");
}
