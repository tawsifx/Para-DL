//! RED-first tests for the four ledgered W4-1 handoffs.
//!
//! (a) `paradl-core::get_effective_isp()`'s `"auto"` sentinel is resolved by
//!     `paradl-net::resolve_effective_isp()`.
//! (b) watchdog config receives a real `compute_hour_bucket(local_hour)`, not
//!     the `-1` unknown sentinel.
//! (c) API resolve failures are a client error (400), never a 500.
//! (d) media handshake/error mapping reuses the single `paradl-core` model and
//!     never emits exit code 3.

use std::sync::Arc;

use paradl_app::handoffs::{
    api_resolve_failure_status, map_media_error_kind, resolve_isp, watchdog_config_for,
};
use paradl_core::{Error, ExitCode, FakeClock, PDLConfig};
use paradl_media::check_protocol;
use paradl_watchdog::{WatchdogController, compute_hour_bucket};

#[test]
fn handoff_a_auto_isp_uses_paradl_net_detector() {
    paradl_net::set_isp_detector(Some(|| Some("Detected Fiber ISP".to_string())));

    let config = PDLConfig {
        isp: "auto".to_string(),
        ..PDLConfig::default()
    };
    assert_eq!(config.get_effective_isp(), "auto", "core returns the sentinel");
    assert_eq!(resolve_isp(&config), "Detected Fiber ISP");

    let config = PDLConfig {
        isp: "  Comcast Cable  ".to_string(),
        ..PDLConfig::default()
    };
    assert_eq!(config.get_effective_isp(), "Comcast Cable");
    assert_eq!(resolve_isp(&config), "Comcast Cable");

    paradl_net::reset_isp_detector();
}

#[test]
fn handoff_b_watchdog_receives_real_hour_bucket() {
    let config = watchdog_config_for(&PDLConfig::default(), 14, "example.com", 99, 1_000.0, true);
    assert_eq!(config.hour_bucket, Some(compute_hour_bucket(14)));
    assert_eq!(config.hour_bucket, Some(3));
    // Clamping still comes from the engine contract, not from this handoff.
    assert_eq!(config.connections, 16);
    let watchdog = WatchdogController::with_clock(config, Arc::new(FakeClock::default()));
    assert_eq!(watchdog.state().hour_bucket, 3, "no -1 sentinel in the app wiring");
}

#[test]
fn handoff_c_api_resolve_failure_is_400_not_500() {
    assert_eq!(api_resolve_failure_status(&Error::failure("resolver exploded")), 400);
    assert_eq!(api_resolve_failure_status(&Error::usage("bad target")), 400);
}

#[test]
fn handoff_d_media_error_and_handshake_mapping_never_yields_three() {
    let expectations = [
        ("metadata", ExitCode::Failure),
        ("download", ExitCode::Failure),
        ("conversion", ExitCode::Failure),
        ("dependency", ExitCode::MissingDependency),
        ("usage", ExitCode::Usage),
        ("interrupted", ExitCode::Interrupted),
        ("totally-unknown", ExitCode::Failure),
    ];
    for (kind, expected) in expectations {
        let err = map_media_error_kind(kind, "worker message");
        assert_eq!(err.exit_code(), expected, "kind={kind}");
        assert_ne!(err.exit_code(), ExitCode::VerificationFailed, "worker never emits 3");
    }

    // Unknown/missing protocol major fails closed with the missing-dependency code.
    for bad in [Some("paradl-media/2"), None, Some("garbage")] {
        let error = map_media_error_kind("dependency", "unsupported protocol version");
        assert_eq!(error.exit_code(), ExitCode::MissingDependency);
        assert!(check_protocol(bad).is_err(), "bad protocol: {bad:?}");
    }
    assert!(check_protocol(Some("paradl-media/1")).is_ok());
    assert!(check_protocol(Some("paradl-media/1.1")).is_ok(), "minor additions tolerated");
}
