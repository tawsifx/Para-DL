//! Ledgered cross-crate handoffs wired at integration time (W4-1).
//!
//! The framework crates were built in disjoint waves and deliberately never
//! depend on each other across a layering boundary. This module is where the
//! ledger's four deferred handoffs are joined:
//!
//! 1. **ISP auto-detection (W0-3 → W1-4).** [`paradl_core::PDLConfig`] returns
//!    the literal `"auto"` sentinel; [`paradl_net::resolve_effective_isp`]
//!    performs the detection. [`resolve_isp`] composes the two without editing
//!    `paradl-core` (ledger line 161).
//! 2. **Watchdog hour bucket (W1-3).** `WatchdogConfig::default()` uses the
//!    `-1` "unknown" sentinel; [`watchdog_config_for`] always supplies
//!    `Some(compute_hour_bucket(local_hour))` so learned profiles are keyed on
//!    a real 4-hour bucket (ledger line 154).
//! 3. **API resolve failure (W3-3).** `paradl-api` maps `ExitCode::Failure` to
//!    HTTP 500, but the Python oracle answers 400 for an unresolvable target.
//!    [`normalise_resolve_failure`] lets the app executor turn any resolution
//!    failure into a usage error so `ApiError::from_core` produces 400.
//! 4. **Media handshake / error mapping (W3-4b).** [`map_media_error_kind`]
//!    reuses the single `paradl-core` model instead of the duplicate helpers in
//!    `paradl-media`, and never produces exit code 3.

use paradl_core::{Error, ExitCode, PDLConfig, WatchdogMode, parse_rate};
use paradl_engine::clamp_connections;
use paradl_net::resolve_effective_isp;
use paradl_watchdog::{WatchdogConfig, compute_hour_bucket};

/// Resolve the configured ISP, detecting it when the config asks for `"auto"`.
///
/// This is the W4-1 integration of `paradl-core`'s documented sentinel with
/// `paradl-net`'s detector. Explicit names pass through unchanged (trimmed).
///
/// ```
/// use paradl_app::handoffs::resolve_isp;
/// use paradl_core::PDLConfig;
/// let config = PDLConfig { isp: "Comcast Cable".into(), ..PDLConfig::default() };
/// assert_eq!(resolve_isp(&config), "Comcast Cable");
/// ```
pub fn resolve_isp(config: &PDLConfig) -> String {
    resolve_effective_isp(&config.get_effective_isp())
}

/// Build a watchdog configuration with a real local-hour bucket.
///
/// `local_hour` is the caller's local hour of day (`0..=23`). It is reduced to
/// the oracle's 4-hour bucket and installed as `Some(..)`, never leaving the
/// `-1` unknown sentinel in the resulting state. Requested connection counts
/// are clamped through the engine's `min(max(n,1),16)` contract.
///
/// ```
/// use paradl_app::handoffs::watchdog_config_for;
/// use paradl_core::PDLConfig;
/// let config = watchdog_config_for(&PDLConfig::default(), 14, "example.com", 99, 0.0, true);
/// assert_eq!(config.hour_bucket, Some(3));
/// assert_eq!(config.connections, 16);
/// ```
pub fn watchdog_config_for(
    config: &PDLConfig,
    local_hour: i32,
    host: &str,
    requested_connections: i32,
    floor_bps: f64,
    supports_range: bool,
) -> WatchdogConfig {
    let mode = match config.watchdog.trim().to_ascii_lowercase().as_str() {
        "v1" => WatchdogMode::V1,
        "off" => WatchdogMode::Off,
        _ => WatchdogMode::V2,
    };
    let configured_floor = config
        .anti_throttle
        .as_deref()
        .and_then(|rate| parse_rate(rate).ok())
        .unwrap_or(floor_bps);
    WatchdogConfig {
        mode,
        user_floor_bps: configured_floor.max(floor_bps),
        grace_period: config.anti_throttle_grace.max(2),
        connections: clamp_connections(requested_connections.max(0) as u32) as i32,
        host: host.to_owned(),
        supports_range,
        hour_bucket: Some(compute_hour_bucket(local_hour)),
        ..WatchdogConfig::default()
    }
}

/// Normalise a target-resolution failure into a client-side usage error.
///
/// The Python `POST /resolve` endpoint answers HTTP 400 for a bad target.
/// `paradl-api`'s `ApiError::from_core` maps [`ExitCode::Failure`] to 500, so an
/// app-owned executor must convert resolution failures to [`Error::usage`]
/// before handing them to the API layer. This preserves the worker's human
/// message while producing the byte-exact HTTP status.
///
/// ```
/// use paradl_app::handoffs::{api_resolve_failure_status, normalise_resolve_failure};
/// use paradl_core::Error;
/// let normalised = normalise_resolve_failure(&Error::failure("no such target"));
/// assert_eq!(normalised.exit_code(), paradl_core::ExitCode::Usage);
/// assert_eq!(api_resolve_failure_status(&Error::failure("no such target")), 400);
/// ```
pub fn normalise_resolve_failure(error: &Error) -> Error {
    Error::usage(error.to_string())
}

/// HTTP status produced by the API layer for a normalised resolve failure.
///
/// Always `400` (client error); a resolution failure is never an internal
/// server error in the oracle.
pub fn api_resolve_failure_status(error: &Error) -> u16 {
    match normalise_resolve_failure(error).exit_code() {
        ExitCode::Usage | ExitCode::VerificationFailed | ExitCode::MissingDependency => 400,
        _ => 500,
    }
}

/// Map a media worker `error.kind` to the single framework error model.
///
/// Reuses [`Error::from_media_kind`] rather than the protocol crate's duplicate
/// helper so there is exactly one exit-code mapping in the process. The worker
/// never emits `3`, and an unknown kind fails closed to [`ExitCode::Failure`].
///
/// ```
/// use paradl_app::handoffs::map_media_error_kind;
/// use paradl_core::ExitCode;
/// assert_eq!(
///     map_media_error_kind("dependency", "yt-dlp missing").exit_code(),
///     ExitCode::MissingDependency,
/// );
/// ```
pub fn map_media_error_kind(kind: &str, message: impl Into<String>) -> Error {
    Error::from_media_kind(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_isp_does_not_consult_the_detector() {
        let config = PDLConfig {
            isp: "  Telco  ".into(),
            ..PDLConfig::default()
        };
        assert_eq!(resolve_isp(&config), "Telco");
    }

    #[test]
    fn every_hour_maps_to_a_real_bucket() {
        for hour in 0..24 {
            let config = watchdog_config_for(&PDLConfig::default(), hour, "h", 4, 0.0, true);
            assert!(config.hour_bucket.is_some_and(|bucket| (0..=5).contains(&bucket)));
        }
    }
}
