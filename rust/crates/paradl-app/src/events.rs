//! Oracle-ordered JSONL event rows for the reference binary.
//!
//! `paradl-events` owns the single emitter (`emit_jsonl`) and its Python-style
//! separators. These structs only pin the **field order and explicit nulls** of
//! the Python `UI.emit_event(...)` call sites, which the non-media parity lane
//! compares byte-for-byte on raw stdout. `Option::None` is deliberately NOT
//! skipped here: Python passes `null` kwargs explicitly in these events.

use serde::Serialize;
use serde_json::Value;

/// `target_classified` (E01/E57) with the oracle's six payload fields.
///
/// EXCEPTION (post-review C3): these row structs are deliberately NOT replaced
/// by `paradl_events` schema types. `emit_jsonl` with Python separators writes
/// explicit `null` on raw stdout; schema types use `#[serde(skip_serializing_if)]`
/// and omit the key. Pinned by `paradl-app/tests/dispatch.rs`
/// (`info_json_emits_target_classified_then_info_with_oracle_fields`,
/// `download_json_emits_target_classified_before_result`) and the doctor/parity
/// goldens. Do not change schema serde attributes or golden nulls.
///
/// The three kept structs are named explicitly: `TargetClassifiedRow`,
/// `ProgressRow` and `ResultRow`. Each carries `Option` fields that must
/// reach raw stdout as explicit `null` (e.g. `filename_hint`,
/// `total_bytes`/`eta_seconds`/`peer_count`, and `file_path`/`checksum_*`/
/// `error` respectively), whereas `paradl_events::TargetClassifiedEvent`,
/// `paradl_events::ProgressEvent` and `paradl_events::ResultEvent` drop
/// those keys via `skip_serializing_if`. The divergence is locked by
/// `tests::explicit_null_rows_differ_from_schema_variants`.
///
/// Audit outcome (C3): `InfoRow`/`InfoProperties` are also kept — the schema
/// `paradl_events::InfoEvent` carries `properties: Value` (and
/// `target: String`) rather than a typed struct with a pinned insertion order,
/// so it is not a byte-identical drop-in (see
/// `tests::info_row_key_order_and_optional_hint_are_pinned`). Zero schema-type
/// replacements were made.
#[derive(Debug, Serialize)]
pub(crate) struct TargetClassifiedRow<'a> {
    event: &'static str,
    timestamp: f64,
    target: &'a str,
    target_type: &'a str,
    clean_url: &'a str,
    filename_hint: Option<&'a str>,
    content_length: Option<i64>,
    accept_ranges: Option<bool>,
}

impl<'a> TargetClassifiedRow<'a> {
    /// Build a `target_classified` row.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        timestamp: f64,
        target: &'a str,
        target_type: &'a str,
        clean_url: &'a str,
        filename_hint: Option<&'a str>,
        content_length: Option<i64>,
        accept_ranges: Option<bool>,
    ) -> Self {
        Self {
            event: "target_classified",
            timestamp,
            target,
            target_type,
            clean_url,
            filename_hint,
            content_length,
            accept_ranges,
        }
    }
}

/// The property dict of an `info` event (insertion order matches the oracle).
///
/// C3 audit: kept rather than replaced by `paradl_events::InfoEvent`, whose
/// `properties` field is an untyped `serde_json::Value` (and whose `target` is
/// `String`), so it cannot pin this typed insertion order as a drop-in.
#[derive(Debug, Serialize)]
pub(crate) struct InfoProperties<'a> {
    /// Upper-cased target type.
    #[serde(rename = "Classification")]
    classification: &'a str,
    /// Resolved URL.
    #[serde(rename = "Resolved URL")]
    resolved_url: &'a str,
    /// Suggested filename, when known.
    #[serde(rename = "Filename Hint", skip_serializing_if = "Option::is_none")]
    filename_hint: Option<&'a str>,
}

/// `info` (E37) with the oracle's human-labelled property keys.
#[derive(Debug, Serialize)]
pub(crate) struct InfoRow<'a> {
    event: &'static str,
    timestamp: f64,
    target: &'a str,
    properties: InfoProperties<'a>,
}

