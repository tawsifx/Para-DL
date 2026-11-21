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
