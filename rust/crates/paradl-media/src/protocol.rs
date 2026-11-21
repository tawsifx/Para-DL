//! Wire model for `paradl-media/1`: request, frames, and the handshake.
//!
//! Nothing here spawns a process or writes stdout. Every public parser returns
//! a [`paradl_core::Error`], so protocol faults already carry the final
//! [`ExitCode`] the reference binary must use.

use std::collections::BTreeMap;
use std::path::PathBuf;

use paradl_core::{Error, ExitCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::router::MediaRoute;

/// Protocol identifier sent on the request and on every response frame.
pub const PROTOCOL_ID: &str = "paradl-media/1";

/// Accepted major version. Minor additions are tolerated (charter §4).
pub const PROTOCOL_MAJOR: u32 = 1;

/// Prefix every `paradl-media/<major>[.<minor>]` version starts with.
pub const PROTOCOL_PREFIX: &str = "paradl-media/";

/// Check a response (or request) protocol version.
///
/// `None` fails closed with exit code `4` ("protocol version missing"),
/// unparseable values fail closed with `4`, and any major other than
/// [`PROTOCOL_MAJOR`] fails closed with `4` and a clear diagnostic. Minor
/// additions (`paradl-media/1.1`, `paradl-media/1.999`) are accepted.
///
/// ```
/// use paradl_media::{check_protocol, PROTOCOL_ID};
/// assert_eq!(check_protocol(Some(PROTOCOL_ID)).unwrap(), 1);
/// assert!(check_protocol(Some("paradl-media/1.1")).is_ok());
/// assert!(check_protocol(Some("paradl-media/2")).is_err());
/// assert!(check_protocol(None).is_err());
/// ```
pub fn check_protocol(version: Option<&str>) -> Result<u32, Error> {
    let raw = match version {
        Some(value) => value,
        None => return Err(Error::missing_dependency("protocol version missing")),
    };
    let rest = raw.strip_prefix(PROTOCOL_PREFIX).ok_or_else(|| {
        Error::missing_dependency(format!("unparseable protocol version: {raw:?}"))
    })?;
    let major_text = rest.split('.').next().unwrap_or_default();
    let major: u32 = major_text.parse().map_err(|_| {
        Error::missing_dependency(format!("unparseable protocol version: {raw:?}"))
    })?;
    if major != PROTOCOL_MAJOR {
        return Err(Error::missing_dependency(format!(
            "unsupported protocol major version: {major} (expected {PROTOCOL_MAJOR})"
        )));
    }
    Ok(major)
}

/// Map a worker `error.kind` to the Rust-owned [`ExitCode`].
///
/// `metadata|download|conversion` → `1`, `dependency` → `4`, `usage` → `2`,
/// `interrupted` → `130`. The worker never emits `3`; unknown kinds fail
/// closed to `1`.
pub fn error_kind_to_exit_code(kind: &str) -> ExitCode {
    ExitCode::from_media_kind(kind)
}

/// Build the full [`Error`] for a worker `error.kind` and human message.
///
/// The mapping is the same as [`error_kind_to_exit_code`] but preserves the
/// worker's message for diagnostics.
pub fn error_kind_to_error(kind: &str, message: impl Into<String>) -> Error {
    Error::from_media_kind(kind, message)
}

/// Structured error kind emitted by the Python worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Metadata extraction failed.
    Metadata,
    /// Media download failed.
    Download,
    /// Audio conversion failed.
    Conversion,
    /// A required external dependency (yt-dlp, JS runtime) is missing.
    Dependency,
    /// The request was invalid.
    Usage,
    /// The operation was interrupted by SIGINT/SIGTERM.
    Interrupted,
    /// A kind this client does not know yet; fails closed to exit code `1`.
    Unknown(String),
}

impl ErrorKind {
    /// Parse a worker `error.kind` string into a typed kind.
    pub fn parse(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "metadata" => Self::Metadata,
            "download" => Self::Download,
            "conversion" => Self::Conversion,
            "dependency" => Self::Dependency,
            "usage" => Self::Usage,
            "interrupted" => Self::Interrupted,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Return the canonical wire string of this kind.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Metadata => "metadata",
            Self::Download => "download",
            Self::Conversion => "conversion",
            Self::Dependency => "dependency",
            Self::Usage => "usage",
            Self::Interrupted => "interrupted",
            Self::Unknown(other) => other,
        }
    }

    /// Return the exit code this kind maps to.
    pub fn exit_code(&self) -> ExitCode {
        error_kind_to_exit_code(self.as_str())
    }
}

/// Media options forwarded verbatim to the worker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaOptions {
    /// Request audio-only extraction.
    #[serde(default)]
    pub audio_only: bool,
    /// Requested format selector, or `None` for the worker default.
    #[serde(default)]
    pub format: Option<String>,
    /// Browser name to import cookies from, if any.
    #[serde(default)]
    pub cookies_from_browser: Option<String>,
}

