//! Public event schema: one typed, serde-serializable struct per oracle event.
//!
//! Every struct carries the two envelope fields (`event`, `timestamp`) followed
//! by the exact field set of its Python `UI.emit_event(...)` call site. Optional
//! fields are skipped when `None`, which is how event *variants* (e.g. `result`
//! A/B, `verification` success/error, `watchdog_action` 3 shapes) share one
//! struct without emitting absent keys.
//!
//! Field names, spellings and order are byte-exact. Notably `netprobe.ipv6_DNS`
//! keeps the oracle's unusual capitalization via `#[serde(rename)]`.

use serde::Serialize;
use serde_json::{Map, Value, json};

/// Define an event struct with the two envelope fields plus typed payload fields.
///
/// The generated `new` constructor takes a timestamp followed by every payload
/// field; passing `None` for an optional field omits that key from the JSONL
/// line, matching Python's kwargs-based `emit_event`.
macro_rules! event_struct {
    (
        $(#[$meta:meta])*
        $name:ident $literal:literal {
            $( $(#[$fmeta:meta])* $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize)]
        pub struct $name {
            /// The literal event name written into the `event` field.
            pub event: &'static str,
            /// `round(time.time(), 3)` at emission time (Python banker's rounding).
            pub timestamp: f64,
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        impl $name {
            #[doc = concat!("Construct a `", $literal, "` event.")]
            #[allow(clippy::too_many_arguments)]
            pub fn new(timestamp: f64, $($field: $ty),*) -> Self {
                Self {
                    event: $literal,
                    timestamp,
                    $( $field, )*
                }
            }
        }
    };
}

/// Canonical event-name literals, used by every caller and golden test.
pub mod names {
    /// `api_server_started` (E17).
    pub const API_SERVER_STARTED: &str = "api_server_started";
    /// `auth_remove` (E08/E18).
    pub const AUTH_REMOVE: &str = "auth_remove";
    /// `auth_set` (E08/E19).
    pub const AUTH_SET: &str = "auth_set";
    /// `auth_status` (E08/E20).
    pub const AUTH_STATUS: &str = "auth_status";
    /// `bench` (E21).
    pub const BENCH: &str = "bench";
    /// `clean` (E22).
    pub const CLEAN: &str = "clean";
    /// `config` (E23).
    pub const CONFIG: &str = "config";
    /// `config_reset` (E24).
    pub const CONFIG_RESET: &str = "config_reset";
    /// `config_updated` (E25).
    pub const CONFIG_UPDATED: &str = "config_updated";
    /// `deferred` (E26).
    pub const DEFERRED: &str = "deferred";
    /// `diagnose` (E27).
    pub const DIAGNOSE: &str = "diagnose";
    /// `doctor` (E10/E28).
    pub const DOCTOR: &str = "doctor";
    /// `error` (E29).
    pub const ERROR: &str = "error";
    /// `hashes` (E30).
    pub const HASHES: &str = "hashes";
    /// `history` (E31).
    pub const HISTORY: &str = "history";
    /// `hook_error` (E14/E32).
    pub const HOOK_ERROR: &str = "hook_error";
    /// `hook_executed` (E14/E33).
    pub const HOOK_EXECUTED: &str = "hook_executed";
    /// `hook_failed` (E14/E34).
    pub const HOOK_FAILED: &str = "hook_failed";
    /// `hook_timeout` (E14/E35).
    pub const HOOK_TIMEOUT: &str = "hook_timeout";
    /// `hosts_reset` (E36).
    pub const HOSTS_RESET: &str = "hosts_reset";
    /// `info` (E37).
    pub const INFO: &str = "info";
    /// `install_security_check` (E09/E38).
    pub const INSTALL_SECURITY_CHECK: &str = "install_security_check";
    /// `interrupted` (E39).
    pub const INTERRUPTED: &str = "interrupted";
    /// `launchd_plist_generated` (E06/E40).
    pub const LAUNCHD_PLIST_GENERATED: &str = "launchd_plist_generated";
    /// `mirror_dropped` (E04/E41).
    pub const MIRROR_DROPPED: &str = "mirror_dropped";
    /// `mirror_rejected` (E04/E42).
    pub const MIRROR_REJECTED: &str = "mirror_rejected";
    /// `netprobe` (E43).
    pub const NETPROBE: &str = "netprobe";
    /// `profiles` (E07/E44).
    pub const PROFILES: &str = "profiles";
    /// `profiles_reset` (E07/E45).
    pub const PROFILES_RESET: &str = "profiles_reset";
    /// `progress` (E02/E46).
    pub const PROGRESS: &str = "progress";
    /// `queue_crash_recovered` (E06/E47).
    pub const QUEUE_CRASH_RECOVERED: &str = "queue_crash_recovered";
    /// `queue_job_added` (E06/E48).
    pub const QUEUE_JOB_ADDED: &str = "queue_job_added";
    /// `queue_job_deferred` (E06/E49).
    pub const QUEUE_JOB_DEFERRED: &str = "queue_job_deferred";
    /// `queue_job_started` (E06/E50).
    pub const QUEUE_JOB_STARTED: &str = "queue_job_started";
    /// `queue_list` (E06/E51).
    pub const QUEUE_LIST: &str = "queue_list";
    /// `queue_paused` (E52).
    pub const QUEUE_PAUSED: &str = "queue_paused";
    /// `queue_removed` (E53).
    pub const QUEUE_REMOVED: &str = "queue_removed";
    /// `queue_resumed` (E54).
    pub const QUEUE_RESUMED: &str = "queue_resumed";
    /// `resilience_event` (E13).
    pub const RESILIENCE_EVENT: &str = "resilience_event";
    /// `result` (E12/E55/E56).
    pub const RESULT: &str = "result";
    /// `target_classified` (E01/E57).
    pub const TARGET_CLASSIFIED: &str = "target_classified";
    /// `throttle_launchd_plist_generated` (E58, single-emitter fix).
    pub const THROTTLE_LAUNCHD_PLIST_GENERATED: &str = "throttle_launchd_plist_generated";
    /// `verification` (E11/E59).
    pub const VERIFICATION: &str = "verification";
    /// `watchdog_action` (E03/E60).
    pub const WATCHDOG_ACTION: &str = "watchdog_action";
    /// `when_fast_probe` (E05/E61).
    pub const WHEN_FAST_PROBE: &str = "when_fast_probe";
}

event_struct! {
    /// API server startup notification (E17).
    ApiServerStartedEvent "api_server_started" {
        /// Bind host.
        host: String,
        /// Bind port.
        port: u16,
    }
}

event_struct! {
    /// A platform credential was removed (E18).
    AuthRemoveEvent "auth_remove" {
        /// Logical service (`hf` or `github`).
        service: String,
        /// Whether removal succeeded.
        success: bool,
    }
}

event_struct! {
    /// A platform credential was stored (E19).
    AuthSetEvent "auth_set" {
        /// Logical service (`hf` or `github`).
        service: String,
        /// Whether storage succeeded.
        success: bool,
    }
}

event_struct! {
    /// Authentication status; never includes token values (E20).
    AuthStatusEvent "auth_status" {
        /// Per-service source metadata (env/keychain/none).
        services: Value,
    }
}

event_struct! {
    /// Benchmark results (E21).
    BenchEvent "bench" {
        /// Benchmarked URL.
        url: String,
        /// Requested duration cap in seconds.
        duration_limit_sec: f64,
        /// Per-mode measurements.
        results: Value,
    }
}

event_struct! {
    /// Orphan-file cleanup result (E22).
    CleanEvent "clean" {
        /// Cleaned directory.
        directory: String,
        /// Removed paths.
        removed: Vec<String>,
    }
}

event_struct! {
    /// Current configuration snapshot (E23).
    ConfigEvent "config" {
        /// Serialized configuration.
        config: Value,
    }
}

event_struct! {
    /// Configuration reset snapshot (E24).
    ConfigResetEvent "config_reset" {
        /// Reset configuration.
        config: Value,
    }
}

event_struct! {
    /// One configuration key was updated (E25).
    ConfigUpdatedEvent "config_updated" {
        /// Updated key.
        key: String,
        /// New value.
        value: Value,
    }
}

event_struct! {
    /// A job was deferred by `--at` or `--when-fast` (E26).
    DeferredEvent "deferred" {
        /// Deferral cause.
        reason: String,
        /// Scheduled start time, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        start_at: Option<String>,
        /// Seconds until due, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        wait_seconds: Option<f64>,
    }
}

event_struct! {
    /// Diagnose payload; mirrors `doctor` (E27).
    DiagnoseEvent "diagnose" {
        /// Toolchain version.
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// yt-dlp version.
        #[serde(skip_serializing_if = "Option::is_none")]
        ytdlp: Option<String>,
        /// yt-dlp-ejs version.
        #[serde(skip_serializing_if = "Option::is_none")]
        ytdlp_ejs: Option<String>,
        /// Auth status summary.
        #[serde(skip_serializing_if = "Option::is_none")]
        auth: Option<Value>,
        /// macOS utility availability.
        #[serde(skip_serializing_if = "Option::is_none")]
        macos_utilities: Option<Value>,
        /// Core tool availability.
        #[serde(skip_serializing_if = "Option::is_none")]
        core_tools: Option<Value>,
        /// Suggested fixes.
        #[serde(skip_serializing_if = "Option::is_none")]
        fixes: Option<Value>,
    }
}

event_struct! {
    /// Doctor payload (E28).
    DoctorEvent "doctor" {
        /// Toolchain version.
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        /// yt-dlp version.
        #[serde(skip_serializing_if = "Option::is_none")]
        ytdlp: Option<String>,
        /// yt-dlp-ejs version.
        #[serde(skip_serializing_if = "Option::is_none")]
        ytdlp_ejs: Option<String>,
        /// Auth status summary.
        #[serde(skip_serializing_if = "Option::is_none")]
        auth: Option<Value>,
        /// macOS utility availability.
        #[serde(skip_serializing_if = "Option::is_none")]
        macos_utilities: Option<Value>,
        /// Core tool availability.
        #[serde(skip_serializing_if = "Option::is_none")]
        core_tools: Option<Value>,
        /// Suggested fixes.
        #[serde(skip_serializing_if = "Option::is_none")]
        fixes: Option<Value>,
    }
}

event_struct! {
    /// Error event with all known variants (E29).
    ErrorEvent "error" {
        /// Human-readable message (present in every variant).
        message: String,
        /// Structured error reason.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// HTTP status code, when applicable.
        #[serde(skip_serializing_if = "Option::is_none")]
        status_code: Option<i32>,
        /// Offending target URL.
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Measured throughput in bytes/s.
        #[serde(skip_serializing_if = "Option::is_none")]
        measured_bps: Option<f64>,
        /// Effective threshold in bytes/s.
        #[serde(skip_serializing_if = "Option::is_none")]
        threshold_bps: Option<f64>,
    }
}

event_struct! {
    /// Hash computation result (E30).
    HashesEvent "hashes" {
        /// Hashed path.
        file: String,
        /// File name.
        filename: String,
        /// File size in bytes.
        size: i64,
        /// Computation duration in seconds.
        duration: f64,
        /// Algorithm -> digest map.
        hashes: Value,
    }
}

event_struct! {
    /// Download history listing (E31).
    HistoryEvent "history" {
        /// History entries.
        entries: Value,
        /// Number of entries.
        count: usize,
        /// Optional search term.
        #[serde(skip_serializing_if = "Option::is_none")]
        search: Option<String>,
        /// Optional target-type filter.
        #[serde(skip_serializing_if = "Option::is_none")]
        target_type: Option<String>,
        /// Optional success filter.
        #[serde(skip_serializing_if = "Option::is_none")]
        success: Option<bool>,
    }
}

event_struct! {
    /// A hook process could not be spawned (E32).
    HookErrorEvent "hook_error" {
        /// Hook path or name.
        hook: String,
        /// Spawn error message.
        error: String,
    }
}

event_struct! {
    /// A hook executed successfully (E33).
    HookExecutedEvent "hook_executed" {
        /// Hook path or name.
        hook: String,
        /// Always `true` for this event.
        success: bool,
    }
}

event_struct! {
    /// A hook exited non-zero (E34).
    HookFailedEvent "hook_failed" {
        /// Hook path or name.
        hook: String,
        /// Process exit code.
        exit_code: i32,
        /// Trimmed stderr, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
    }
}

event_struct! {
    /// A hook exceeded its 30-second timeout and was SIGKILLed (E35).
    HookTimeoutEvent "hook_timeout" {
        /// Hook path or name.
        hook: String,
        /// Timeout in seconds.
        timeout_seconds: f64,
    }
}

event_struct! {
    /// Saved host strategies were cleared.
    HostsResetEvent "hosts_reset" {
        /// Number of cleared strategies.
        cleared: usize,
    }
}

event_struct! {
    /// Target inspection result (E37).
    InfoEvent "info" {
        /// Inspected target.
        target: String,
        /// Remote/local properties.
        properties: Value,
    }
}

event_struct! {
    /// Install-time code-signing / Gatekeeper result (E38).
    InstallSecurityCheckEvent "install_security_check" {
        /// Installed file path.
        file: String,
        /// `codesign --verify` result.
        codesign_valid: bool,
        /// Human-readable codesign detail.
        codesign_detail: String,
        /// Gatekeeper assessment result.
        gatekeeper_accepted: bool,
        /// Human-readable Gatekeeper detail.
        gatekeeper_detail: String,
        /// Whether paradl cryptographically verified the binary.
        verified_by_pdl: bool,
    }
}

event_struct! {
    /// SIGINT/SIGTERM interruption (E39).
    InterruptedEvent "interrupted" {
        /// Signal name.
        signal: String,
        /// Exit code (130).
        exit_code: i32,
    }
}

event_struct! {
    /// A queue LaunchAgent plist was generated (E40).
    LaunchdPlistGeneratedEvent "launchd_plist_generated" {
        /// Plist path.
        path: String,
        /// LaunchAgent label.
        label: String,
        /// Log directory.
        logs_dir: String,
        /// Validation message.
        lint: String,
    }
}

event_struct! {
    /// A mirror was dropped after repeated failures (E41).
    MirrorDroppedEvent "mirror_dropped" {
        /// Mirror URL.
        mirror_url: String,
        /// Observed failure count.
        failures: i32,
        /// Drop reason.
        reason: String,
    }
}

event_struct! {
    /// A mirror was rejected for a size mismatch (E42).
    MirrorRejectedEvent "mirror_rejected" {
        /// Mirror URL.
        mirror_url: String,
        /// Rejection reason.
        reason: String,
        /// Expected byte length.
        expected_length: i64,
        /// Mirror-reported byte length.
        mirror_length: i64,
    }
}

event_struct! {
    /// `netprobe` diagnosis result (E43).
    NetprobeEvent "netprobe" {
        /// Probed URL.
        url: String,
        /// Probed host.
        host: String,
        /// Whether Range requests are supported.
        supports_range: bool,
        /// Whether IPv6 DNS resolution succeeded (oracle spelling).
        #[serde(rename = "ipv6_DNS")]
        ipv6_dns: bool,
        /// Inferred throttle class.
        throttle_class: String,
        /// Recommended connection count.
        recommended_connections: i32,
        /// Recommended watchdog action.
        recommended_action: String,
        /// Overall verdict.
        verdict: String,
        /// Whether HTTP/3 was detected.
        http3_supported: bool,
        /// HTTP/3 probe detail.
        http3_detail: String,
        /// Overall pass/fail flag.
        passes: bool,
    }
}

event_struct! {
    /// Learned per-host/hour/network profiles (E44).
    ProfilesEvent "profiles" {
        /// Profile map.
        profiles: Value,
        /// Number of profiles.
        count: usize,
    }
}

event_struct! {
    /// Learned profiles were reset for a host (E45).
    ProfilesResetEvent "profiles_reset" {
        /// Host whose profiles were cleared.
        host: String,
        /// Number of cleared profiles.
        cleared: usize,
    }
}

event_struct! {
    /// One download progress tick (E02/E46).
    ProgressEvent "progress" {
        /// Human-readable transfer title.
        title: String,
        /// Bytes downloaded so far.
        downloaded_bytes: i64,
        /// Total bytes, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        total_bytes: Option<i64>,
        /// Completion percentage.
        #[serde(skip_serializing_if = "Option::is_none")]
        percent: Option<f64>,
        /// Aggregate speed in bytes/s.
        #[serde(skip_serializing_if = "Option::is_none")]
        speed_bps: Option<f64>,
        /// Estimated seconds remaining.
        #[serde(skip_serializing_if = "Option::is_none")]
        eta_seconds: Option<f64>,
        /// Active connection count.
        #[serde(skip_serializing_if = "Option::is_none")]
        connections: Option<i32>,
        /// Number of connected peers.
        #[serde(skip_serializing_if = "Option::is_none")]
        peer_count: Option<i32>,
        /// Watchdog state label.
        #[serde(skip_serializing_if = "Option::is_none")]
        watchdog_state: Option<String>,
        /// Per-file progress details.
        #[serde(skip_serializing_if = "Option::is_none")]
        file_progress: Option<Value>,
        /// Per-connection speed details.
        #[serde(skip_serializing_if = "Option::is_none")]
        connection_speeds: Option<Value>,
    }
}

event_struct! {
    /// Crashed `running` jobs were re-queued (E47).
    QueueCrashRecoveredEvent "queue_crash_recovered" {
        /// Number of recovered jobs.
        recovered_count: usize,
    }
}

event_struct! {
    /// A job was added to the persistent queue (E48).
    QueueJobAddedEvent "queue_job_added" {
        /// Queue job id.
        job_id: i64,
        /// Download target.
        target: String,
        /// Whether the job came from history.
        #[serde(skip_serializing_if = "Option::is_none")]
        from_history: Option<bool>,
        /// Job priority.
        #[serde(skip_serializing_if = "Option::is_none")]
        priority: Option<i64>,
        /// Whether AC power is required.
        #[serde(skip_serializing_if = "Option::is_none")]
        require_ac: Option<bool>,
        /// `--when-fast` threshold in bytes/s.
        #[serde(skip_serializing_if = "Option::is_none")]
        when_fast_bps: Option<f64>,
        /// Scheduled start time.
        #[serde(skip_serializing_if = "Option::is_none")]
        start_at: Option<String>,
    }
}

event_struct! {
    /// A queued job was deferred (E49).
    QueueJobDeferredEvent "queue_job_deferred" {
        /// Queue job id.
        job_id: i64,
        /// Deferral reason (`start_at` or `when_fast`).
        reason: String,
        /// Scheduled start time, for `start_at` deferrals.
        #[serde(skip_serializing_if = "Option::is_none")]
        start_at: Option<String>,
        /// Measured speed, for `when_fast` deferrals.
        #[serde(skip_serializing_if = "Option::is_none")]
        measured_bps: Option<f64>,
        /// Required speed threshold, for `when_fast` deferrals.
        #[serde(skip_serializing_if = "Option::is_none")]
        threshold_bps: Option<f64>,
    }
}

event_struct! {
    /// A queued job started (E50).
    QueueJobStartedEvent "queue_job_started" {
        /// Queue job id.
        job_id: i64,
        /// Download target.
        target: String,
        /// Job priority.
        priority: i64,
        /// Effective connection count.
        connections: i32,
    }
}

/// One row of a `queue_list` event.
///
/// Field order is byte-exact against `src/paradl/resilience/queue.py:464-478`.
/// All 13 keys are always emitted (including `null` optionals), matching the
/// oracle dict comprehension — unlike an `Option` event payload, these fields
/// must not be skipped.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueJobRow {
    /// Queue job id.
    pub id: i64,
    /// Download target.
    pub target: String,
    /// Job status (`queued`, `running`, `paused`, `completed`, `failed`).
    pub status: String,
    /// Job priority.
    pub priority: i64,
    /// Connection count.
    pub connections: i64,
    /// Whether AC power is required.
    pub require_ac: bool,
    /// `--when-fast` threshold in bytes/s.
    pub when_fast_bps: f64,
    /// Scheduled start time.
    pub start_at: Option<String>,
    /// Bytes downloaded so far.
    pub bytes_downloaded: i64,
    /// Expected content length.
    pub expected_length: Option<i64>,
    /// Absolute output directory.
    pub out_dir: String,
    /// Output filename, when known.
    pub filename: Option<String>,
    /// Last error message, when failed.
    pub error: Option<String>,
}

impl QueueJobRow {
    /// Construct a row with all 13 oracle fields in insertion order.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: i64,
        target: String,
        status: String,
        priority: i64,
        connections: i64,
        require_ac: bool,
        when_fast_bps: f64,
        start_at: Option<String>,
        bytes_downloaded: i64,
        expected_length: Option<i64>,
        out_dir: String,
        filename: Option<String>,
        error: Option<String>,
    ) -> Self {
        Self {
            id,
            target,
            status,
            priority,
            connections,
            require_ac,
            when_fast_bps,
            start_at,
            bytes_downloaded,
            expected_length,
            out_dir,
            filename,
            error,
        }
    }
}

/// Persistent queue listing (E51) with insertion-ordered typed rows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueueListEvent {
    /// The literal event name.
    pub event: &'static str,
    /// `round(time.time(), 3)` at emission time.
    pub timestamp: f64,
    /// Typed job rows; each keeps the oracle's 13-key insertion order.
    pub jobs: Vec<QueueJobRow>,
}

impl QueueListEvent {
    /// Construct a `queue_list` event from typed rows.
    pub fn new(timestamp: f64, jobs: Vec<QueueJobRow>) -> Self {
        Self {
            event: "queue_list",
            timestamp,
            jobs,
        }
    }
}

event_struct! {
    /// Queue pause result (E52).
    QueuePausedEvent "queue_paused" {
        /// Paused count or identifiers.
        paused: Value,
    }
}

event_struct! {
    /// Queue removal result (E53).
    QueueRemovedEvent "queue_removed" {
        /// Number of removed jobs.
        removed: usize,
    }
}

event_struct! {
    /// Queue resume result (E54).
    QueueResumedEvent "queue_resumed" {
        /// Resumed count or identifiers.
        resumed: Value,
        /// Whether `--all` was used.
        #[serde(skip_serializing_if = "Option::is_none")]
        all: Option<bool>,
    }
}

event_struct! {
    /// A WAKE / NET_CHANGE / STALL source observation (E13).
    ResilienceEventEvent "resilience_event" {
        /// `wake`, `net_change`, or `stall`.
        event_type: String,
        /// Human-readable reason.
        reason: String,
        /// Rounded numeric details.
        detail: Value,
    }
}

event_struct! {
    /// Terminal transfer result; union of variants A and B (E12/E55/E56).
    ResultEvent "result" {
        /// Originating subcommand (variant A).
        #[serde(skip_serializing_if = "Option::is_none")]
        subcommand: Option<String>,
        /// Download target.
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        /// Success flag.
        success: bool,
        /// Final file path.
        #[serde(skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
        /// Whether checksum verification passed.
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum_verified: Option<bool>,
        /// Error message, when failed.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Process exit code.
        exit_code: i32,
        /// Resolved target type (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        target_type: Option<String>,
        /// Bytes downloaded (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        bytes_downloaded: Option<i64>,
        /// Transfer duration in seconds (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<f64>,
        /// Average speed in bytes/s (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        avg_speed_bps: Option<f64>,
        /// Socket recycle count (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        recycle_count: Option<i32>,
        /// Audio format produced by the media worker.
        #[serde(skip_serializing_if = "Option::is_none")]
        audio_format_produced: Option<String>,
        /// Checksum algorithm (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum_algo: Option<String>,
        /// Checksum digest (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum_digest: Option<String>,
        /// Whether an ADB sync was performed (variant B).
        #[serde(skip_serializing_if = "Option::is_none")]
        adb_synced: Option<bool>,
    }
}

event_struct! {
    /// Target classification result (E01/E57).
    TargetClassifiedEvent "target_classified" {
        /// Original target string.
        target: String,
        /// Resolved target type.
        target_type: String,
        /// Cleaned download URL.
        clean_url: String,
        /// Suggested filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        filename_hint: Option<String>,
        /// Remote content length.
        #[serde(skip_serializing_if = "Option::is_none")]
        content_length: Option<i64>,
        /// Whether Range requests are accepted.
        #[serde(skip_serializing_if = "Option::is_none")]
        accept_ranges: Option<bool>,
    }
}

event_struct! {
    /// Throttle sampler plist event: the raw-`json.dumps` fix for E58.
    /// Goes through this emitter so it gains `timestamp` and redaction.
    ThrottleLaunchdPlistGeneratedEvent "throttle_launchd_plist_generated" {
        /// Plist path.
        path: String,
        /// LaunchAgent label.
        label: String,
        /// Validation message.
        lint: String,
    }
}

event_struct! {
    /// Checksum verification result; union of success and error variants (E59).
    VerificationEvent "verification" {
        /// Verified file path.
        file: String,
        /// Algorithm name.
        #[serde(skip_serializing_if = "Option::is_none")]
        algorithm: Option<String>,
        /// Expected digest.
        #[serde(skip_serializing_if = "Option::is_none")]
        expected: Option<String>,
        /// Computed digest.
        #[serde(skip_serializing_if = "Option::is_none")]
        computed: Option<String>,
        /// Verification outcome.
        verified: bool,
        /// Call site / source label.
        source: String,
        /// Verification error, when failed.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    }
}

/// Short `watchdog_action` variant from `engines/aria2.py` (E60).
///
/// Flattens the consumed `paradl_watchdog::WatchdogActionEvent` and adds the
/// optional `supports_range` key emitted by the early guard call sites. The
/// watchdog crate models the full variant only, so this adapter keeps the
/// union of all three E60 shapes reachable through the single emitter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatchdogActionRangeEvent {
    /// The watchdog-produced event, flattened into the same JSON object.
    #[serde(flatten)]
    pub action: paradl_watchdog::WatchdogActionEvent,
    /// Whether the target accepts Range requests, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_range: Option<bool>,
}

impl WatchdogActionRangeEvent {
    /// Wrap a consumed watchdog event with a `supports_range` value.
    pub fn new(action: paradl_watchdog::WatchdogActionEvent, supports_range: Option<bool>) -> Self {
        Self {
            action,
            supports_range,
        }
    }
}

event_struct! {
    /// One `--when-fast` speed probe (E05/E61).
    WhenFastProbeEvent "when_fast_probe" {
        /// Probed URL.
        url: String,
        /// 1-based attempt number.
        attempt: i32,
        /// Measured speed in bytes/s, rounded to 2 decimals.
        measured_bps: f64,
        /// Required speed in bytes/s, rounded to 2 decimals.
        threshold_bps: f64,
        /// Whether the probe passed.
        passed: bool,
    }
}

/// Every supported event in one typed enum.
///
/// `#[serde(untagged)]` means the enum serializes exactly like the inner
/// struct, so `emit_jsonl(&event)` works uniformly for local and consumed
/// (e.g. watchdog) event types.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Event {
    /// API server started.
    ApiServerStarted(ApiServerStartedEvent),
    /// Credential removed.
    AuthRemove(AuthRemoveEvent),
    /// Credential stored.
    AuthSet(AuthSetEvent),
    /// Authentication status.
    AuthStatus(AuthStatusEvent),
    /// Benchmark results.
    Bench(BenchEvent),
    /// Cleanup result.
    Clean(CleanEvent),
    /// Configuration snapshot.
    Config(ConfigEvent),
    /// Configuration reset.
    ConfigReset(ConfigResetEvent),
    /// Configuration key update.
    ConfigUpdated(ConfigUpdatedEvent),
    /// Job deferral.
    Deferred(DeferredEvent),
    /// Diagnose output.
    Diagnose(DiagnoseEvent),
    /// Doctor output.
    Doctor(DoctorEvent),
    /// Error report.
    Error(ErrorEvent),
    /// Hash results.
    Hashes(HashesEvent),
    /// History listing.
    History(HistoryEvent),
    /// Hook spawn error.
    HookError(HookErrorEvent),
    /// Hook executed.
    HookExecuted(HookExecutedEvent),
    /// Hook failed.
    HookFailed(HookFailedEvent),
    /// Hook timed out.
    HookTimeout(HookTimeoutEvent),
    /// Host strategies reset.
    HostsReset(HostsResetEvent),
    /// Target info.
    Info(InfoEvent),
    /// Install security check.
    InstallSecurityCheck(InstallSecurityCheckEvent),
    /// Interruption notice.
    Interrupted(InterruptedEvent),
    /// Queue plist generated.
    LaunchdPlistGenerated(LaunchdPlistGeneratedEvent),
    /// Mirror dropped.
    MirrorDropped(MirrorDroppedEvent),
    /// Mirror rejected.
    MirrorRejected(MirrorRejectedEvent),
    /// Network probe result.
    Netprobe(NetprobeEvent),
    /// Learned profiles.
    Profiles(ProfilesEvent),
    /// Profiles reset.
    ProfilesReset(ProfilesResetEvent),
    /// Progress tick.
    Progress(ProgressEvent),
    /// Crash recovery.
    QueueCrashRecovered(QueueCrashRecoveredEvent),
    /// Job added.
    QueueJobAdded(QueueJobAddedEvent),
    /// Job deferred.
    QueueJobDeferred(QueueJobDeferredEvent),
    /// Job started.
    QueueJobStarted(QueueJobStartedEvent),
    /// Queue listing.
    QueueList(QueueListEvent),
    /// Queue paused.
    QueuePaused(QueuePausedEvent),
    /// Queue removal.
    QueueRemoved(QueueRemovedEvent),
    /// Queue resumed.
    QueueResumed(QueueResumedEvent),
    /// Resilience source event.
    ResilienceEvent(ResilienceEventEvent),
    /// Terminal result.
    Result(ResultEvent),
    /// Target classification.
    TargetClassified(TargetClassifiedEvent),
    /// Throttle sampler plist generated (E58).
    ThrottleLaunchdPlistGenerated(ThrottleLaunchdPlistGeneratedEvent),
    /// Verification result.
    Verification(VerificationEvent),
    /// Watchdog action, consumed from `paradl-watchdog`.
    WatchdogAction(paradl_watchdog::WatchdogActionEvent),
    /// Short watchdog action variant with `supports_range` (E60 union).
    WatchdogActionRange(WatchdogActionRangeEvent),
    /// `--when-fast` probe.
    WhenFastProbe(WhenFastProbeEvent),
}

/// Sample timestamp used by [`Event::all`]; the oracle envelope value.
const SAMPLE_TS: f64 = 1_763_376_000.123;

fn sample_obj() -> Value {
    let mut map = Map::new();
    map.insert("key".to_string(), Value::String("value".to_string()));
    Value::Object(map)
}

fn sample_array() -> Value {
    json!([1, 2, 3])
}

impl Event {
    /// Return the canonical event name of this variant.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ApiServerStarted(_) => names::API_SERVER_STARTED,
            Self::AuthRemove(_) => names::AUTH_REMOVE,
            Self::AuthSet(_) => names::AUTH_SET,
            Self::AuthStatus(_) => names::AUTH_STATUS,
            Self::Bench(_) => names::BENCH,
            Self::Clean(_) => names::CLEAN,
            Self::Config(_) => names::CONFIG,
            Self::ConfigReset(_) => names::CONFIG_RESET,
            Self::ConfigUpdated(_) => names::CONFIG_UPDATED,
            Self::Deferred(_) => names::DEFERRED,
            Self::Diagnose(_) => names::DIAGNOSE,
            Self::Doctor(_) => names::DOCTOR,
            Self::Error(_) => names::ERROR,
            Self::Hashes(_) => names::HASHES,
            Self::History(_) => names::HISTORY,
            Self::HookError(_) => names::HOOK_ERROR,
            Self::HookExecuted(_) => names::HOOK_EXECUTED,
            Self::HookFailed(_) => names::HOOK_FAILED,
            Self::HookTimeout(_) => names::HOOK_TIMEOUT,
            Self::HostsReset(_) => names::HOSTS_RESET,
            Self::Info(_) => names::INFO,
            Self::InstallSecurityCheck(_) => names::INSTALL_SECURITY_CHECK,
            Self::Interrupted(_) => names::INTERRUPTED,
            Self::LaunchdPlistGenerated(_) => names::LAUNCHD_PLIST_GENERATED,
            Self::MirrorDropped(_) => names::MIRROR_DROPPED,
            Self::MirrorRejected(_) => names::MIRROR_REJECTED,
            Self::Netprobe(_) => names::NETPROBE,
            Self::Profiles(_) => names::PROFILES,
            Self::ProfilesReset(_) => names::PROFILES_RESET,
            Self::Progress(_) => names::PROGRESS,
            Self::QueueCrashRecovered(_) => names::QUEUE_CRASH_RECOVERED,
            Self::QueueJobAdded(_) => names::QUEUE_JOB_ADDED,
            Self::QueueJobDeferred(_) => names::QUEUE_JOB_DEFERRED,
            Self::QueueJobStarted(_) => names::QUEUE_JOB_STARTED,
            Self::QueueList(_) => names::QUEUE_LIST,
            Self::QueuePaused(_) => names::QUEUE_PAUSED,
            Self::QueueRemoved(_) => names::QUEUE_REMOVED,
            Self::QueueResumed(_) => names::QUEUE_RESUMED,
            Self::ResilienceEvent(_) => names::RESILIENCE_EVENT,
            Self::Result(_) => names::RESULT,
            Self::TargetClassified(_) => names::TARGET_CLASSIFIED,
            Self::ThrottleLaunchdPlistGenerated(_) => names::THROTTLE_LAUNCHD_PLIST_GENERATED,
            Self::Verification(_) => names::VERIFICATION,
            Self::WatchdogAction(_) | Self::WatchdogActionRange(_) => names::WATCHDOG_ACTION,
            Self::WhenFastProbe(_) => names::WHEN_FAST_PROBE,
        }
    }

    /// Return one representative instance of every supported event.
    ///
    /// All optional fields are populated so golden tests can assert the full
    /// union of fields for each name. The 44 oracle names plus `resilience_event`
    /// are all present.
    #[allow(clippy::too_many_lines)]
    pub fn all() -> Vec<Self> {
        use names as n;
        vec![
            Self::ApiServerStarted(ApiServerStartedEvent::new(SAMPLE_TS, "127.0.0.1".into(), 8080)),
            Self::AuthRemove(AuthRemoveEvent::new(SAMPLE_TS, "hf".into(), true)),
            Self::AuthSet(AuthSetEvent::new(SAMPLE_TS, "github".into(), true)),
            Self::AuthStatus(AuthStatusEvent::new(SAMPLE_TS, sample_obj())),
            Self::Bench(BenchEvent::new(SAMPLE_TS, "https://e.test".into(), 60.0, sample_obj())),
            Self::Clean(CleanEvent::new(SAMPLE_TS, "/tmp".into(), vec!["a.part".into()])),
            Self::Config(ConfigEvent::new(SAMPLE_TS, sample_obj())),
            Self::ConfigReset(ConfigResetEvent::new(SAMPLE_TS, sample_obj())),
            Self::ConfigUpdated(ConfigUpdatedEvent::new(SAMPLE_TS, "key".into(), Value::Bool(true))),
            Self::Deferred(DeferredEvent::new(SAMPLE_TS, "at".into(), Some("12:00".into()), Some(42.0))),
            // The W0-2 golden captured bare `emit_event("doctor")` calls with
            // no kwargs; the seven typed fields are optional variants (E28)
            // populated by W4-1. Sample the golden (empty) shape here.
            Self::Diagnose(DiagnoseEvent::new(
                SAMPLE_TS, None, None, None, None, None, None, None,
            )),
            Self::Doctor(DoctorEvent::new(
                SAMPLE_TS, None, None, None, None, None, None, None,
            )),
            Self::Error(ErrorEvent::new(
                SAMPLE_TS,
                "boom".into(),
                Some("timeout".into()),
                Some(503),
                Some("https://e.test".into()),
                Some(100.0),
                Some(200.0),
            )),
            Self::Hashes(HashesEvent::new(
                SAMPLE_TS,
                "/tmp/f".into(),
                "f".into(),
                1024,
                0.5,
                sample_obj(),
            )),
            Self::History(HistoryEvent::new(
                SAMPLE_TS,
                sample_array(),
                3,
                Some("q".into()),
                Some("direct".into()),
                Some(true),
            )),
            Self::HookError(HookErrorEvent::new(SAMPLE_TS, "/h".into(), "spawn failed".into())),
            Self::HookExecuted(HookExecutedEvent::new(SAMPLE_TS, "/h".into(), true)),
            Self::HookFailed(HookFailedEvent::new(SAMPLE_TS, "/h".into(), 7, Some("bad".into()))),
            Self::HookTimeout(HookTimeoutEvent::new(SAMPLE_TS, "/h".into(), 30.0)),
            Self::HostsReset(HostsResetEvent::new(SAMPLE_TS, 4)),
            Self::Info(InfoEvent::new(SAMPLE_TS, "https://e.test".into(), sample_obj())),
            Self::InstallSecurityCheck(InstallSecurityCheckEvent::new(
                SAMPLE_TS,
                "/bin/f".into(),
                true,
                "ok".into(),
                true,
                "accepted".into(),
                true,
            )),
            Self::Interrupted(InterruptedEvent::new(SAMPLE_TS, "SIGINT".into(), 130)),
            Self::LaunchdPlistGenerated(LaunchdPlistGeneratedEvent::new(
                SAMPLE_TS,
                "/tmp/f.plist".into(),
                "label".into(),
                "/logs".into(),
                "OK".into(),
            )),
            Self::MirrorDropped(MirrorDroppedEvent::new(SAMPLE_TS, "https://m".into(), 2, "repeated_failures".into())),
            Self::MirrorRejected(MirrorRejectedEvent::new(SAMPLE_TS, "https://m".into(), "size_mismatch".into(), 10, 20)),
            Self::Netprobe(NetprobeEvent::new(
                SAMPLE_TS,
                "https://e.test".into(),
                "e.test".into(),
                true,
                true,
                "plausible".into(),
                16,
                "tier2".into(),
                "pass".into(),
                false,
                "n/a".into(),
                true,
            )),
            Self::Profiles(ProfilesEvent::new(SAMPLE_TS, sample_obj(), 2)),
            Self::ProfilesReset(ProfilesResetEvent::new(SAMPLE_TS, "e.test".into(), 3)),
            Self::Progress(ProgressEvent::new(
                SAMPLE_TS,
                "demo.bin".into(),
                1024,
                Some(2048),
                Some(50.0),
                Some(1000.0),
                Some(1.0),
                Some(16),
                Some(4),
                Some("v2".into()),
                Some(sample_array()),
                Some(sample_array()),
            )),
            Self::QueueCrashRecovered(QueueCrashRecoveredEvent::new(SAMPLE_TS, 2)),
            Self::QueueJobAdded(QueueJobAddedEvent::new(
                SAMPLE_TS,
                1,
                "https://e.test".into(),
                Some(true),
                Some(5),
                Some(true),
                Some(5e6),
                Some("12:00".into()),
            )),
            Self::QueueJobDeferred(QueueJobDeferredEvent::new(
                SAMPLE_TS,
                1,
                "when_fast".into(),
                Some("12:00".into()),
                Some(100.0),
                Some(200.0),
            )),
            Self::QueueJobStarted(QueueJobStartedEvent::new(SAMPLE_TS, 1, "https://e.test".into(), 5, 16)),
            Self::QueueList(QueueListEvent::new(
                SAMPLE_TS,
                vec![QueueJobRow::new(
                    1,
                    "https://e.test".into(),
                    "queued".into(),
                    0,
                    16,
                    true,
                    0.0,
                    None,
                    0,
                    None,
                    "/tmp/out".into(),
                    None,
                    None,
                )],
            )),
            Self::QueuePaused(QueuePausedEvent::new(SAMPLE_TS, Value::Number(2.into()))),
            Self::QueueRemoved(QueueRemovedEvent::new(SAMPLE_TS, 2)),
            Self::QueueResumed(QueueResumedEvent::new(SAMPLE_TS, Value::Number(2.into()), Some(true))),
            Self::ResilienceEvent(ResilienceEventEvent::new(
                SAMPLE_TS,
                "wake".into(),
                "System wake detected".into(),
                sample_obj(),
            )),
            Self::Result(ResultEvent::new(
                SAMPLE_TS,
                Some("download".into()),
                Some("https://e.test".into()),
                true,
                Some("/tmp/f".into()),
                Some(true),
                Some("error-detail".into()),
                0,
                Some("direct".into()),
                Some(1024),
                Some(1.0),
                Some(1000.0),
                Some(2),
                Some("passthrough_aac".into()),
                Some("sha256".into()),
                Some("digest".into()),
                Some(false),
            )),
            Self::TargetClassified(TargetClassifiedEvent::new(
                SAMPLE_TS,
                "gh:o/r".into(),
                "github".into(),
                "https://e.test".into(),
                Some("f.zip".into()),
                Some(1024),
                Some(true),
            )),
            Self::ThrottleLaunchdPlistGenerated(ThrottleLaunchdPlistGeneratedEvent::new(
                SAMPLE_TS,
                "/tmp/sample.plist".into(),
                "throttle-sample".into(),
                "OK".into(),
            )),
            Self::Verification(VerificationEvent::new(
                SAMPLE_TS,
                "/tmp/f".into(),
                Some("sha256".into()),
                Some("expected".into()),
                Some("computed".into()),
                true,
                "github".into(),
                Some("verify-error".into()),
            )),
            Self::WatchdogAction(paradl_watchdog::WatchdogActionEvent {
                event: n::WATCHDOG_ACTION,
                timestamp: SAMPLE_TS,
                cycle: 1,
                mode: paradl_watchdog::WatchdogMode::V2,
                action: paradl_watchdog::WatchdogActionType::PauseUnpause,
                resilience_event: Some("wake".to_string()),
                reason: "low speed".into(),
                speed_bps: 500.0,
                threshold_bps: 1000.0,
                rolling_p90_bps: 1200.0,
                connections: 16,
            }),
            Self::WhenFastProbe(WhenFastProbeEvent::new(
                SAMPLE_TS,
                "https://e.test".into(),
                1,
                100.0,
                200.0,
                false,
            )),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_unique_and_complete() {
        let events = Event::all();
        assert_eq!(events.len(), 45);
        let mut seen = std::collections::BTreeSet::new();
        for event in &events {
            assert!(seen.insert(event.name()), "duplicate: {}", event.name());
        }
        assert_eq!(seen.len(), events.len());
    }

    #[test]
    fn netprobe_keeps_oracle_capitalization() {
        let event = Event::all()
            .into_iter()
            .find(|event| event.name() == "netprobe")
            .unwrap();
        let line = crate::emit_jsonl(&event);
        assert!(line.contains("\"ipv6_DNS\": true"));
    }
}
