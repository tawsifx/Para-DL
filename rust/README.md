# paraDL Rust framework

A Cargo workspace of public, semver-`1.0.0` Rust crates, a thin native
`paradl` binary, and an **optional** Python media worker. All non-media paths
are pure Rust: `paradl --version`, `paradl doctor`, downloads, history, queue,
throttle diagnostics, hashing and extraction need **no Python interpreter**.

Media extraction is the one exception: `media/` is a standalone Python package
invoked as `python -m paradl_media` over the versioned `paradl-media/1`
JSON-lines protocol. It is never linked into the binary (no PyO3, no sidecar).

## Quickstart: CLI

```sh
cd rust
cargo build --release -p paradl      # -> rust/target/release/paradl

./target/release/paradl --version    # parallel-dl (paradl) v1.0.0
./target/release/paradl --help
./target/release/paradl doctor       # deterministic, Python-free diagnostics
./target/release/paradl doctor --json
./target/release/paradl https://example.com/file.zip -o ./downloads
```

The binary also answers to `parallel-dl` (same command, installed as a second
name by the Formula / install scripts). Optional Cargo features are
`media`, `api`, `tui`, `platform-macos`, `platform-windows`; the default build
needs no optional system tools:

```sh
cargo build --release -p paradl --features media,tui
```

Exit codes: `0` success, `1` failure, `2` usage, `3` verification failure,
`4` missing dependency (including media handshake failures), `130`
interruption — all owned by `paradl_core::Error` / `ExitCode`.

## Quickstart: library use

Every crate is published with crate-level docs, a `README.md`, and a runnable
example. Add the crates you need from crates.io (or by path in this workspace):

```toml
[dependencies]
paradl-core = "1.0"
paradl-app = "1.0"
```

Byte-exact helpers and the single error/exit-code model (`paradl-core`):

```rust
use paradl_core::{Error, ExitCode, parse_rate, sanitize_filename};

assert_eq!(parse_rate("1M").unwrap(), 1024.0 * 1024.0);
assert_eq!(sanitize_filename("CON"), "_CON");
assert_eq!(Error::usage("bad flag").exit_code(), ExitCode::Usage);
```

Orchestrating a command through the public app seam (`paradl-app`):

```rust
use paradl_app::{App, DoctorReport};
use paradl_cli::{Cli, Command, Dispatch, ParseOutcome};

let report = DoctorReport::collect(false);
assert!(!report.python_probed); // non-media diagnostics never probe Python
let ParseOutcome::Command(cli) = Cli::parse_command_line(vec!["doctor".into()]).unwrap() else {
    unreachable!()
};
let command = Command::from_cli(&cli).unwrap();
assert_eq!(App::new_offline().handle(&command, &cli), paradl_cli::ExitCode::Ok);
```

Run the crate tours to see more of the public API:

```sh
cargo run -p paradl-core --example basics
cargo run -p paradl-ui --example plain_progress
cargo run -p paradl-ui --example tui_preview --features paradl-ui/tui
```

## Optional media setup

Media routes are off by default. To enable them, build the binary with the
`media` feature and install the worker into a dedicated venv, then point
`PARADL_MEDIA_PY` at that venv's Python:

```sh
# 1. Build with media support
cd rust && cargo build --release -p paradl --features media && cd ..

# 2. Create a media-only venv and install the worker + its dependencies
uv venv --python 3.12 .venv-media
uv pip install --python .venv-media/bin/python "yt-dlp[default]>=2026.6.9" "mutagen>=1.47"
uv pip install --python .venv-media/bin/python -e ./media
# optional JS-challenge support:
uv pip install --python .venv-media/bin/python yt-dlp-ejs

# 3. Tell the native binary which Python to spawn for media routes
export PARADL_MEDIA_PY="$PWD/.venv-media/bin/python"
```

`PARADL_MEDIA_PY` is the only bridge: the binary spawns
`$PARADL_MEDIA_PY -m paradl_media` per media operation. If it is unset or the
worker is missing/incompatible, media routes fail closed with exit `4` and a
clear diagnostic; non-media routes keep working. Handshake rules for
`paradl-media/1` (N, N±1, unknown major, missing version) are covered by the
boundary lane and protocol contract tests.

The install scripts (`scripts/install-local.sh` / `.ps1`) and the Homebrew
Formula automate exactly this layout (native binary in `bin/`, media venv in
`libexec/`). `PARADL_SKIP_MEDIA=1` performs a non-media-only install.

## Workspace layout

| Path | Role |
|---|---|
| `crates/paradl-core` | models, helpers, config, paths, clock, `Error`→`ExitCode` |
| `crates/paradl-store` | `history.db` v4, WAL, device+inode guard |
| `crates/paradl-net` / `paradl-engine` | HTTP/DNS/routing, site resolvers, transfer engine |
| `crates/paradl-watchdog` / `paradl-throttle` | anti-throttle state machine, classifier/reports |
| `crates/paradl-postproc` / `paradl-platform` | checksums/archive/ADB, OS metadata/keychain |
| `crates/paradl-resilience` / `paradl-events` | queue/hooks/auth, the sole JSONL emitter |
| `crates/paradl-ui` / `paradl-api` | progress/TUI, optional axum server |
| `crates/paradl-media` | Rust protocol client only — no extractor logic |
| `crates/paradl-cli` / `paradl-app` / `paradl` | parser, orchestration, thin binary |
| `crates/paradl-oracle` / `paradl-parity` | dev-only golden fixtures and 3-lane harness |

## Build / test

```sh
cd rust
PYO3_PYTHON="$PWD/../.venv/bin/python" cargo test --all
PYO3_PYTHON="$PWD/../.venv/bin/python" cargo clippy --all -- -D warnings
cargo doc --no-deps && cargo test --doc
cargo build --all --all-features
cargo build --all --no-default-features
```

MSRV: Rust 1.98 (edition 2024). On this host, legacy `paradl-rs` needs
`PYO3_PYTHON` pointed at a Python 3.12 interpreter; the new framework crates do
not depend on PyO3.

See the repository [CHANGELOG.md](../CHANGELOG.md) and
`docs/superpowers/framework-charter.md` for the release and architecture policy.
