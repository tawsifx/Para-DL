# W0-2 — Contract oracle: final report

**Status:** COMPLETE. RED→GREEN demonstrated, docs/clippy/fmt/example clean, committed as
`feat(W0-2): contract oracle golden fixtures`. No files outside `rust/crates/paradl-oracle/**`
(and this report) were created or modified by this task.

## 1. Scope / ownership compliance

- Owned and edited only `rust/crates/paradl-oracle/**` plus this report.
- Did **not** touch `src/paradl/**`, `tests/**` (read-only oracles), `rust/Cargo.toml`,
  `rust/paradl-rs/**`, `rust/rapids/**`, or any W0-1/W0-3 file.
- Crate manifest: `paradl-oracle`, `version = "0.1.0"`, `edition = "2024"`,
  `rust-version = "1.98"`, `publish = false`, `serde_json` as its only dependency.
- No PyO3 dependency; new crates must not depend on PyO3 (ledger line 27/29).
- A temporary `[workspace]` marker was used during development so the crate could be tested
  before W0-1 landed. **Removed before commit** per the lead's integration alert. Final manifest
  has no `[workspace]`; `cargo test -p paradl-oracle` resolves through the single root workspace
  at `rust/Cargo.toml` (verified from `rust/`, output below). The crate-local `Cargo.lock` and
  `target/` created by the temporary standalone build were deleted.
- No subagents were dispatched (per instruction).

## 2. Golden values — derived by RUNNING the oracle, never invented

Capture method: `.venv/bin/python` (3.12.16) with `PYTHONPATH=src`, `PYO3_PYTHON` set to the venv,
invoking the real oracle modules; `rapids` sidecar bytes captured by a scratch binary
(`cargo run` with a path dependency on the read-only `rapids` crate). Scratch scripts live in
`/private/var/folders/zq/y94lwyxd3691r95vgrbk1ymr0000gn/T/opencode/` and are **not** committed.

| Fixture | Captured from | Highlights |
|---|---|---|
| `core_models.json` | `tests/unit/test_models.py` + `src/paradl/core/models.py` | 22 `parse_rate` inputs (incl. `1Kbps`=128.0, `8Mbit/s`=1,048,576.0, exact `ValueError` text), `format_size`/`format_speed` (incl. `-1` → `0 B` / `-1 B/s`), 29 `sanitize_filename` cases (200-char cap verified at output length 200; `CON`→`_CON`, `NUL.txt`→`_NUL.txt`, `con`→`_con`, `COM0` unchanged), media/image domains, all enum values, 58 direct extensions. |
| `config_defaults.json` | `tests/unit/test_config.py` + `src/paradl/core/config.py` | `CONFIG_SCHEMA_VERSION=2`; full `asdict(PDLConfig())` (16→connections, v2 watchdog, `4MBps`, `engine=rust`, …). |
| `classifier_seeded.json` | `tests/unit/test_classifier.py` (12 seeds 0..11) | 12 `next_best_hours` lists captured byte-for-byte, including exact float-weight strings `0.0/3.0`, `2.7/3.0`, `1.4/3.0`, `1.8/3.0`, `1.7/3.0`; empty case, 5 edge cases, dataset case; each seed's exact sample inputs (4 driving fields) stored for later verification. |
| `report_schema.json` | `src/paradl/throttle/models.py::ThrottleReport.to_dict()` | `schema_version: "1.0"`, full top-level key order, `modules.m0_sanity … m10_local_vs_intl` (11 keys) and exact field order for m0/m10. |
| `watchdog.json` | `tests/unit/test_watchdog.py` + `src/paradl/engines/watchdog.py` | `compute_percentile` p90 of 5 samples = **16000.0** (the test comment says 14000 — the real oracle value was captured, not the comment); 4 `WatchdogDecision` scenarios with exact reasons (`Speed 500 B/s < 2.4 KB/s floor; cycling 16 RPC sockets`, `Straggler connection 48.8 KB/s < 488.3 KB/s/4; …`, `Periodic 5s cycle (speed 4.9 KB/s); …`, `[STALL] no progress for 30s`); `tier2_pause_unpause` action; exact `watchdog_action` JSONL line. |
| `jsonl_events.json` | `src/paradl/ui/display.py::UI.emit_event` (AST capture) + `PARALLEL_DL_DEV_CONTEXT.md` §4 | 43 distinct emitted event names with exact field order; exact envelope line with `timestamp = round(time.time(),3)` = `1763376000.123` from fixed time `1763376000.1234`. |
| `redaction.json` | `src/paradl/ui/display.py::redact_secrets/_redact_obj` | Bearer/token/env redaction strings, nested `_redact_obj` result, exact redacted JSONL line. |
| `sidecar.json` | `rust/rapids/src/state.rs` (run via scratch binary) | Exact 482-byte pretty JSON layout (2-space indent, field order `version,url,final_url,dest,size,validators,segments,content_type,checksum`), `<DEST>`-normalized planner numbers, `.pdl.part`/`.pdl.json` suffixes, `state_version=1`, SHA-256 digest. |
| `media_protocol.json` | charter §4 (authoritative) + `src/paradl/engines/media.py:48–72` (run) | Request/progress/result/error field sets, ops/frames/caps/versions, error.kind→ExitCode (`1/1/1/4/2/130`, never 3), byte-exact worker contracts, 14 captured `classify_audio_production` outputs + lossless codec set, and the version-conformance matrix. |

## 3. Tests

