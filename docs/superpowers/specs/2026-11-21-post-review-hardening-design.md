# Post-review hardening: modularity, simplicity, efficiency

**Date:** 2026-11-21
**Status:** Design proposed — awaiting lead approval
**Source:** Code review of 2026-11-21 (seven prioritized findings P0–P2)

## Intent and why

paraDL v1.0.0 shipped immediately after a Python→Rust cutover. The review found that
the architecture is sound but the cutover was not finished: ~4,200 lines of orphaned
duplicate code still compile by default, the "thin" app layer is a 1,530-line god-module,
advertised trait/platform abstractions are not exercised in production, and the
per-chunk I/O boundary blocks async workers. The user asked to implement **all seven**
recommendations. The goal is to finish the migration and remove the efficiency gap
without regressing the byte-exact goldens that are the project's parity contract.

## Goals

1. Eliminate blocking fsync/sidecar writes from Tokio worker threads.
2. Remove orphaned legacy crates, their CI baggage, and tracked process artifacts.
3. Wire the real `Platform` implementations into production and make the trait a real
   seam (runtime `dyn` selection with `NoopPlatform` fallback).
4. Reuse SQLite connections and prepared statements; hoist repeated regex compilation;
   stop cloning `HeaderMap` on the same-host path.
5. Reduce the CLI's accepted-then-discarded parity surface.
6. Split `app.rs` along charter boundaries and remove duplicated event/probe types.
7. Keep every existing golden/parity test green; behavior changes must move *toward*
   the deleted Python oracle, never away from it.

## Non-goals

- No new features, no new dependencies, no changes to the media protocol.
- No rewrite of the download concurrency model (bounded `JoinSet`, no lock-across-await,
  single-pass offloaded hashing are already correct).
- No deletion of `docs/superpowers/plans/` (active process location; the new plans live there).

## Anti-goals (what would make this a failure)

- A "simplification" that changes emitted event shapes, help text, or SQLite DDL
  without updating the corresponding golden fixtures deliberately.
- Deleting platform features that the charter marks as required.
- Optimizing bounded ≤16-segment loops at the cost of readability.

## Constraints (verbatim from project docs)

- `rust/Cargo.toml:20`: "New crates MUST NOT depend on PyO3."
- `docs/superpowers/framework-charter.md:38`: "Default feature set compiles WITHOUT
  optional system tools."
- `docs/superpowers/framework-charter.md:172`: "single-emitter rule: `paradl-events` is
  the only JSONL emitter."
- `docs/superpowers/framework-charter.md:205`: "cargo doc --no-deps clean: no missing
  docs on public items."
- `docs/superpowers/parity-matrix.md:429+`: byte-exact parity with the deleted Python
  oracle is the acceptance contract; both Python benchmark harnesses are "retired"
  and must emit `null`+reason instead of crashing.
