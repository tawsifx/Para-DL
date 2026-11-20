//! Thin reference binary for the paraDL Rust framework.
//!
//! This binary owns the `paradl` / `parallel-dl` names. It is deliberately
//! small: [`paradl_cli`] provides the byte-exact `--help`, `--version` and
//! argparse error text, and [`paradl_app::App`] executes every subcommand by
//! delegating to the framework crates. Media stays a Python one-shot worker
//! (`python -m paradl_media`, `paradl-media/1`) behind the `media` feature;
//! no Python is spawned on any non-media path.
#![warn(missing_docs)]

use paradl_app::App;
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome, ThrottleCommand};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match Cli::parse_command_line(argv) {
        Ok(outcome) => outcome,
        Err(error) => exit_with_error(&error),
    };

    match outcome {
        ParseOutcome::Help => print!("{}", paradl_cli::HELP_TEXT),
        ParseOutcome::ThrottleHelp => print!("{}", paradl_cli::THROTTLE_HELP_TEXT),
        ParseOutcome::Version => print!("{}", paradl_cli::VERSION_TEXT),
        ParseOutcome::Throttle(options) => {
            // The standalone throttle parser owns these flags (dispatch.py fast
            // path). Rebuild the flat namespace so the same App handles it.
            let cli = empty_cli();
            let command = Command::Throttle(ThrottleCommand { options });
            let code = App::new().handle(&command, &cli);
            if code != ExitCode::Ok {
                std::process::exit(code.as_i32());
            }
        }
        ParseOutcome::Command(cli) => {
            let command = match Command::from_cli(&cli) {
                Ok(command) => command,
                Err(error) => exit_with_error(&error),
            };
            let code = App::new().handle(&command, &cli);
            if code != ExitCode::Ok {
                std::process::exit(code.as_i32());
            }
        }
    }
}

/// Print an argparse-exact error to stderr and exit with its mapped code.
fn exit_with_error(error: &paradl_cli::Error) -> ! {
    eprintln!("{error}");
    std::process::exit(error.exit_code().as_i32());
}

/// The default flat namespace used for the standalone throttle fast path.
fn empty_cli() -> Cli {
    match Cli::parse_command_line(Vec::new()) {
        Ok(ParseOutcome::Command(cli)) => cli,
        other => unreachable!("an empty argv always parses to a Cli: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_byte_exact_oracle() {
        assert_eq!(paradl_cli::VERSION_TEXT, "parallel-dl (paradl) v1.0.0\n");
        assert_eq!(
            Cli::parse_command_line(vec!["--version".to_string()]).unwrap(),
            ParseOutcome::Version,
        );
    }

    #[test]
    fn help_is_byte_exact_oracle() {
        let oracle = include_str!("../../paradl-cli/tests/fixtures/help.txt");
        assert_eq!(paradl_cli::HELP_TEXT, oracle);
        assert_eq!(Cli::render_help(), oracle);
    }

    #[test]
    fn throttle_help_is_byte_exact_oracle() {
        let oracle = include_str!("../../paradl-cli/tests/fixtures/throttle_help.txt");
        assert_eq!(paradl_cli::THROTTLE_HELP_TEXT, oracle);
    }

    #[test]
    fn empty_cli_carries_oracle_defaults() {
        let cli = empty_cli();
        assert_eq!(cli.connections, 16);
        assert_eq!(cli.engine, paradl_cli::Engine::Rust);
    }
}
