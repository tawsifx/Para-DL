//! Serde request/response models and FastAPI-compatible validation.
//!
//! The field sets mirror `src/paradl/api/server.py:85-195` exactly. Validation
//! reproduces pydantic v2's `Field(ge=1, le=16)` behaviour, including the
//! `{type, loc, msg, input, ctx}` issue shape returned inside a 422 `detail`
//! array.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// One entry in a FastAPI/pydantic v2 422 `detail` array.
///
/// Serialized with the exact key order `type`, `loc`, `msg`, `input`, `ctx`;
/// `ctx` is omitted when the issue has no bound context (e.g. `missing`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationIssue {
    /// Pydantic error type, serialized as the JSON key `type`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Python-style location path, e.g. `["body", "connections"]`.
    pub loc: Vec<String>,
    /// Human-readable pydantic message.
    pub msg: String,
    /// The rejected raw JSON value.
    pub input: Value,
    /// Bound context such as `{"ge": 1}`; absent for `missing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctx: Option<Value>,
}

impl ValidationIssue {
    /// A required field was not supplied.
    pub fn missing(loc: Vec<String>, input: Value) -> Self {
        Self {
            kind: "missing".into(),
            loc,
            msg: "Field required".into(),
            input,
            ctx: None,
        }
    }

    /// An integer violated `Field(ge = n)`.
    pub fn greater_than_equal(loc: Vec<String>, input: Value, ge: i64) -> Self {
        Self {
            kind: "greater_than_equal".into(),
            loc,
            msg: format!("Input should be greater than or equal to {ge}"),
            input,
            ctx: Some(json!({ "ge": ge })),
        }
    }

    /// An integer violated `Field(le = n)`.
    pub fn less_than_equal(loc: Vec<String>, input: Value, le: i64) -> Self {
        Self {
            kind: "less_than_equal".into(),
            loc,
            msg: format!("Input should be less than or equal to {le}"),
            input,
            ctx: Some(json!({ "le": le })),
        }
    }

    /// A value had the wrong fundamental type (`string`, `integer`, ...).
    pub fn wrong_type(loc: Vec<String>, input: Value, expected: &str) -> Self {
        Self {
            kind: format!("{expected}_type"),
            loc,
            msg: format!("Input should be a valid {expected}"),
            input,
            ctx: None,
        }
    }

    /// A generic issue (used for JSON decode failures and serde fallbacks).
    pub fn value_error(loc: Vec<String>, input: Value, msg: impl Into<String>) -> Self {
        Self {
            kind: "value_error".into(),
            loc,
            msg: msg.into(),
            input,
            ctx: None,
        }
    }
}

/// A request type that validates a raw JSON value the way pydantic does.
pub trait ValidatedRequest: Sized + DeserializeOwned {
    /// Validate *value*, returning all issues instead of failing on the first.
    fn validate(value: &Value) -> Result<Self, Vec<ValidationIssue>>;
}

fn body_location(field: &str) -> Vec<String> {
    vec!["body".into(), field.into()]
}

fn required_string(obj: &Map<String, Value>, field: &str, whole: &Value, issues: &mut Vec<ValidationIssue>) -> String {
    match obj.get(field) {
        Some(Value::String(s)) => s.clone(),
        Some(other) => {
            issues.push(ValidationIssue::wrong_type(body_location(field), other.clone(), "string"));
            String::new()
        }
        None => {
            issues.push(ValidationIssue::missing(body_location(field), whole.clone()));
            String::new()
        }
    }
}

/// `POST /resolve` request (`server.py:85-91`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolveRequest {
    /// URL, magnet, shortcut, or search query to classify.
    pub target: String,
}

impl ValidatedRequest for ResolveRequest {
    fn validate(value: &Value) -> Result<Self, Vec<ValidationIssue>> {
        let Some(obj) = value.as_object() else {
            return Err(vec![ValidationIssue::missing(body_location("target"), value.clone())]);
        };
        let mut issues = Vec::new();
        let target = required_string(obj, "target", value, &mut issues);
        if issues.is_empty() {
            Ok(Self { target })
        } else {
            Err(issues)
        }
    }
}

/// `POST /resolve` response (`server.py:93-105`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolveResponse {
    /// The original target string.
    pub target: String,
    /// Classified target type string (e.g. `direct`, `github`).
    pub target_type: String,
    /// Normalized URL after shortcut expansion.
    pub clean_url: Option<String>,
    /// Filename suggested by the target.
    pub filename_hint: Option<String>,
    /// `Content-Length` reported by the probe.
    pub content_length: Option<i64>,
    /// MIME type reported by the probe.
    pub mime_type: Option<String>,
    /// Whether the server supports byte ranges.
    pub accept_ranges: Option<bool>,
    /// SHA-256 of the ETag, when available.
    pub etag_sha256: Option<String>,
    /// Expected checksum from the resolver.
    pub expected_digest: Option<String>,
    /// Probe HTTP status code.
    pub status_code: Option<i32>,
    /// Non-fatal probe error text.
    pub probe_error: Option<String>,
}