/// A caller's request to the media worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaRequest {
    /// URL or `ytsearch1:` free-text search target.
    pub target: String,
    /// Absolute output directory for worker-produced files.
    pub out_dir: PathBuf,
    /// Extraction options.
    #[serde(default)]
    pub options: MediaOptions,
}

impl MediaRequest {
    /// Construct a request with default options.
    pub fn new(target: impl Into<String>, out_dir: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            out_dir: out_dir.into(),
            options: MediaOptions::default(),
        }
    }
}

/// The four operations of the `paradl-media/1` protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolOp {
    /// Extract metadata without downloading media bytes.
    Resolve,
    /// Download and tag media managed by the worker.
    DownloadManaged,
    /// Finalize tags/parts after Rust-owned checksum verification.
    Finalize,
    /// Ask the live yt-dlp extractor registry whether it claims a URL.
    Claims,
}

impl ProtocolOp {
    /// Return the wire string of this operation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::DownloadManaged => "download_managed",
            Self::Finalize => "finalize",
            Self::Claims => "claims",
        }
    }
}

/// The single JSON line written to worker stdin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolRequest {
    /// Protocol identifier; always [`PROTOCOL_ID`].
    pub protocol: String,
    /// UUID v4 used to correlate every response frame.
    pub request_id: String,
    /// Requested operation.
    pub op: ProtocolOp,
    /// URL or search target.
    pub target: String,
    /// Absolute output directory as a string.
    pub out_dir: String,
    /// Extraction options.
    pub options: MediaOptions,
}

impl ProtocolRequest {
    /// Build a protocol request from a caller request and a correlation id.
    pub fn new(
        op: ProtocolOp,
        request: &MediaRequest,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            protocol: PROTOCOL_ID.to_owned(),
            request_id: request_id.into(),
            op,
            target: request.target.clone(),
            out_dir: request.out_dir.to_string_lossy().into_owned(),
            options: request.options.clone(),
        }
    }
}

/// Worker and tool versions reported in every terminal frame.
///
/// These are data (not protocol): a new yt-dlp version never changes
/// [`PROTOCOL_ID`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerVersions {
    /// Version of the `paradl_media` package.
    pub worker_version: String,
    /// Installed yt-dlp version.
    pub yt_dlp_version: String,
    /// Installed mutagen version.
    pub mutagen_version: String,
    /// Installed ffmpeg version (empty when the weak dependency is absent).
    pub ffmpeg_version: String,
}

impl WorkerVersions {
    /// Parse and require the four version keys in a terminal frame.
    ///
    /// Missing keys or non-string (non-null) values fail closed with exit code
    /// `4`. `null` is accepted for optional tools and becomes an empty string.
    pub fn from_value(value: Option<&Value>) -> Result<Self, Error> {
        let object = value
            .and_then(Value::as_object)
            .ok_or_else(|| Error::missing_dependency("versions missing from terminal frame"))?;
        let field = |key: &str| -> Result<String, Error> {
            match object.get(key) {
                Some(Value::String(value)) => Ok(value.clone()),
                Some(Value::Null) => Ok(String::new()),
                _ => Err(Error::missing_dependency(format!(
                    "unparseable versions: {key} must be a string"
                ))),
            }
        };
        Ok(Self {
            worker_version: field("worker_version")?,
            yt_dlp_version: field("yt_dlp_version")?,
            mutagen_version: field("mutagen_version")?,
            ffmpeg_version: field("ffmpeg_version")?,
        })
    }
}

/// One decoded `progress` frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProgressFrame {
    /// Bytes downloaded so far.
    pub downloaded_bytes: u64,
    /// Total bytes when known; `None` for close-delimited streams.
    pub total_bytes: Option<u64>,
    /// Current speed in bytes per second.
    pub speed_bps: u64,
    /// Estimated seconds remaining, when known.
    pub eta_seconds: Option<f64>,
}

/// The payload of a successful terminal frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaData {
    /// Sanitized output filename (200-char cap, Windows reserved names).
    #[serde(default)]
    pub filename: String,
    /// HTTP headers with unchanged keys and string-coerced values.
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    /// Exact `classify_audio_production` description, when applicable.
    #[serde(default)]
    pub audio_format_produced: Option<String>,
    /// Files produced by the worker.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
}

impl MediaData {
    /// Decode a terminal `data` object, coercing header values to strings.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let object = value
            .as_object()
            .ok_or_else(|| Error::failure("terminal data must be a JSON object"))?;
        let filename = match object.get("filename") {
            Some(Value::String(value)) => value.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let mut http_headers = BTreeMap::new();
        if let Some(Value::Object(headers)) = object.get("http_headers") {
            for (key, value) in headers {
                http_headers.insert(key.clone(), coerce_string(value));
            }
        }
        let audio_format_produced = match object.get("audio_format_produced") {
            Some(Value::String(value)) => Some(value.clone()),
            _ => None,
        };
        let paths = match object.get("paths") {
            Some(Value::Array(values)) => values
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            filename,
            http_headers,
            audio_format_produced,
            paths,
        })
    }
}

