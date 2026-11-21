# paradl-cli

**Role:** the clap-based command-line parser and typed dispatch seam for the paraDL
Rust framework — a faithful port of `src/paradl/cli/{parser,main,dispatch}.py` and the
standalone `paradl.throttle.cli` parser. Command *bodies* are intentionally absent;
they are wired by `paradl-app` in W4-1.

## Public surface

- `Cli` — clap-derived namespace for every flag in the parity matrix (~60 main flags,
  `--doh-endpoint`, the retained `--http3-probe` (F59), the target-surface aliases,
  and all throttle flags). At W5-3 the `--http3`/`--no-http3`, `--accept-ranges` and
  `--total` flags and the `--install`/`--info`/`--hash` long forms were retired; the
  corresponding subcommands are canonical.
- `ThrottleCli` / `ThrottleSubcommand` — the standalone `pdl throttle …` parser.
- `ParseOutcome` — `Command`, `Throttle`, `Help`, `ThrottleHelp`, `Version`.
- `Command` and its typed payloads — the ~20 subcommands from parity matrix §1,
  produced by `Command::from_cli` with the oracle's exact `Usage:` error strings and
  `Error::Usage` → `ExitCode::Usage` (`2`).
- `Dispatch` / `dispatch` / `NoopDispatch` — the trait `paradl-app` implements to
  execute those commands.
- `HELP_TEXT`, `THROTTLE_HELP_TEXT`, `VERSION_TEXT` — byte-exact oracle strings,
  captured from `.venv/bin/python` and embedded via `include_str!`.
- `SUBCOMMANDS`, `CLI_ENV_VARS` — the authoritative positional subcommand set and
  the environment variables the Python CLI contract reads.

## Help/usage parity

`Cli::command()` installs the captured oracle text with
`clap::Command::override_help`, so clap never re-wraps or re-orders the help.
`--help`/`-h` and `--version` are handled explicitly by `Cli::parse_command_line`
to preserve the exact trailing bytes (including the oracle's trailing recipe
line). `tests/fixtures/help.txt` and `tests/fixtures/throttle_help.txt` are the
captured goldens asserted by `tests/oracle_parity.rs`.

`argparse` binds no environment variables itself; `CLI_ENV_VARS` documents the
variables the Python entry point reads through `PDLConfig` and the auth layer.
No new flags or env bindings were invented.

Parse failures are rendered into argparse's exact stderr byte stream by
`argparse_error`: the captured usage block followed by
`parallel-dl: error: <argparse message>\n`. Clap's own wording is never passed
through. Dispatch-time `USAGE_*` constants hold the **rendered** (Rich-markup-
stripped) text, captured from piped oracle processes, including the oracle's
trailing spaces and line wraps. `--help`/`--version` are argparse-style immediate
actions: whichever token is encountered first wins, so `--version --engine bogus`
prints the exact version bytes and exits `0`, while `--engine bogus --version`
exits `2`. `--version` is added as a clap `ArgAction::Version` with the oracle's
exact text (`display_name` + `version`); clap's extra `-V` alias is not registered.

## Feature flags

None. This crate depends only on `clap` (derive + env) and `paradl-core`; it must not
depend on PyO3.

## Example

```bash
cargo run -p paradl-cli --example parse_and_dispatch -- doctor --json --engine rust
cargo run -p paradl-cli --example parse_and_dispatch -- install astral-sh/uv
cargo run -p paradl-cli --example parse_and_dispatch -- throttle full --seed 7
```

See [`examples/parse_and_dispatch.rs`](examples/parse_and_dispatch.rs).

## Verification

```bash
cargo test -p paradl-cli
cargo clippy -p paradl-cli -- -D warnings
cargo doc -p paradl-cli --no-deps
```