/// `POST /tasks` request (`server.py:107-150`).
///
/// Unknown fields are ignored, matching `ConfigDict(extra="ignore")`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateTaskRequest {
    /// URL or shortcut to download.
    pub target: String,
    /// Output directory; falls back to the configured default.
    pub out_dir: Option<String>,
    /// Suggested output filename.
    pub filename: Option<String>,
    /// Parallel connection streams; constrained to `1..=16`.
    pub connections: i32,
    /// Alternate mirror URLs.
    pub mirrors: Vec<String>,
    /// Queue priority (higher runs first).
    pub priority: i32,
    /// Watchdog mode: `v1`, `v2`, or `off`.
    pub watchdog: String,
    /// Auto-recycle threshold such as `4MBps`.
    pub anti_throttle: Option<String>,
    /// Hard speed cap such as `10MB/s`.
    pub limit_rate: Option<String>,
    /// Fail unless the machine is on AC power.
    pub require_ac: bool,
    /// Pause on macOS Low Power / Battery Saver.
    pub pause_on_low_power: bool,
    /// Start only when a speed test clears this threshold.
    pub when_fast_bps: f64,
    /// Scheduled start time `HH:MM`.
    pub start_at: Option<String>,
    /// Expected checksum (`sha256:<hex>`).
    pub expected_digest: Option<String>,
    /// Alias for `expected_digest`.
    pub checksum: Option<String>,
    /// Auto-extract archives on completion.
    pub auto_extract: bool,
    /// Enable HTTP/3 when supported.
    pub http3: bool,
    /// Proxy URL for transfers.
    pub proxy: Option<String>,
    /// Media: extract audio only.
    pub is_audio: bool,
    /// Media audio codec.
    pub audio_codec: String,
    /// Media video quality preset.
    pub video_quality: String,
    /// Media: download the entire playlist.
    pub playlist: bool,
    /// Media: download and embed subtitles.
    pub subs: bool,
    /// Media: embed thumbnail art.
    pub embed_thumbnail: bool,
}

impl Default for CreateTaskRequest {
    fn default() -> Self {
        Self {
            target: String::new(),
            out_dir: None,
            filename: None,
            connections: 16,
            mirrors: Vec::new(),
            priority: 0,
            watchdog: "v2".into(),
            anti_throttle: Some("4MBps".into()),
            limit_rate: None,
            require_ac: false,
            pause_on_low_power: false,
            when_fast_bps: 0.0,
            start_at: None,
            expected_digest: None,
            checksum: None,
            auto_extract: false,
            http3: true,
            proxy: None,
            is_audio: true,
            audio_codec: "aac".into(),
            video_quality: "best".into(),
            playlist: false,
            subs: false,
            embed_thumbnail: true,
        }
    }
}

impl ValidatedRequest for CreateTaskRequest {
    fn validate(value: &Value) -> Result<Self, Vec<ValidationIssue>> {
        let Some(obj) = value.as_object() else {
            return Err(vec![ValidationIssue::missing(body_location("target"), value.clone())]);
        };
        let mut issues = Vec::new();
        let target = required_string(obj, "target", value, &mut issues);

        if let Some(connections) = obj.get("connections") {
            match connections.as_i64() {
                Some(n) if n < 1 => issues.push(ValidationIssue::greater_than_equal(
                    body_location("connections"),
                    connections.clone(),
                    1,
                )),
                Some(n) if n > 16 => issues.push(ValidationIssue::less_than_equal(
                    body_location("connections"),
                    connections.clone(),
                    16,
                )),
                Some(_) => {}
                None => issues.push(ValidationIssue::wrong_type(
                    body_location("connections"),
                    connections.clone(),
                    "integer",
                )),
            }
        }

        if !issues.is_empty() {
            return Err(issues);
        }
        serde_json::from_value::<Self>(value.clone())
            .map(|mut req| {
                req.target = target;
                req
            })
            .map_err(|err| vec![ValidationIssue::value_error(vec!["body".into()], value.clone(), err.to_string())])
    }
}

/// `POST /tasks` response (`server.py:152-156`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreateTaskResponse {
    /// Assigned task id (UUID v4 or the SQLite queue job id as text).
    pub id: String,
    /// Initial status, always `queued`.
    pub status: String,
    /// Human-readable acknowledgement.
    pub message: String,
}

/// A task as exposed by the API (`server.py:158-183`).
///
/// Python's `@property` members (`progress_percent`, `speed_human`) are not
/// pydantic computed fields, so they are deliberately absent from the JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskInfo {
    /// Task identifier.
    pub id: String,
    /// Original download target.
    pub target: String,
    /// Classified target type.
    pub target_type: String,
    /// One of `queued`, `running`, `paused`, `completed`, `failed`.
    pub status: String,
    /// Output directory.
    pub out_dir: String,
    /// Suggested filename, when supplied.
    pub filename: Option<String>,
    /// Configured connection count.
    pub connections: i32,
    /// Queue priority.
    pub priority: i32,
    /// Bytes transferred so far.
    pub bytes_downloaded: i64,
    /// Total bytes when known.
    pub total_bytes: Option<i64>,
    /// Instantaneous speed, rounded to two decimals.
    pub speed_bps: f64,
    /// Final path on disk.
    pub file_path: Option<String>,
    /// Failure text, when the task failed.
    pub error: Option<String>,
    /// Creation timestamp (`%Y-%m-%d %H:%M:%S`).
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
}

/// `GET /tasks` response (`server.py:186-189`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskListResponse {
    /// Matching tasks in `created_at DESC` order.
    pub tasks: Vec<TaskInfo>,
    /// Number of matching tasks.
    pub count: usize,
}

/// Body returned by pause/resume/delete (`server.py:191-195`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskActionResponse {
    /// Affected task id.
    pub id: String,
    /// New status of the task.
    pub status: String,
    /// Human-readable result message.
    pub message: String,
}
