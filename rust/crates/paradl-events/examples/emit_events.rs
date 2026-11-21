//! Demonstrates the single JSONL emitter: typed events, redaction, watchdog
//! consumption, and foreign-event adoption. Run with:
//!
//! ```bash
//! cargo run -p paradl-events --example emit_events
//! ```

use paradl_events::{
    AdoptedEvent, Event, HookExecutedEvent, ProgressEvent, WatchdogAction, WatchdogActionType,
    emit_jsonl, now_timestamp, register_secret,
};
use serde::Serialize;

#[derive(Serialize)]
struct ThrottleTyped {
    label: &'static str,
}

fn main() {
    // 1. A locally defined typed event. The binary/API writes this line to stdout.
    let progress = ProgressEvent::new(
        now_timestamp(),
        "demo.bin".into(),
        1024,
        Some(2048),
        Some(50.0),
        Some(1000.0),
        Some(1.0),
        Some(16),
        None,
        Some("v2".into()),
        None,
        None,
    );
    print!("{}", emit_jsonl(&progress));

    // 2. Registered secrets are redacted from every subsequent line.
    register_secret("super-secret-token-1234");
    #[derive(Serialize)]
    struct Detail<'a> {
        event: &'a str,
        message: &'a str,
    }
    print!(
        "{}",
        emit_jsonl(&Detail {
            event: "info",
            message: "using super-secret-token-1234",
        })
    );

    // 3. Consume the watchdog's public typed event through this emitter.
    let action = WatchdogAction::new(WatchdogActionType::PauseUnpause);
    let line = paradl_events::from_watchdog(&action);
    print!("{line}");

    // 4. Adopt a typed event from a crate we must not depend on (W2-1 throttle).
    let adopted = AdoptedEvent::new(
        "throttle_launchd_plist_generated",
        now_timestamp(),
        ThrottleTyped { label: "hourly" },
    );
    print!("{}", emit_jsonl(&adopted));

    // 5. The registry covers all oracle names.
    println!("# {} event shapes, version {}", Event::all().len(), paradl_events::EVENT_SCHEMA_VERSION);

    // 6. A hook event without a newline (for hook stdin JSON).
    let hook = HookExecutedEvent::new(now_timestamp(), "/hooks/ok".into(), true);
    println!("# hook stdin payload: {}", paradl_events::to_json_line(&hook));
}