impl<'a> InfoRow<'a> {
    /// Build an `info` row.
    pub(crate) fn new(
        timestamp: f64,
        target: &'a str,
        classification: &'a str,
        resolved_url: &'a str,
        filename_hint: Option<&'a str>,
    ) -> Self {
        Self {
            event: "info",
            timestamp,
            target,
            properties: InfoProperties {
                classification,
                resolved_url,
                filename_hint,
            },
        }
    }
}

/// `progress` (E02/E46) with all eleven oracle fields.
///
/// Kept explicit per the C3 exception on `TargetClassifiedRow`: `total_bytes`,
/// `eta_seconds` and `peer_count` must serialize as explicit `null` on raw
/// stdout, which `paradl_events::ProgressEvent` would omit via
/// `skip_serializing_if`.
#[derive(Debug, Serialize)]
pub(crate) struct ProgressRow<'a> {
    event: &'static str,
    timestamp: f64,
    title: &'a str,
    downloaded_bytes: i64,
    total_bytes: Option<i64>,
    percent: f64,
    speed_bps: f64,
    eta_seconds: Option<f64>,
    connections: i32,
    peer_count: Option<i32>,
    watchdog_state: &'a str,
    file_progress: Value,
    connection_speeds: Value,
}

impl<'a> ProgressRow<'a> {
    /// Build a `progress` row from real engine telemetry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        timestamp: f64,
        title: &'a str,
        downloaded_bytes: i64,
        total_bytes: Option<i64>,
        percent: f64,
        speed_bps: f64,
        eta_seconds: Option<f64>,
        connections: i32,
        watchdog_state: &'a str,
    ) -> Self {
        Self {
            event: "progress",
            timestamp,
            title,
            downloaded_bytes,
            total_bytes,
            percent,
            speed_bps,
            eta_seconds,
            connections,
            peer_count: None,
            watchdog_state,
            file_progress: Value::Array(Vec::new()),
            connection_speeds: Value::Array(Vec::new()),
        }
    }
}

/// Terminal `result` (E12/E56) with the oracle's explicit-null kwargs.
///
/// Kept explicit per the C3 exception on `TargetClassifiedRow`: `file_path`,
/// `audio_format_produced`, `checksum_verified`/`checksum_algo`/
/// `checksum_digest` and `error` must serialize as explicit `null` on raw
/// stdout, which `paradl_events::ResultEvent` would omit via
/// `skip_serializing_if`.
#[derive(Debug, Serialize)]
pub(crate) struct ResultRow<'a> {
    event: &'static str,
    timestamp: f64,
    target: &'a str,
    target_type: &'a str,
    success: bool,
    file_path: Option<&'a str>,
    bytes_downloaded: i64,
    duration: f64,
    avg_speed_bps: f64,
    recycle_count: i32,
    audio_format_produced: Option<&'a str>,
    checksum_verified: Option<bool>,
    checksum_algo: Option<&'a str>,
    checksum_digest: Option<&'a str>,
    adb_synced: bool,
    error: Option<&'a str>,
    exit_code: i32,
}

