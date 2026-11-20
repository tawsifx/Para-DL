//! Drive the watchdog with an injected clock and print byte-exact events.
//!
//! Run: `cargo run -p paradl-watchdog --example basic`

use std::sync::Arc;

use paradl_core::{FakeClock, WatchdogMode};
use paradl_watchdog::{
    ThroughputSample, Watchdog, WatchdogActionType, WatchdogConfig, WatchdogController,
};

fn main() {
    let clock = Arc::new(FakeClock::new(1_763_376_000.123, 1000.0));
    let mut wd = WatchdogController::with_clock(
        WatchdogConfig {
            mode: WatchdogMode::V2,
            user_floor_bps: 1000.0,
            recycle_interval_sec: 0.0,
            ..WatchdogConfig::default()
        },
        clock,
    );

    // Warm up above the floor so the adaptive p90 threshold is established.
    for t in 0..5 {
        // Invoke through the charter `Watchdog` trait explicitly.
        Watchdog::observe(&mut wd, ThroughputSample::new(t as f64, 5000.0));
    }

    // Five low-speed ticks: the fifth triggers a tier-2 socket recycle.
    for i in 0..5 {
        let action = wd.observe(ThroughputSample::new(10.0 + i as f64, 500.0));
        println!("tick {i}: {}", action.action);
        if action.action == WatchdogActionType::PauseUnpause {
            print!("{}", action.to_event().to_jsonl_line());
        }
    }

    println!("state: {}", wd.state().status_label);
    println!("cycles: {}", wd.state().cycle_count);
}
