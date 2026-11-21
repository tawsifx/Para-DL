//! Integration coverage for Platform-backed auth at the dispatch level.
//!
//! `PlatformSecretStore` is `pub(crate)` on purpose, so precedence over the
//! keychain is asserted in `platform_store.rs` unit tests instead. This file
//! verifies the observable `paradl auth status --json` contract: a single
//! JSONL line whose `source` metadata reflects the environment, and no raw
//! token ever leaks into captured output.

use std::sync::{Arc, Mutex};

use paradl_app::App;
use paradl_cli::{Cli, Command, Dispatch, ParseOutcome};

const ENV_TOKEN: &str = "c1_env_token_9f3a2b";

fn auth_status_json() -> String {
    let outcome = Cli::parse_command_line(vec!["auth".into(), "status".into(), "--json".into()])
        .expect("parse succeeds");
    let ParseOutcome::Command(cli) = outcome else {
        panic!("expected a command, got {outcome:?}");
    };
    let command = Command::from_cli(&cli).expect("classification succeeds");

    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = {
        let buffer = Arc::clone(&buffer);
        Arc::new(move |line: &str| buffer.lock().unwrap().push_str(line))
    };
    let mut app = App::new_offline().with_output(sink);
    let code = app.handle(&command, &cli);
    assert_eq!(code.as_i32(), 0, "auth status must succeed");
    buffer.lock().unwrap().clone()
}

#[test]
fn auth_status_reports_env_source_and_never_leaks_the_token() {
    let previous = std::env::var_os("HF_TOKEN");
    unsafe { std::env::set_var("HF_TOKEN", ENV_TOKEN) };
    let output = auth_status_json();
    match previous {
        Some(value) => unsafe { std::env::set_var("HF_TOKEN", value) },
        None => unsafe { std::env::remove_var("HF_TOKEN") },
    }

    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 1, "expected one JSONL line, got: {output}");
    assert!(lines[0].contains("\"event\": \"auth_status\""), "{output}");
    assert!(lines[0].contains("\"hf\""), "{output}");
    assert!(lines[0].contains("\"github\""), "{output}");
    assert!(
        lines[0].contains("\"source\": \"env\""),
        "HF_TOKEN must win with source=env: {output}"
    );
    assert!(
        !output.contains(ENV_TOKEN),
        "raw token must never appear in emitted output: {output}"
    );
}