fn coerce_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        other => other.to_string(),
    }
}

/// A fully decoded worker stdout frame.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerFrame {
    /// A progress tick, to be re-emitted as a `paradl-events` event.
    Progress(ProgressFrame),
    /// The single terminal success frame.
    Result {
        /// Worker/tool versions from the frame.
        versions: WorkerVersions,
        /// Advertised worker capabilities.
        caps: Vec<String>,
        /// Raw operation payload.
        data: Value,
    },
    /// The single terminal error frame.
    Error {
        /// Structured worker error kind.
        kind: ErrorKind,
        /// Human-readable diagnostic from the worker.
        message: String,
        /// Worker/tool versions from the frame.
        versions: WorkerVersions,
    },
}

impl WorkerFrame {
    /// Parse one stdout line, enforcing handshake and request correlation.
    pub fn parse(line: &str, expected_request_id: &str) -> Result<Self, Error> {
        let value: Value = serde_json::from_str(line)
            .map_err(|error| Error::failure(format!("malformed worker frame: {error}")))?;
        let object: &Map<String, Value> = value
            .as_object()
            .ok_or_else(|| Error::failure("worker frame must be a JSON object"))?;

        check_protocol(object.get("protocol").and_then(Value::as_str))?;
        let request_id = object
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::failure("worker frame missing request_id"))?;
        if request_id != expected_request_id {
            return Err(Error::failure(format!(
                "request_id mismatch: expected {expected_request_id}, got {request_id}"
            )));
        }

        match object.get("frame").and_then(Value::as_str) {
            Some("progress") => Ok(Self::Progress(parse_progress(object)?)),
            Some("result") => {
                if object.get("ok") != Some(&Value::Bool(true)) {
                    return Err(Error::failure("terminal result frame has ok=false"));
                }
                Ok(Self::Result {
                    versions: WorkerVersions::from_value(object.get("versions"))?,
                    caps: parse_caps(object.get("caps")),
                    data: object.get("data").cloned().unwrap_or_else(|| Value::Object(Map::new())),
                })
            }
            Some("error") => {
                let error = object
                    .get("error")
                    .and_then(Value::as_object)
                    .ok_or_else(|| Error::failure("terminal error frame missing error object"))?;
                let kind = error
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::failure("terminal error frame missing error.kind"))?;
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                Ok(Self::Error {
                    kind: ErrorKind::parse(kind),
                    message,
                    versions: WorkerVersions::from_value(object.get("versions"))?,
                })
            }
            other => Err(Error::failure(format!(
                "unexpected worker frame: {other:?}"
            ))),
        }
    }
}

fn parse_progress(object: &Map<String, Value>) -> Result<ProgressFrame, Error> {
    fn required_u64(object: &Map<String, Value>, key: &str) -> Result<u64, Error> {
        object
            .get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::failure(format!("progress frame: {key} must be an integer")))
    }
    fn required_f64(object: &Map<String, Value>, key: &str) -> Result<f64, Error> {
        object
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::failure(format!("progress frame: {key} must be a number")))
    }
    fn optional_u64(object: &Map<String, Value>, key: &str) -> Result<Option<u64>, Error> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or_else(|| Error::failure(format!("progress frame: {key} must be an integer"))),
        }
    }
    fn optional_f64(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, Error> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_f64()
                .map(Some)
                .ok_or_else(|| Error::failure(format!("progress frame: {key} must be a number"))),
        }
    }
    Ok(ProgressFrame {
        downloaded_bytes: required_u64(object, "downloaded_bytes")?,
        total_bytes: optional_u64(object, "total_bytes")?,
        // The worker computes speed as a float (protocol.py::progress_frame);
        // accept integers and floats without failing closed on a decimal tick.
        speed_bps: required_f64(object, "speed_bps")? as u64,
        eta_seconds: optional_f64(object, "eta_seconds")?,
    })
}

fn parse_caps(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The typed result returned by `resolve` / `download_managed` / `finalize`.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaResult {
    /// `Direct` when the worker does not claim the URL (caller falls back to
    /// `paradl-engine`); `Managed` when the worker handled the media itself.
    pub route: MediaRoute,
    /// Worker payload; `None` on the DIRECT fallback.
    pub data: Option<MediaData>,
    /// Worker/tool versions from the terminal frame.
    pub versions: WorkerVersions,
    /// Capabilities advertised by the worker.
    pub caps: Vec<String>,
}

impl MediaResult {
    /// Whether the caller must perform a plain direct download instead.
    pub fn is_direct(&self) -> bool {
        self.route == MediaRoute::Direct
    }
}
