# W0-1 Report — Workspace, CI, Distribution, Remove Committed `.so`

**Status:** DONE_WITH_CONCERNS
**Branch:** `final-rust-framework` · **BASE:** `0ebb09e` · **Commit:** `feat(W0-1): workspace, CI, distribution, remove committed .so`

## Standing ruling observed

The entire Python app becomes Rust **except the media subsystem**, which stays a Python
package invoked as `python -m paradl_media` over `paradl-media/1`. No all-Rust media plan,
no PyO3 in new crates, no sidecar. Rust owns the full ExitCode map; the worker emits
`error.kind` only.

## Files changed / created

Workspace and crates (owned):
- `rust/Cargo.toml` — `members = ["crates/*", "rapids", "paradl-rs"]`, `resolver = "2"`,
  workspace package values kept (0.1.0 / edition 2024 / rust-version 1.98 / MIT), full
  shared `[workspace.dependencies]` table (tokio, reqwest, serde, serde_json, clap,
  thiserror, async-trait, anyhow, tracing, tracing-subscriber, axum, ratatui, rusqlite,
  hickory-dns, blake2, zip, tar, sha2, md-5, sha1, crc32fast, base64, hex, httpdate, url,
  bytes, futures, tokio-util, uuid, chrono, regex, dirs, notify, tempfile) and the feature
  matrix documented (`api`, `tui`, `media`, `platform-macos`, `platform-windows`, `doh`,
  `server`; all opt-in).
- 14 new stub manifests + `src/lib.rs` with crate docs and `#![warn(missing_docs)]`:
  `paradl-store`, `paradl-net`, `paradl-engine`, `paradl-watchdog`, `paradl-throttle`,
  `paradl-postproc`, `paradl-platform`, `paradl-media`, `paradl-events`, `paradl-ui`,
  `paradl-api`, `paradl-cli`, `paradl-app`, `paradl-parity`. Feature flags are declared in
  the owning manifests (`doh`, `server`, `media`, `tui`, `api`, `platform-*`). Stubs are
  deliberately dependency-free and empty so downstream owners add their own APIs.
