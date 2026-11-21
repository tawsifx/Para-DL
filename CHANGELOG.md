# Changelog

All notable changes to paraDL are documented here. This file follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project's
semantic-versioning policy (framework charter §6). The media protocol version
`paradl-media/1` is independent of crate semver.

## [1.0.0] - 2026-11-21

First stable release of the paraDL framework. The Python application has been
replaced by a Cargo workspace of public Rust crates and a thin native binary;
only the optional media subsystem remains Python.

### Added

- **Rust framework** (`rust/crates/*`, 1.0.0): public, documented crates for
  core models/errors (`paradl-core`), SQLite persistence (`paradl-store`),
  HTTP/DNS/routing and site resolvers (`paradl-net`), the in-process download
  engine (`paradl-engine`), the anti-throttle watchdog (`paradl-watchdog`),
  classifier/reports (`paradl-throttle`), checksum/archive/ADB post-processing
  (`paradl-postproc`), byte-exact OS metadata/keychain (`paradl-platform`),
  queue/hooks/resilience/auth (`paradl-resilience`), the sole JSONL emitter
  (`paradl-events`), progress/TUI (`paradl-ui`), the optional axum API
  (`paradl-api`), the CLI parser (`paradl-cli`), and the media protocol client
  (`paradl-media`). `paradl-oracle` and `paradl-parity` remain dev-only.
- **Thin binary** (`paradl`, owning both `paradl` and `parallel-dl` command
  names) with `doctor`, `netprobe`, `bench`, full subcommand dispatch, the
  deterministic fixed-width human `doctor` renderer, and the complete
  `0/1/2/3/4/130` exit-code map owned by Rust.
- **Optional Python media worker** (`media/`, `python -m paradl_media`): a
  one-shot JSON-lines worker over `paradl-media/1` that owns all yt-dlp /
  mutagen / ffmpeg extraction, with no `paradl.*` imports, no console script,
  and no PyO3 or sidecar runtime. Rust owns the protocol client, JSONL
  emission, and exit-code mapping.
- **Release hygiene**: crate versions unified at `1.0.0` via
  `workspace.package.version`; docs.rs metadata (`documentation`, `repository`,
  `homepage`, `keywords`, `categories`) added to published crates; this
  CHANGELOG; `rust/README.md` quickstart covering library use, CLI, and
  optional media setup; refreshed `uv.lock`.
- **Protocol version-compatibility matrix**: `paradl-media/1` verified for
  N (`1`), N−minor (`1.1`), N+minor (`1.999`), unknown majors (`0`, `2`, `99`),
  missing/unparseable versions, and the boundary lane's clean `version-mismatch`
  case (exit `4`).

### Changed

- Workspace version `0.1.0` → `1.0.0`; `Cargo.lock` and `uv.lock` refreshed.
- `doctor` no longer recommends the deleted legacy PyO3 module
  (`maturin develop --release`); its human and JSON fixtures are updated.
- Benchmark harnesses are retired rather than crashing now that the differential
  Python oracle is deleted: each measurement is emitted as
  `{"value": null, "reason": "..."}` and all honesty/drift caveats are kept.

### Removed / retired

- **W5-1 cutover** deleted the non-media Python application (`src/paradl/**`,
  bulk `tests/**`, `parallel-dl.py`). The native binary owns all entry points.
- The `paradl-parity` differential test `full_non_media_gate_is_clean` is
  `#[ignore]`d: it required the deleted Python CLI oracle. Its final GREEN
  evidence (exit `0`, 6/6 cases CLEAN) is preserved at commit `c4e271a` in the
  framework ledger. The boundary lane remains live and oracle-free.

### Migration notes

- Non-media usage needs no Python. Install the native binary via the Formula or
  the install scripts; `paradl --version` and `paradl doctor` work with a
  poisoned Python `PATH`.
- Media routes are optional. Create a media venv with `yt-dlp[default]` and
  `mutagen`, install `media/`, and point `PARADL_MEDIA_PY` at its Python
  interpreter (see `rust/README.md`). Handshake or dependency failures fail
  closed with exit `4`.

### Verification (fresh, release build)

- `cargo build --all`, `cargo test --all`, `cargo clippy --all -- -D warnings`,
  `cargo doc --no-deps`, `cargo test --doc`,
  `cargo build --all --all-features`, `cargo build --all --no-default-features`
  — all exit `0`.
- Boundary `version-mismatch` → exit `4`; protocol matrix N/N±1/unknown/missing
  passes.
- `PYTHONPATH=media .venv/bin/python -m pytest media/tests -q` passes.
- Both benchmark harnesses exit `0` with retired null+reason results.

[1.0.0]: https://github.com/tawsifx/parallel-dl/releases/tag/v1.0.0