- Release profile is fixed: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`.

## Approaches considered

### A — Safe core only (rejected by lead)
Do findings 1, 2, 4, 5. Leaves the god-module, unwired Platform, and CLI cruft in place.
Rejected: the lead chose "Everything."

### B — Everything, big-bang single commit (rejected)
One monolithic change across all seven findings. Rejected: too large to review, and a
failure in one workstream would block all the others.

### C — Everything, four independent workstreams, one plan each (CHOSEN)
Each workstream produces independently testable, compilable software and is reviewed
before the next starts. Order: **B (cleanup) → A (efficiency) → C (structural) → D (CLI)**.
Cleanup first shrinks the build surface (no PyO3, no duplicate engine) so subsequent
test cycles are faster; CLI last because it deliberately touches golden help fixtures.

## Decisions already made with the lead

- **Platform:** wire the real `MacOSPlatform`/`WindowsPlatform` via compile-time
  selection with a `NoopPlatform` fallback; make `auth set/remove/status` use the
  platform secret store so tokens actually persist. Do *not* delete the platform
  features. (Chosen over "drop the abstraction.")
- **Scope:** all seven findings, including the CLI parity-flag removal and the
  `app.rs` split.

## Workstream B — Cleanup (findings 2, 5, S6)

**Files:**
- `rust/Cargo.toml` — drop `rapids` and `paradl-rs` from `members`; add explicit
  `default-members` excluding `paradl-oracle` and `paradl-parity` once glob support is
  confirmed from Cargo docs via Context7.
- Delete `rust/rapids/` and `rust/paradl-rs/` from disk.
- `.github/workflows/ci.yml` — remove the "Set up Python 3.12 for the legacy PyO3
  member" steps and all `PYO3_PYTHON` env vars from the Rust jobs; simplify rustfmt
  check to `--all`; keep the media-package/cross-boundary Python jobs unchanged.
- `README.md:283-295` — remove the PyO3 contributor note and `PYO3_PYTHON` commands.
- `pyproject.toml`, `uv.lock` — delete (root stub; `media/pyproject.toml` is the only
  real Python manifest). Keep `.python-version` (media tooling still targets 3.12).
- `benchmarks/bench_baseline.py`, `bench_rust_engine.py` — change the retirement guard
  from `ORACLE_DIR.is_dir()` to an oracle *source-file* check
  (`(ORACLE_DIR / "__init__.py").is_file()`), so leftover `__pycache__` cannot defeat it.
- Delete tracked process artifacts: `.superpowers/reviews/*` (6 files),
  `docs/master-prompts/*` (5 files), `benchmarks/results/*.json` (20 files).
  Keep `docs/superpowers/plans/` (active process dir).
- Local-only residue (untracked, gitignored) `src/`, `tests/`, `test.jpg`, `BUG_REPORT.md`,
  `PARALLEL_DL_DEV_CONTEXT.md` — leave in place; the corrected guard makes them inert.

**Verification:** `cargo build --all` and `cargo test --all -- --skip
full_non_media_gate_is_clean` green; `cargo tree` contains no `pyo3` and no duplicate
`base64` from `rapids`. `syn 2.0` is explicitly allowed — it remains via
`tracing-attributes` and other proc-macro dependencies, independent of PyO3. A fresh
`git status` shows only intended deletions.

## Workstream A — Efficiency (findings 1, 4)

- **A1 (E1):** in `paradl-engine/src/download.rs`, extract the sidecar write into an
  owned, `Send + 'static` helper; drop the `resume_state` mutex guard *before* calling
  it, and invoke it with `tokio::task::spawn_blocking`. Mirror the pattern already used
  by verification at `download.rs:883-888`. Final `rename`/`remove_file` also offloaded.
  Add a regression test that pins the offload (see plan).
- **A2 (E2):** keep the per-chunk `pwrite` on the executor but document it as a
  deliberate tradeoff in `download.rs` (a per-write `spawn_blocking` costs more than
  the page-cache-hit write).
- **A3 (E3):** `paradl-store`: hold one `Mutex<Connection>` per `SqliteStore` instead of
  calling `connect()` per operation; set `journal_mode=WAL` once at init and
  `busy_timeout` per open; switch repeated SQL to `prepare_cached`. (Context7-confirmed
  rusqlite pattern.)
- **A4 (E4):** `paradl-postproc/src/checksum.rs`: hoist the BSD checksum regex to a
  `static BSD_RE: LazyLock<Regex>`.
- **A5 (E5):** `paradl-engine/src/probe.rs`: return `Cow<'_, HeaderMap>` from
  `scope_headers()`, borrowed when no cross-host stripping is needed.

**Verification:** new unit tests for A1/A4/A5; full `cargo test --all`; `cargo clippy
--all -- -D warnings`.

## Workstream C — Structural (findings 3, 7)

- **C1 Platform wiring:** add `paradl-platform::current()` returning `&'static dyn
  Platform` selected by `#[cfg(all(feature, target_os))]` with a `NoopPlatform` fallback;
  forward `platform-macos`/`platform-windows` features through `paradl-app` and the
  `paradl` binary. Replace `NoopPlatform` at `app.rs:1011`; make `auth_status` consult
  `Platform::load_secret` as the primary source (env fallback preserved, since the
  charters give env precedence). Call `set_origin_metadata` after a successful transfer.
  This is the one place we adopt a trait object deliberately: runtime OS/feature
  selection cannot be expressed with static dispatch without duplicating call sites.
  Other charter traits stay as zero-cost static-dispatch bounds; that is a deliberate
  call, not an oversight, and will be documented in `app.rs`.
- **C2 app.rs split:** extract `transfer.rs` (transfer/run_engine/watchdog config),
  `naming.rs` (fallback_name/destination/clean_orphans), and `commands.rs`
  (config/history/doctor helpers), leaving `app.rs` as orchestration only (<500 lines).
  Characterization tests run before the split.
- **C3 event dedup:** keep the `paradl-app/src/events.rs` row structs that emit
  explicit nulls on raw stdout (`TargetClassifiedRow`, `ProgressRow`, `ResultRow`) —
  they are a deliberate exception because `paradl-events` schema types use
  `skip_serializing_if` and cannot produce those byte-exact nulls. Dedup only the
  types whose serde behavior is genuinely compatible, and document the exception in
  `events.rs` with a link to the raw-stdout goldens. Do **not** change schema serde
  attributes or drop nulls from goldens (that would move away from the oracle).
- **C4 probe dedup:** compare `paradl-engine/src/probe.rs:145` with
  `paradl-net/src/client.rs:520,584`; consolidate the shared HTTP probe into
  `paradl-net` and have `paradl-engine` consume it. If the two differ in behavior,
  keep both and document why rather than forcing a wrong merge.

**Verification:** full suite + golden tests; `paradl doctor` and `paradl auth status
--json` smoke tests; `cargo doc --no-deps` clean.

## Workstream D — CLI (finding 6)

- Remove `--accept-ranges`, `--total`, `--http3`/`--no-http3` from
  `paradl-cli/src/parser.rs` and their dispatch handling. Keep `--http3-probe` (F59)
  and keep `paradl-net/src/compat.rs` — it is publicly re-exported and used by
  `routing_oracle.rs` and an example, so the "delete if unused" condition does not
  fire. The `netprobe.http3_supported` field stays; compute it from `http3_probe`
  alone now that `--no-http3` is gone, preserving C17/F59 goldens.
- Keep subcommands as canonical for `--install`, `--info`, `--hash`; remove the
  duplicate long-flag forms.
- Amend the parity contract deliberately: retire rows F09, F11, F60 (and the note at
  `parity-matrix.md:178`) with a "retired at W5-3; oracle deleted" reason, and adjust
  the audit loop `paradl-app/tests/parity.rs::every_cli_flag_row_has_a_proving_path`
  (currently loops F01..=F92) to skip retired rows. Do not silently leave rows whose
  proving path no longer exists.
- Regenerate/update `rust/crates/paradl-cli/tests/fixtures/help.txt`,
  `throttle_help.txt`, and any oracle-parity assertions that reference removed flags.
- Update README flag documentation where present.

**Verification:** `cargo test -p paradl-cli`, full suite, and `paradl --help` compared
to the updated fixture.

## Builder-check resolutions (calls made, not open questions)

A fresh engineer reviewed the design before planning. Five points were settled from
the constraints; two were confirmed by the lead. All resolutions are recorded here so
the plan is unambiguous:

1. **Parity matrix authority:** the lead's "Everything" scope explicitly removes the
   CLI parity flags, so Workstream D amends `parity-matrix.md` (retire F09/F11/F60,
   adjust the F01..=F92 audit loop) rather than treating the matrix as frozen.
2. **Event dedup:** keep explicit-null row structs where goldens require them; dedup
   only serde-compatible types. No schema serde changes, no golden null removal.
3. **`--http3-probe`:** retained; `compat.rs` retained (still used); `http3_supported`
   stays in the netprobe event, now derived from `http3_probe` only.
4. **Auth precedence:** keep `lookup_token`'s env → keychain → none chain and its
   `source` metadata/redaction; add a thin `SecretStore` adapter over `Platform`
   rather than calling `load_secret` directly.
5. **Dependency gate:** assert no `pyo3` and no duplicate `base64`; allow `syn 2`.
6. **Default features (lead):** stay feature-gated. `default = []` and `NoopPlatform`
   remain per the charter; real persistence is active with `platform-macos` /
   `platform-windows`, and CI/smoke tests cover those configurations. "Wired" means
   the production code path exists and is tested in the enabled configurations.

## Verified technical assumptions (Context7 / Cargo Book)

- `rusqlite::Connection` is `Send` but **not** `Sync`; sharing one connection requires
  `Mutex<Connection>`. `prepare_cached` returns statements to an LRU cache on drop.
  Source: `/rusqlite/rusqlite` — confirms A3's design.
- Cargo's `default-members` documentation shows explicit paths (globs are documented
  for `members`, not for `default-members`). Use an explicit list of shippable crates,
  excluding `paradl-oracle` and `paradl-parity`. Source: Cargo Book, "Workspaces".
- Tokio's documented pattern for blocking fs and CPU-bound work is
  `task::spawn_blocking`; verification already follows it at `download.rs:883-888`.
  Source: `/websites/rs_tokio` — confirms A1's design.
- reqwest `Client` is `Arc`-backed and meant to be created once and reused; the code
  already does this at `paradl-net/src/client.rs:335`. No change needed.

## What is left to the builder

Exact function signatures, test names, and module paths for the `app.rs` split are left
to each implementation plan. The `default-members` list will be written explicitly
(not as a glob), per the verified assumption above.