- `rust/crates/paradl/Cargo.toml` + `rust/crates/paradl/src/main.rs` — thin reference
  binary; `--version`/`-V`/`--help` only; `--version` derives its name from `argv[0]` so the
  `parallel-dl` symlink reports `parallel-dl 0.1.0`. 3 unit tests cover version text,
  symlink naming (portable across `/` and `\`), and the pinned `paradl-media/1` constant.
- `rust/Cargo.lock` — regenerated for the 15 new local packages (automatic consequence of
  the manifest change; no external dependency was added).

CI/distribution (owned):
- `.github/workflows/ci.yml` — kept the Python lint/type and macOS/Windows test jobs plus
  `bench`; added `rust-native` (macos-14, macos-13, windows-latest; build/test/clippy
  `-D warnings`/rustfmt/version smoke, `PYO3_PYTHON` pinned to Python 3.12), `media-package`
  (Py 3.11/3.12; uv venv, pip-installed media package, asserts no console script), and
  `cross-boundary` (happy-path handshake + protocol-mismatch negative test that asserts
  exit code 4).
- `Formula/parallel-dl.rb` — Rust release build into `bin`, media venv in `libexec`,
  `depends_on "rust"`, keeps `python@3.12` + `ffmpeg`, `deno => :recommended`, drops aria2;
  test block runs `paradl --version`, `parallel-dl --version`, `paradl doctor`, and a
  one-shot `paradl-media/1` handshake.
- `scripts/install-local.sh` / `.ps1` — build/copy native binary, `uv venv` +
  `uv pip install` of `yt-dlp[default]` + `mutagen` (+ best-effort `yt-dlp-ejs`), editable
  install of `media/` when present, symlinks/copies for `paradl` and `parallel-dl`, and
  `PARADL_MEDIA_PY` exported/persisted.
- `git rm` of `src/paradl/_rust_engine.cpython-312-darwin.so` and
  `rust/paradl-rs/python/paradl/_rust_engine.cpython-312-darwin.so`. `git ls-files '*.so'`
  is now empty.

Not touched: `src/paradl/**` (other than the explicitly-owned `.so` deletion), `tests/**`,
`docs/**`, `.superpowers/**` (other than this report), `rust/paradl-rs/**` and
`rust/rapids/**` sources, `paradl-core` / `paradl-oracle` (owned by W0-3/W0-2).

## Verification (fresh output)

Command run from `rust/` with `PYO3_PYTHON="$PWD/../.venv/bin/python"`:

| Check | Result |
|---|---|
| `cargo metadata --no-deps` | 17 members, all expected names present |
| `cargo build --all` | exit 0 — `Finished dev profile` |
| `cargo test --all` | exit 0 — 3 passed (paradl, incl. the `--version` tests), 21 passed (rapids), 31 suites 0 tests |
| `cargo clippy --all -- -D warnings` | exit 0 |
| `cargo doc --no-deps` | exit 0 |
| `cargo fmt --check` (15 framework crates) | exit 0 |
| `cargo run -p paradl -- --version` | `paradl 0.1.0` |
| `./target/debug/paradl --version` via `parallel-dl` symlink | `parallel-dl 0.1.0` |
| `ruby -e "YAML.load_file('.github/workflows/ci.yml')"` | CI YAML OK (7 jobs) |
| `bash -n scripts/install-local.sh` | OK |
| `ruby -c Formula/parallel-dl.rb` | Syntax OK |
| `git ls-files '*.so'` | empty |

TDD note: the `--version` unit tests were written first and initially failed
(`symlink_name_is_preserved` asserted `paradl.exe` while the implementation strips the
Windows `.exe` suffix); I corrected the expectation to match the intended behavior, then
re-ran to green. CI YAML validity is checked with Ruby's psych (PyYAML is not installed in
this workspace).

## Concerns

1. **Concurrent temporary workspace markers (resolved at verification, must stay resolved).**
   While I verified, W0-2 and W0-3 still had `TEMPORARY ... [workspace]` markers in
   `rust/crates/paradl-oracle/Cargo.toml` and `rust/crates/paradl-core/Cargo.toml`, which
   make Cargo report "multiple workspace roots". I temporarily added
   `exclude = ["crates/paradl-core", "crates/paradl-oracle"]` to run the verification, then
   removed it before the commit. **Those two agents must remove their markers before the
   final Wave 0 gate**, otherwise `cargo build --all` from `rust/` fails with the multiple
   roots error. My committed `rust/Cargo.toml` has no exclude.
2. **18 `unused workspace dependency` cargo warnings.** The charter/lead explicitly asked
   for the shared dependency table, but the empty stubs do not consume them yet. These are
   Cargo manifest warnings (not rustc), so `clippy -D warnings` stays green; they disappear
   as W1–W4 owners adopt the dependencies.
3. **`cargo fmt --all -- --check` is red on legacy code.** 76 diffs come from
   `rust/rapids/src/*.rs` and `rust/paradl-rs/src/lib.rs`, which I am forbidden to edit. I
   scoped the CI rustfmt step to the 15 framework crates I own; a later wave should format
   the legacy sources and widen the step back to `--all`.
4. **Forward-looking CI/Formula steps.** The `paradl doctor` Formula assertion (W4-1), the
   media package install (W3-4a), and the `paradl-parity boundary --case version-mismatch`
   harness (W4-2) do not exist yet; those jobs/steps are intentionally written for their
   future implementations. The media-package job degrades gracefully when `media/` is
   absent; the cross-boundary gate is expected to go green at W4-2.
5. **Removing `src/paradl/_rust_engine...so`** may break any Python oracle test that imports
   the prebuilt extension until the extension is rebuilt (e.g. via maturin) — this is the
   explicit X01 requirement, so I did not add a fallback.
6. `pwsh` is not available on this host, so `scripts/install-local.ps1` was reviewed
   manually rather than executed; the equivalent Bash script passed `bash -n`.
