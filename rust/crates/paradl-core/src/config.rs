//! Configuration model, validation, atomic persistence, and `PARADL_HOME` isolation.
//!
//! Byte-exact port of `src/paradl/core/config.py` (parity rows G01/G02).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::Error;
use crate::models::parse_rate;
use crate::paths::{config_file, default_bin_dir, get_paradl_home};

/// Current `config.json` schema version (Python `CONFIG_SCHEMA_VERSION`).
pub const CONFIG_SCHEMA_VERSION: i32 = 2;

/// Valid values for `PDLConfig::audio_codec` (Python `VALID_AUDIO_CODECS`).
pub const VALID_AUDIO_CODECS: &[&str] =
    &["lossless", "native", "mp3", "flac", "opus", "wav", "aac"];

/// Valid values for `PDLConfig::video_quality` (Python `VALID_VIDEO_QUALITIES`).
pub const VALID_VIDEO_QUALITIES: &[&str] = &["best", "4k", "1440p", "1080p", "720p", "480p"];

/// Valid values for `PDLConfig::watchdog` (Python `VALID_WATCHDOG_MODES`).
pub const VALID_WATCHDOG_MODES: &[&str] = &["v1", "v2", "off"];

/// Valid transfer engines (Python `VALID_ENGINES`).
pub const VALID_ENGINES: &[&str] = &["rust", "aria2"];

/// Config field names accepted from `config.json` and by [`PDLConfig::set`].
const KNOWN_FIELDS: &[&str] = &[
    "schema_version",
    "connections",
    "output_dir",
    "audio_codec",
    "video_quality",
    "auto_extract",
    "auto_adb_push",
    "bin_dir",
    "adb_music_dir",
    "adb_download_dir",
    "max_download_limit",
    "history_limit",
    "anti_throttle",
    "anti_throttle_grace",
    "watchdog",
    "ytdlp_player_client",
    "caffeinate",
    "net_watch",
    "stall_timeout",
    "require_ac",
    "pause_on_low_power",
    "isp",
    "doh_endpoint",
    "engine",
];

const BOOLEAN_FIELDS: &[&str] = &[
    "auto_extract",
    "auto_adb_push",
    "caffeinate",
    "net_watch",
    "require_ac",
    "pause_on_low_power",
];
const INTEGER_FIELDS: &[&str] = &["connections", "history_limit", "anti_throttle_grace"];
const OPTIONAL_STRING_FIELDS: &[&str] =
    &["max_download_limit", "anti_throttle", "ytdlp_player_client"];

static ACTIVE_ENGINE: Mutex<String> = Mutex::new(String::new());

/// Set the process-wide transfer engine, validated against [`VALID_ENGINES`].
///
/// Mirrors `paradl.core.config.set_active_engine`.
pub fn set_active_engine(name: &str) -> Result<(), Error> {
    let normalized = name.trim().to_ascii_lowercase();
    if !VALID_ENGINES.contains(&normalized.as_str()) {
        return Err(Error::usage(format!(
            "engine must be one of ['aria2', 'rust'], got '{name}'"
        )));
    }
    *ACTIVE_ENGINE
        .lock()
        .map_err(|e| Error::failure(e.to_string()))? = normalized;
    Ok(())
}

/// Return the process-wide transfer engine name (default `"rust"`).
pub fn get_active_engine() -> String {
    ACTIVE_ENGINE
        .lock()
        .map(|guard| {
            if guard.is_empty() {
                "rust".to_owned()
            } else {
                guard.clone()
            }
        })
        .unwrap_or_else(|_| "rust".to_owned())
}

