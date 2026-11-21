//! Wire the full framework in-process and dispatch a command without a TTY.
//!
//! ```bash
//! cargo run -p paradl-app --example wire_all
//! ```

use paradl_app::{App, Capture};
use paradl_cli::{Cli, Command, Dispatch, ParseOutcome};

fn main() {
    let cli = match Cli::parse_command_line(vec!["doctor".into(), "--json".into()]) {
        Ok(ParseOutcome::Command(cli)) => cli,
        other => panic!("unexpected parse outcome: {other:?}"),
    };
    let command = Command::from_cli(&cli).expect("doctor needs no arguments");
    let capture = Capture::default();
    let mut app = App::new_offline().with_output(capture.sink());
    let code = app.handle(&command, &cli);
    println!("exit={code}");
    print!("{}", capture.contents());
}