41 tests total: **34 GREEN fixture/protocol tests**, **7 `#[ignore]` contract tests** that require
crates not yet implemented (`paradl-core` W0-3 ×2, `paradl-watchdog` W1-3, `paradl-throttle`
W2-1, `paradl-events` W2-4, `paradl-engine` W1-2, `paradl-media` W3-4b). Each ignored test keeps
its exact expected goldens in the fixture files and an `unimplemented!()` binding site with the
owning wave in the ignore reason.

`paradl-media/1` conformance module covers **N** (`paradl-media/1`, accept), **minor N+1**
(`1.1`, `1.999`, accept — minor additions tolerated), **major N−1** (`0`), **major N+1** (`2`),
**unknown** (`99`), **missing** (null), and unparseable/empty-major — all rejected with exit 4
and an exact diagnostic. A documented reference checker in the test module implements the
charter's handshake rule and is asserted against the fixture matrix.

### TDD evidence

RED (before fixture files existed, run from the crate with the temporary standalone marker):

```
test result: FAILED. 0 passed; 34 failed; 7 ignored; 0 measured; 0 filtered out
```
Every failure was `missing golden fixture …/<name>.json: No such file or directory`, proving the
tests reference the captured fixtures rather than passing vacuously.

GREEN (fresh output after adding fixtures, re-run from `rust/` through the root workspace after
removing the `[workspace]` marker):

```
running 41 tests
test result: ok. 34 passed; 0 failed; 7 ignored; 0 measured; 0 filtered out; finished in 0.02s
   Doc-tests paradl_oracle
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Command:
```
cd rust && PYO3_PYTHON=<worktree>/.venv/bin/python cargo test -p paradl-oracle
```

Additional verification (all from `rust/`):
- `cargo clippy -p paradl-oracle --all-targets -- -D warnings` → clean for this crate (only
  pre-existing root-workspace unused-dependency warnings from W0-1's manifest remain).
- `cargo doc -p paradl-oracle --no-deps` → clean; `#![warn(missing_docs)]` produced no warnings.
- `cargo run -p paradl-oracle --example dump_fixtures` → printed all 9 fixtures with byte sizes.
- `cargo fmt -p paradl-oracle -- --check` → clean.
- `cargo metadata --no-deps` → `paradl-oracle` present as a normal workspace member; no
  competing workspace roots.

Two assertions were corrected during GREEN stabilization (test-side only, goldens unchanged):
serde_json `Value` uses a BTreeMap so JSON key order was verified against the **raw fixture text**
instead of parsed key iteration; and the sidecar `<DEST>` placeholder now substitutes a directory
prefix so the rendered path is `/tmp/pdl-oracle/a.bin`.

## 4. Files changed

```
rust/crates/paradl-oracle/Cargo.toml
rust/crates/paradl-oracle/README.md
rust/crates/paradl-oracle/src/lib.rs
rust/crates/paradl-oracle/src/fixtures.rs
rust/crates/paradl-oracle/src/model_tests.rs
rust/crates/paradl-oracle/src/classifier_tests.rs
rust/crates/paradl-oracle/src/watchdog_tests.rs
rust/crates/paradl-oracle/src/events_tests.rs
rust/crates/paradl-oracle/src/sidecar_tests.rs
rust/crates/paradl-oracle/src/media_protocol_tests.rs
rust/crates/paradl-oracle/examples/dump_fixtures.rs
rust/crates/paradl-oracle/fixtures/core_models.json
rust/crates/paradl-oracle/fixtures/config_defaults.json
rust/crates/paradl-oracle/fixtures/classifier_seeded.json
rust/crates/paradl-oracle/fixtures/watchdog.json
rust/crates/paradl-oracle/fixtures/jsonl_events.json
rust/crates/paradl-oracle/fixtures/redaction.json
rust/crates/paradl-oracle/fixtures/sidecar.json
rust/crates/paradl-oracle/fixtures/report_schema.json
rust/crates/paradl-oracle/fixtures/media_protocol.json
.superpowers/final-rust-framework/task-W0-2-report.md   (this file)
```

## 5. Concerns / notes for downstream waves

1. **Event corpus count:** the source emits **43** distinct event names; the master prompt says
   `~48`. The 43 include all events named in `PARALLEL_DL_DEV_CONTEXT.md` §4 and parity-matrix
   §3; `resilience_event` (E13) is not a standalone emitted event in the oracle — WAKE/NET_CHANGE/
   STALL are carried in the `watchdog_action.resilience_event` field. W2-4 should confirm whether
   the remaining ~5 are new Rust-only events or a documentation count discrepancy.
2. **Classified fixture size:** `classifier_seeded.json` is ~211 KB because it stores the 12
   exact sample sets. This is intentional: it lets W2-1 feed identical inputs to a Rust
   MT19937-compatible generator or replay them directly and compare the byte-exact suggestion
   strings.
3. **Audio worker contract:** `media_protocol.json` records the charter's authoritative frame
   schemas (the Python worker is built in W3-4a, so no running worker output exists yet), while
   `audio_format_production` values were captured by executing `classify_audio_production` in
   the current oracle. W3-4a should re-assert these against the extracted worker.
4. **Sidecar normalization:** the golden stores `<DEST>` as a directory placeholder; a later
   engine test should render with a fixed absolute path and compare the full pretty-JSON bytes
   (the layout itself, including omitted-by-default fields, is exact).
5. **`#[ignore]` contract tests** must have their ignore removed by each owning wave; the
   `unimplemented!()` placeholders are deliberately loud so they cannot be mistaken for parity.
6. **Root `rust/Cargo.lock`:** cargo regenerated it while testing, adding `paradl-oracle` (and,
   concurrently, W0-3's `paradl-core`). I did not commit that shared file because it is owned by
   W0-1; the lead should keep the regenerated lockfile in the integrated tree so both new crates
   are locked.
