//! Parse a command line, classify it, and run it through the no-op dispatch seam.
//!
//! ```bash
//! cargo run -p paradl-cli --example parse_and_dispatch -- \
//!     doctor --json --engine rust
//! ```

use paradl_cli::{Cli, Command, Dispatch, NoopDispatch, ParseOutcome};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match Cli::parse_command_line(argv.clone()) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("usage error ({:?}): {}", err.exit_code(), err);
            std::process::exit(err.exit_code().as_i32());
        }
    };

    match &outcome {
        ParseOutcome::Help => print!("{}", paradl_cli::HELP_TEXT),
        ParseOutcome::ThrottleHelp => print!("{}", paradl_cli::THROTTLE_HELP_TEXT),
        ParseOutcome::Version => print!("{}", paradl_cli::VERSION_TEXT),
        ParseOutcome::Throttle(options) => {
            println!("argv    : {argv:?}");
            println!("throttle: {options:?}");
            println!("exit    : 0");
        }
        ParseOutcome::Command(cli) => {
            match Command::from_cli(cli) {
                Ok(command) => {
                    println!("argv    : {argv:?}");
                    println!("command : {command:?}");
                    let code = NoopDispatch.handle(&command, cli);
                    println!("exit    : {}", code.as_i32());
                }
                Err(err) => {
                    eprintln!("usage error ({}): {}", err.exit_code().as_i32(), err);
                    std::process::exit(err.exit_code().as_i32());
                }
            }
        }
    }
}