/// The full `config.json` model with Python-compatible defaults and validation.
///
/// Field order matches the Python dataclass so serialized JSON is byte-exact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PDLConfig {
    /// Schema version; always forced to [`CONFIG_SCHEMA_VERSION`] on load/save.
    pub schema_version: i32,
    /// Concurrent connections, clamped to `1..=16`.
    pub connections: i32,
    /// Default output directory.
    pub output_dir: String,
    /// Audio codec (`VALID_AUDIO_CODECS`).
    pub audio_codec: String,
    /// Video quality (`VALID_VIDEO_QUALITIES`).
    pub video_quality: String,
    /// Automatically extract archives after download.
    pub auto_extract: bool,
    /// Automatically push media to Android via adb.
    pub auto_adb_push: bool,
    /// Directory for installed helper binaries.
    pub bin_dir: String,
    /// adb music destination.
    pub adb_music_dir: String,
    /// adb download destination.
    pub adb_download_dir: String,
    /// Optional maximum download rate string.
    pub max_download_limit: Option<String>,
    /// Number of history rows shown by default.
    pub history_limit: i32,
    /// Optional anti-throttle rate string.
    pub anti_throttle: Option<String>,
    /// Anti-throttle grace period in seconds (minimum 3).
    pub anti_throttle_grace: i32,
    /// Watchdog mode (`VALID_WATCHDOG_MODES`).
    pub watchdog: String,
    /// Optional forced yt-dlp player client.
    pub ytdlp_player_client: Option<String>,
    /// Use caffeinate on macOS.
    pub caffeinate: bool,
    /// Monitor network changes.
    pub net_watch: bool,
    /// Stall timeout in seconds.
    pub stall_timeout: f64,
    /// Require AC power.
    pub require_ac: bool,
    /// Pause on low battery.
    pub pause_on_low_power: bool,
    /// ISP name or `auto`.
    pub isp: String,
    /// Optional DNS-over-HTTPS endpoint.
    pub doh_endpoint: Option<String>,
    /// Active transfer engine (`VALID_ENGINES`).
    pub engine: String,
}

impl Default for PDLConfig {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            connections: 16,
            output_dir: ".".to_owned(),
            audio_codec: "aac".to_owned(),
            video_quality: "best".to_owned(),
            auto_extract: false,
            auto_adb_push: false,
            bin_dir: default_bin_dir(),
            adb_music_dir: "/storage/emulated/0/Music".to_owned(),
            adb_download_dir: "/storage/emulated/0/Download".to_owned(),
            max_download_limit: None,
            history_limit: 50,
            anti_throttle: Some("4MBps".to_owned()),
            anti_throttle_grace: 5,
            watchdog: "v2".to_owned(),
            ytdlp_player_client: None,
            caffeinate: true,
            net_watch: true,
            stall_timeout: 30.0,
            require_ac: false,
            pause_on_low_power: false,
            isp: "auto".to_owned(),
            doh_endpoint: None,
            engine: "rust".to_owned(),
        }
    }
}

impl PDLConfig {
    /// Validate and normalize every field, returning [`Error::Usage`] on bad input.
    ///
    /// Port of `PDLConfig.validate()`: clamps connections to 16, disables
    /// off-like rate strings, enforces a minimum 3-second grace period, and
    /// lower-cases enumerated values.
    pub fn validate(&mut self) -> Result<(), Error> {
        if self.schema_version < 1 {
            return Err(Error::usage(format!(
                "schema_version must be a positive integer, got {}",
                self.schema_version
            )));
        }

        if self.connections < 1 {
            return Err(Error::usage(format!(
                "connections must be >= 1, got {}",
                self.connections
            )));
        }
        self.connections = self.connections.min(16);

        if self.output_dir.trim().is_empty() {
            return Err(Error::usage("output_dir must be a non-empty string"));
        }

        if !VALID_AUDIO_CODECS.contains(&self.audio_codec.to_ascii_lowercase().as_str()) {
            return Err(Error::usage(format!(
                "audio_codec must be one of ['aac', 'flac', 'lossless', 'mp3', 'native', 'opus', 'wav'], got '{}'",
                self.audio_codec
            )));
        }
        self.audio_codec = self.audio_codec.to_ascii_lowercase();

        if !VALID_VIDEO_QUALITIES.contains(&self.video_quality.to_ascii_lowercase().as_str()) {
            return Err(Error::usage(format!(
                "video_quality must be one of ['1080p', '1440p', '480p', '4k', '720p', 'best'], got '{}'",
                self.video_quality
            )));
        }
        self.video_quality = self.video_quality.to_ascii_lowercase();

        for (name, value) in [
            ("bin_dir", &self.bin_dir),
            ("adb_music_dir", &self.adb_music_dir),
            ("adb_download_dir", &self.adb_download_dir),
        ] {
            if value.trim().is_empty() {
                return Err(Error::usage(format!(
                    "{name} must be a non-empty path string"
                )));
            }
        }

        if let Some(limit) = self.max_download_limit.clone() {
            if matches!(
                limit.trim().to_ascii_lowercase().as_str(),
                "none" | "null" | "off" | "0" | ""
            ) {
                self.max_download_limit = None;
            } else {
                parse_rate(limit.as_str())?;
            }
        }

        if self.history_limit < 1 {
            return Err(Error::usage(format!(
                "history_limit must be a positive integer, got {}",
                self.history_limit
            )));
        }

        if let Some(anti) = self.anti_throttle.clone() {
            if matches!(
                anti.trim().to_ascii_lowercase().as_str(),
                "none" | "null" | "off" | "0" | "false" | ""
            ) {
                self.anti_throttle = None;
            } else {
                parse_rate(anti.as_str())?;
            }
        }

        if self.anti_throttle_grace < 0 {
            return Err(Error::usage(format!(
                "anti_throttle_grace cannot be negative, got {}",
                self.anti_throttle_grace
            )));
        }
        self.anti_throttle_grace = self.anti_throttle_grace.max(3);

        if !VALID_WATCHDOG_MODES.contains(&self.watchdog.to_ascii_lowercase().as_str()) {
            return Err(Error::usage(format!(
                "watchdog must be one of ['off', 'v1', 'v2'], got '{}'",
                self.watchdog
            )));
        }
        self.watchdog = self.watchdog.to_ascii_lowercase();

        if let Some(client) = self.ytdlp_player_client.clone()
            && matches!(
                client.trim().to_ascii_lowercase().as_str(),
                "none" | "null" | "default" | ""
            )
        {
            self.ytdlp_player_client = None;
        }

        if self.stall_timeout < 0.0 {
            return Err(Error::usage(format!(
                "stall_timeout cannot be negative, got {}",
                py_float_str(self.stall_timeout)
            )));
        }

        if self.isp.trim().is_empty() {
            return Err(Error::usage("isp must be a non-empty string"));
        }
        self.isp = self.isp.trim().to_owned();

        if let Some(endpoint) = self.doh_endpoint.clone() {
            let endpoint = endpoint.trim();
            if endpoint.is_empty() {
                return Err(Error::usage("doh_endpoint must be a URL string or null"));
            }
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                return Err(Error::usage(format!(
                    "doh_endpoint must be an http/https URL, got '{endpoint}'"
                )));
            }
            self.doh_endpoint = Some(endpoint.to_owned());
        }

