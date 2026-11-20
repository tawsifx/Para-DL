//! Read-only access to the captured golden fixtures.
//!
//! Fixtures live in the crate's `fixtures/` directory and are read at runtime relative to
//! `CARGO_MANIFEST_DIR`. A missing fixture is a hard error, which is what makes the contract
//! tests go RED before the fixtures were captured.

use std::path::PathBuf;

/// The canonical golden fixture files.
pub const FIXTURE_NAMES: [&str; 9] = [
    "core_models.json",
    "config_defaults.json",
    "classifier_seeded.json",
    "watchdog.json",
    "jsonl_events.json",
    "redaction.json",
    "sidecar.json",
    "report_schema.json",
    "media_protocol.json",
];

/// Return the absolute path of a golden fixture by file name.
///
/// # Panics
///
/// Panics if `name` is not one of [`FIXTURE_NAMES`].
pub fn fixture_path(name: &str) -> PathBuf {
    assert!(
        FIXTURE_NAMES.contains(&name),
        "unknown golden fixture: {name}"
    );
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Read a golden fixture as raw JSON text.
///
/// # Panics
///
/// Panics if the fixture file is missing (the intended RED state) or is not valid UTF-8.
pub fn fixture_str(name: &str) -> String {
    let path = fixture_path(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing golden fixture {}: {e}", path.display()))
}

/// Parse a golden fixture into a [`serde_json::Value`].
///
/// # Panics
///
/// Panics if the fixture is missing or contains invalid JSON.
pub fn fixture(name: &str) -> serde_json::Value {
    let text = fixture_str(name);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("invalid JSON in golden fixture {name}: {e}"))
}