impl<'a> ResultRow<'a> {
    /// Build a `result` row for a completed transfer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        timestamp: f64,
        target: &'a str,
        target_type: &'a str,
        success: bool,
        file_path: Option<&'a str>,
        bytes_downloaded: i64,
        duration: f64,
        avg_speed_bps: f64,
        recycle_count: i32,
        adb_synced: bool,
        error: Option<&'a str>,
        exit_code: i32,
    ) -> Self {
        Self {
            event: "result",
            timestamp,
            target,
            target_type,
            success,
            file_path,
            bytes_downloaded,
            duration,
            avg_speed_bps,
            recycle_count,
            audio_format_produced: None,
            checksum_verified: None,
            checksum_algo: None,
            checksum_digest: None,
            adb_synced,
            error,
            exit_code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paradl_events::{ProgressEvent, ResultEvent, TargetClassifiedEvent, emit_jsonl};

    /// The explicit-null rows and the schema variants are NOT byte-interchangeable.
    /// This characterization test fails if a row ever starts dropping its nulls
    /// or a schema variant ever starts emitting them.
    #[test]
    fn explicit_null_rows_differ_from_schema_variants() {
        // TargetClassifiedRow: filename_hint (and content_length/accept_ranges)
        // are explicit nulls; the schema variant omits those keys.
        let row = emit_jsonl(&TargetClassifiedRow::new(1.0, "t", "direct", "u", None, None, None));
        assert!(row.contains("\"filename_hint\": null"), "{row}");
        assert!(row.contains("\"content_length\": null"), "{row}");
        assert!(row.contains("\"accept_ranges\": null"), "{row}");
        let schema = emit_jsonl(&TargetClassifiedEvent::new(
            1.0, "t".into(), "direct".into(), "u".into(), None, None, None,
        ));
        assert!(!schema.contains("filename_hint"), "{schema}");
        assert!(!schema.contains("content_length"), "{schema}");
        assert!(!schema.contains("accept_ranges"), "{schema}");

        // ProgressRow: total_bytes, eta_seconds and peer_count are explicit
        // nulls; the schema variant omits those keys.
        let row = emit_jsonl(&ProgressRow::new(1.0, "t", 0, None, 0.0, 0.0, None, 1, "ok"));
        assert!(row.contains("\"total_bytes\": null"), "{row}");
        assert!(row.contains("\"eta_seconds\": null"), "{row}");
        assert!(row.contains("\"peer_count\": null"), "{row}");
        let schema = emit_jsonl(&ProgressEvent::new(
            1.0, "t".into(), 0, None, None, None, None, None, None, None, None, None,
        ));
        assert!(!schema.contains("total_bytes"), "{schema}");
        assert!(!schema.contains("eta_seconds"), "{schema}");
        assert!(!schema.contains("peer_count"), "{schema}");

        // ResultRow: file_path, audio_format_produced, the checksum_* fields
        // and error are explicit nulls; the schema variant omits those keys.
        let row = emit_jsonl(&ResultRow::new(1.0, "t", "direct", true, None, 0, 0.0, 0.0, 0, false, None, 0));
        for field in [
            "file_path",
            "audio_format_produced",
            "checksum_verified",
            "checksum_algo",
            "checksum_digest",
            "error",
        ] {
            assert!(row.contains(&format!("\"{field}\": null")), "{field} missing from {row}");
        }
        let schema = emit_jsonl(&ResultEvent::new(
            1.0,
            None,
            Some("t".into()),
            true,
            None,
            None,
            None,
            0,
            Some("direct".into()),
            Some(0),
            Some(0.0),
            Some(0.0),
            Some(0),
            None,
            None,
            None,
            Some(false),
        ));
        for field in [
            "file_path",
            "audio_format_produced",
            "checksum_verified",
            "checksum_algo",
            "checksum_digest",
            "error",
        ] {
            assert!(!schema.contains(field), "{field} leaked into {schema}");
        }
    }

    /// C3 audit: `InfoRow` stays a typed row. Its oracle property key order and
    /// the `Filename Hint` skip must remain pinned here; `InfoEvent` takes an
    /// untyped `properties: Value`, so it is not a byte-exact drop-in.
    #[test]
    fn info_row_key_order_and_optional_hint_are_pinned() {
        let row = emit_jsonl(&InfoRow::new(1.0, "t", "direct", "u", Some("h")));
        let classification = row.find("\"Classification\"").expect("Classification key");
        let resolved = row.find("\"Resolved URL\"").expect("Resolved URL key");
        let hint = row.find("\"Filename Hint\"").expect("Filename Hint key");
        assert!(classification < resolved && resolved < hint, "{row}");

        let row_none = emit_jsonl(&InfoRow::new(1.0, "t", "direct", "u", None));
        assert!(!row_none.contains("Filename Hint"), "{row_none}");
    }
}
