//! Default-build example: render typed progress events without a TTY.
//!
//! Run with:
//! ```sh
//! cargo run -p paradl-ui --example plain_progress
//! ```

use paradl_events::{Event, ProgressEvent};
use paradl_ui::{ProgressDisplay, event_summary};

fn main() {
    let display = ProgressDisplay::new();
    let ticks = [
        ProgressEvent::new(
            1.0,
            "sample.bin".into(),
            256,
            Some(1024),
            Some(25.0),
            Some(512.0),
            Some(1.5),
            Some(4),
            None,
            Some("Active".into()),
            None,
            None,
        ),
        ProgressEvent::new(
            2.0,
            "sample.bin".into(),
            1024,
            Some(1024),
            Some(100.0),
            Some(0.0),
            Some(0.0),
            Some(4),
            None,
            Some("Active".into()),
            None,
            None,
        ),
    ];

    for tick in &ticks {
        println!("simple:    {}", display.simple_line(tick));
        println!("renderable:{}", display.renderable(tick));
        println!(
            "summary:   {}",
            event_summary(&Event::Progress(tick.clone()))
        );
    }
}
