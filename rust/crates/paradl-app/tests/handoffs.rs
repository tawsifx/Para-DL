//! RED-first tests for the four ledgered W4-1 handoffs.
//!
//! (a) `paradl-core::get_effective_isp()`'s `"auto"` sentinel is resolved by
//!     `paradl-net::resolve_effective_isp()`.
//! (b) watchdog config receives a real `compute_hour_bucket(local_hour)`, not
//!     the `-1` unknown sentinel.
//! (c) API resolve failures are a client error (400), never a 500.
//! (d) media handshake/error mapping reuses the single `paradl-core` model and
//!     never emits exit code 3.

use std::sync::{Arc, Mutex};

use paradl_app::handoffs::{
    api_resolve_failure_status, map_media_error_kind, resolve_isp, watchdog_config_for,
};
use paradl_app::{App, Capture};
use paradl_cli::{Cli, Command, Dispatch, ParseOutcome};
use paradl_core::{Error, ExitCode, FakeClock, PDLConfig};
use paradl_media::check_protocol;
use paradl_watchdog::{WatchdogController, compute_hour_bucket};

/// Serialises tests that mutate the process-wide ISP detector.
static ISP_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn handoff_a_auto_isp_uses_paradl_net_detector() {
    let _guard = ISP_LOCK.lock().unwrap();
    paradl_net::reset_isp_detector(); // clears the 5-minute success cache
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

/// Dispatch-level proof: the resolved ISP and real hour bucket are actually
/// used by production transfer/info paths, not just returned by helpers.
#[test]
fn dispatch_level_handoff_effects_are_observable() {
    let _guard = ISP_LOCK.lock().unwrap();
    paradl_net::reset_isp_detector(); // clears any cached real-ISP detection
    paradl_net::set_isp_detector(Some(|| Some("Detected Fiber ISP".to_string())));
    let config = PDLConfig {
        isp: "auto".to_string(),
        ..PDLConfig::default()
    };

    // (a) + (b): a dispatched download records the detected ISP, a real
    // 4-hour bucket and the ISP as the learned-profile network id.
    let cli = Cli::parse_command_line(vec![
        "https://example.com/file.bin".to_string(),
        "--json".to_string(),
    ])
    .unwrap();
    let ParseOutcome::Command(cli) = cli else { panic!() };
    let command = Command::from_cli(&cli).unwrap();
    let mut app = App::offline_with_config(config.clone());
    assert_eq!(app.handle(&command, &cli), paradl_cli::ExitCode::Ok);
    let setup = app.last_transfer().expect("transfer setup recorded");
    assert_eq!(setup.isp, "Detected Fiber ISP");
    assert_eq!(setup.network_id, "Detected Fiber ISP");
    assert!((0..=5).contains(&setup.hour_bucket), "bucket={}", setup.hour_bucket);
    assert_ne!(setup.hour_bucket, -1, "the -1 unknown sentinel must never appear");
    assert_eq!(setup.hour_bucket, compute_hour_bucket(setup.local_hour));

    // (a) on the info path: the detected ISP appears in the emitted event.
    let cli = Cli::parse_command_line(vec!["info".to_string(), "https://example.com/f.zip".to_string(), "--json".to_string()]).unwrap();
    let ParseOutcome::Command(cli) = cli else { panic!() };
    let command = Command::from_cli(&cli).unwrap();
    let capture = Capture::default();
    let mut app = App::offline_with_config(config).with_output(capture.sink());
    assert_eq!(app.handle(&command, &cli), paradl_cli::ExitCode::Ok);
    assert!(
        capture.contents().contains("\"isp\": \"Detected Fiber ISP\""),
        "{}",
        capture.contents()
    );
    paradl_net::reset_isp_detector();
}

/// Dispatch-level proof (c): the API bridge used by the app executor maps a
/// failed resolution to the oracle's HTTP 400, never a 500.
#[test]
fn dispatch_level_api_resolve_failure_is_400() {
    let app = App::new_offline();
    let failure = app
        .resolve_for_api("")
        .expect_err("an empty target cannot be resolved");
    assert_eq!(failure.exit_code(), ExitCode::Usage);
    assert_eq!(app.resolve_failure_status(&failure), 400);
    assert_ne!(app.resolve_failure_status(&failure), 500);
    assert_eq!(api_resolve_failure_status(&Error::failure("boom")), 400);

    let resolved = app.resolve_for_api("https://example.com/f.zip").unwrap();
    assert_eq!(resolved.target_type, "direct");
}
