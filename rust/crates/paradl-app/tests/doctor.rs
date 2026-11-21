//! Tests for `doctor`: no Python, oracle-aligned JSON, deterministic human
//! output, and single-emitter dispatch.

use std::sync::{Arc, Mutex};

use paradl_app::{
    App, DoctorEnv, DoctorReport, MEDIA_PACKAGE_VERSION, MEDIA_PROTOCOL, PlatformUtilities,
    ToolProbe,
};
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};
use serde_json::json;

/// A fully fixed report: no real system tools and no wall-clock-derived values.
fn fixed_report() -> DoctorReport {
    DoctorReport {
        binary_version: "1.0.0".into(),
        oracle_version: "1.0.0".into(),
        media_protocol: MEDIA_PROTOCOL.into(),
        media_package_version: MEDIA_PACKAGE_VERSION.into(),
        yt_dlp_version: Some("2026.10.26".into()),
        ytdlp_age_days: Some(26),
        ytdlp_outdated: false,
        js_runtime: Some("node v20.11.1".into()),
        media_enabled: false,
        python_probed: false,
        probe: ToolProbe {
            yt_dlp_version: Some("2026.10.26".into()),
            yt_dlp_path: Some("/opt/homebrew/bin/yt-dlp".into()),
            js_runtime: Some("deno (/opt/homebrew/bin/deno)".into()),
            node_version: Some("v20.11.1".into()),
            deno_version: Some("2.9.8".into()),
            deno_path: Some("/opt/homebrew/bin/deno".into()),
            bun_version: None,
            ffmpeg_version: Some("7.0".into()),
            ffmpeg_path: Some("/opt/homebrew/bin/ffmpeg".into()),
            aria2c_path: Some("/opt/homebrew/bin/aria2c".into()),
            afconvert_path: Some("/usr/bin/afconvert".into()),
            ytdlp_ejs_version: Some("0.8.0".into()),
            npm_version: Some("10.2.4".into()),
            npm_path: Some("/opt/homebrew/bin/npm".into()),
        },
        env: DoctorEnv {
            auth: json!({
                "hf": {"service": "hf", "label": "Hugging Face Hub", "configured": false, "source": "none"},
                "github": {"service": "github", "label": "GitHub API", "configured": false, "source": "none"},
            }),
            platform_utilities: PlatformUtilities::default(),
            rust_engine_version: None,
            npm_version: Some("10.2.4".into()),
            npm_path: Some("/opt/homebrew/bin/npm".into()),
            fixes: vec![],
        },
    }
}

/// Replace the timestamp value so the rest of a JSONL line can be compared
/// byte-for-byte against a golden fixture (lead ruling: exclude timestamps).
fn normalise_timestamp(line: &str) -> String {
    const KEY: &str = "\"timestamp\": ";
    let start = line.find(KEY).expect("timestamp present") + KEY.len();
    let end = start + line[start..].find(',').expect("timestamp followed by a field");
    format!("{}<TS>{}", &line[..start], &line[end..])
}

#[test]
fn doctor_report_has_all_required_fields_without_probing_python() {
    // Even a broken media-Python override must not matter to a non-media path.
    // SAFETY: single-threaded test; no other test reads PARADL_MEDIA_PY.
    unsafe { std::env::set_var("PARADL_MEDIA_PY", "/definitely/not/a/python") };
    let report = DoctorReport::collect(false);
    unsafe { std::env::remove_var("PARADL_MEDIA_PY") };

    assert!(!report.python_probed, "non-media doctor must not probe Python");
    assert_eq!(report.binary_version, "1.0.0");
    assert_eq!(report.oracle_version, "1.0.0");
    assert_eq!(report.media_protocol, MEDIA_PROTOCOL);
    assert_eq!(MEDIA_PROTOCOL, "paradl-media/1");
    assert_eq!(report.media_package_version, MEDIA_PACKAGE_VERSION);
    assert_eq!(MEDIA_PACKAGE_VERSION, "0.1.0");

    let text = report.render_human();
    assert!(text.contains("media protocol"), "{text}");
    assert!(text.contains("paradl-media/1"), "{text}");
    assert!(text.contains("media package"), "{text}");
    assert!(text.contains("yt-dlp"), "{text}");
    assert!(text.contains("js runtime"), "{text}");
    assert!(!text.contains("\x1b["), "human output must never contain ANSI");
}

#[test]
fn human_doctor_output_is_byte_exact_and_deterministic() {
    let expected = include_str!("fixtures/doctor_human_golden.txt");
    let actual = fixed_report().render_human();
    assert_eq!(actual, expected, "human doctor output drifted from golden");
    assert!(!actual.contains('\r'), "no CRLF");
    assert!(actual.ends_with('\n'), "trailing newline required");
    assert!(!actual.contains('\u{1b}'), "no ANSI escape bytes");

    // Same input twice, same bytes, independent of any terminal state.
    assert_eq!(fixed_report().render_human(), actual);
}

#[test]
fn doctor_json_matches_oracle_field_set_byte_exact() {
    let golden: Vec<&str> = include_str!("fixtures/doctor_json_golden.txt").lines().collect();
    let events = fixed_report().json_events();
    assert_eq!(events.len(), 2, "doctor and its backward-compat alias");
    assert_eq!(golden.len(), events.len());
    for (index, (actual, expected)) in events.iter().zip(golden).enumerate() {
        assert_eq!(normalise_timestamp(actual).trim_end(), expected, "event {index}");
        assert!(actual.ends_with('\n'));
    }
    assert!(events[0].contains("\"event\": \"doctor\""));
    assert!(events[1].contains("\"event\": \"diagnose\""));
    // Every oracle field is present (null values are emitted, never omitted).
    for field in [
        "version", "ytdlp", "ytdlp_ejs", "rust_engine", "npm", "auth",
        "platform_utilities", "macos_utilities", "core_tools", "fixes",
    ] {
        assert!(events[0].contains(&format!("\"{field}\":")), "missing {field}");
    }
}

#[test]
fn doctor_json_mode_emits_doctor_and_diagnose_through_single_emitter() {
    let report = DoctorReport::collect(false);
    let events = report.json_events();
    assert_eq!(events.len(), 2);
    for line in &events {
        assert!(line.contains("\"timestamp\":"), "{line}");
        assert!(line.contains("\"version\": \"1.0.0\""), "{line}");
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

/// Dump helper used once while authoring the golden fixtures. Keep it behind
/// an env flag so it never affects normal test runs.
#[test]
fn dump_golden_fixtures_when_requested() {
    if std::env::var_os("W41_DUMP_GOLDEN").is_none() {
        return;
    }
    let report = fixed_report();
    println!("---HUMAN---\n{}", report.render_human());
    for line in report.json_events() {
        println!("---JSON---{}", normalise_timestamp(&line).trim_end());
    }
}