        if !VALID_ENGINES.contains(&self.engine.trim().to_ascii_lowercase().as_str()) {
            return Err(Error::usage(format!(
                "engine must be one of ['aria2', 'rust'], got '{}'",
                self.engine
            )));
        }
        self.engine = self.engine.trim().to_ascii_lowercase();
        Ok(())
    }

    /// Return the configured ISP, or the `"auto"` sentinel when dynamic
    /// detection is requested (detection itself is wired by `paradl-net`).
    pub fn get_effective_isp(&self) -> String {
        if !self.isp.trim().eq_ignore_ascii_case("auto") {
            self.isp.trim().to_owned()
        } else {
            "auto".to_owned()
        }
    }

    /// Set one configuration key with Python-style coercion, validation, and
    /// atomic save. Returns `(success, message)`.
    pub fn set(&mut self, key: &str, value: &str) -> (bool, String) {
        if !KNOWN_FIELDS.contains(&key) || key == "schema_version" {
            return (false, format!("Unknown configuration key: '{key}'"));
        }

        let coerced = match self.coerce_value(key, value) {
            Ok(v) => v,
            Err(message) => return (false, message),
        };

        let mut object = match serde_json::to_value(&*self) {
            Ok(Value::Object(map)) => map,
            _ => return (false, "Could not serialize configuration".to_owned()),
        };
        object.insert(key.to_owned(), coerced.clone());
        let candidate: PDLConfig = match serde_json::from_value(Value::Object(object)) {
            Ok(cfg) => cfg,
            Err(err) => return (false, err.to_string()),
        };

        let mut candidate = candidate;
        if let Err(err) = candidate.validate() {
            return (false, err.to_string());
        }
        // Mirror `set_active_engine` from the Python `set()` coercion path: the
        // process-wide engine must follow a committed config change.
        let previous_engine = get_active_engine();
        if let Err(err) = set_active_engine(&candidate.engine) {
            return (false, err.to_string());
        }
        if let Err(err) = candidate.save() {
            // Roll back the global engine when the config cannot be committed.
            let _ = set_active_engine(&previous_engine);
            return (false, err.to_string());
        }
        *self = candidate;
        (
            true,
            format!("Updated {key} -> {}", py_json_display(&coerced)),
        )
    }

    fn coerce_value(&self, key: &str, value: &str) -> Result<Value, String> {
        let trimmed = value.trim();
        if BOOLEAN_FIELDS.contains(&key) {
            return match trimmed.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Ok(Value::Bool(true)),
                "false" | "0" | "no" | "off" => Ok(Value::Bool(false)),
                _ => Err(format!(
                    "Invalid boolean value for {key}: '{value}' (expected true/false)"
                )),
            };
        }
        if INTEGER_FIELDS.contains(&key) {
            return trimmed
                .parse::<i64>()
                .map(|n| json!(n))
                .map_err(|_| format!("Invalid integer value for {key}: '{value}'"));
        }
        if key == "stall_timeout" {
            return trimmed
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .ok_or_else(|| format!("Invalid numeric value for {key}: '{value}'"));
        }
        if key == "engine" {
            return Ok(Value::String(trimmed.to_ascii_lowercase()));
        }
        if OPTIONAL_STRING_FIELDS.contains(&key) && trimmed.eq_ignore_ascii_case("none") {
            return Ok(Value::Null);
        }
        Ok(Value::String(trimmed.to_owned()))
    }

    /// Atomically save the configuration to `PARADL_HOME/config.json`.
    pub fn save(&self) -> Result<(), Error> {
        let mut config = self.clone();
        config.schema_version = CONFIG_SCHEMA_VERSION;
        config.validate()?;

        let home = get_paradl_home();
        fs::create_dir_all(&home)?;
        let target = config_file();
        let pretty = serde_json::to_string_pretty(&config)
            .map_err(|err| Error::failure(format!("Could not serialize config: {err}")))?;

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let tmp = home.join(format!(
            ".config_{}_{}.tmp",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        {
            let mut file = File::create(&tmp)?;
            file.write_all(pretty.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        if let Err(err) = fs::rename(&tmp, &target) {
            let _ = fs::remove_file(&tmp);
            return Err(Error::Io(err));
        }
        Ok(())
    }

    /// Load configuration from `PARADL_HOME/config.json`.
    ///
    /// Missing, corrupt, or invalid files fall back to defaults; a corrupt file
    /// is copied to `config.json.bad` before returning (mirroring Python).
    pub fn load() -> Result<Self, Error> {
        let file = config_file();
        if !file.exists() {
            return Ok(Self::default());
        }
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(_) => {
                backup_corrupt(&file);
                return Ok(Self::default());
            }
        };
        let parsed: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => {
                backup_corrupt(&file);
                return Ok(Self::default());
            }
        };
        let Some(object) = parsed.as_object() else {
            backup_corrupt(&file);
            return Ok(Self::default());
        };
        let filtered: Map<String, Value> = object
            .iter()
            .filter(|(key, _)| KNOWN_FIELDS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut value = Value::Object(filtered);
        value["schema_version"] = json!(CONFIG_SCHEMA_VERSION);
        if value.get("isp").and_then(Value::as_str) == Some("Duronto Internet") {
            value["isp"] = json!("auto");
        }
        let mut config: PDLConfig = match serde_json::from_value(value) {
            Ok(config) => config,
            Err(_) => {
                backup_corrupt(&file);
                return Ok(Self::default());
            }
        };
        if config.validate().is_err() {
            backup_corrupt(&file);
            return Ok(Self::default());
        }
        // Python `load()` sets the process-wide engine after a successful
        // validate; do the same before returning the loaded config.
        if let Err(err) = set_active_engine(&config.engine) {
            backup_corrupt(&file);
            return Err(err);
        }
        Ok(config)
    }
}

fn py_float_str(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn py_json_display(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

fn backup_corrupt(file: &Path) {
    let backup = PathBuf::from(format!("{}.bad", file.to_string_lossy()));
    let _ = fs::copy(file, backup);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_engine_round_trip() {
        set_active_engine("ARIA2").unwrap();
        assert_eq!(get_active_engine(), "aria2");
        set_active_engine("rust").unwrap();
        assert_eq!(get_active_engine(), "rust");
        assert!(set_active_engine("bogus").is_err());
    }

    #[test]
    fn known_fields_cover_every_struct_field() {
        let serialized = serde_json::to_value(PDLConfig::default()).unwrap();
        let map = serialized.as_object().unwrap();
        assert_eq!(map.len(), KNOWN_FIELDS.len());
        for key in map.keys() {
            assert!(KNOWN_FIELDS.contains(&key.as_str()), "missing {key}");
        }
    }
}
