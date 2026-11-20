# W0-3 Implementer Report — `paradl-core`

**Status: COMPLETE.** BASE `0ebb09e`, branch `final-rust-framework`. No commits on main, no push.

## Scope honored

- Edited/created ONLY `rust/crates/paradl-core/**` (incl. its own `Cargo.toml`) and this report.
- Did not touch `rust/Cargo.toml` (W0-1), `rust/crates/paradl-oracle/**` (W0-2), `src/paradl/**`, or `tests/**`.
- No PyO3 dependency. Host note `PYO3_PYTHON=.../.venv/bin/python` was set for workspace commands regardless.
- The temporary standalone `[workspace]` marker used for isolated TDD was **removed** before finish; the crate is a normal member of W0-1's root workspace (`members = ["crates/*", ...]`). Final verification ran from `rust/` with no `exclude` needed.

## TDD evidence (RED → GREEN)

1. Captured exact golden values from the Python oracle via `.venv/bin/python` scripts against
   `paradl.core.models` / `paradl.core.config` (rates, sizes, speeds, sanitization, domains,
   extension set, enum values, config defaults/validation messages, `set()` messages).
2. Wrote `tests/golden.rs` with those literals plus a temporary stub implementation.
   RED run: `20 failed; 1 passed` (real assertion failures, not just compile errors).
3. Implemented the crate. GREEN run (from `rust/`, through root workspace):
   - `cargo test -p paradl-core` → **9 unit + 22 golden + 5 doc tests pass**.
   - `cargo doc -p paradl-core --no-deps` → clean, no missing-doc warnings (`#![warn(missing_docs)]`).
   - `cargo run -p paradl-core --example basics` → runs, prints expected values.
   - `cargo clippy -p paradl-core --all-targets -- -D warnings` → clean for this crate
     (workspace-level unused-dependency warnings from W0-1's manifest are unrelated).
   - `cargo build --all` → succeeds alongside all W0-1/W0-2 crates.
   - `cargo fmt --check` → clean.

## Contract coverage

- **L15 / charter §3:** `ExitCode { Ok=0, Failure=1, Usage=2, VerificationFailed=3, MissingDependency=4, Interrupted=130 }`; `Error` is the single public error type with `exit_code()`, `Display`/`std::error::Error` via `thiserror`, constructors, `From<std::io::Error>`, and `from_media_kind` (metadata/download/conversion→1, dependency→4, usage→2, interrupted→130, unknown→1; never 3).
- **Charter §2:** `pub trait Clock { fn now(&self) -> SystemTime; }`, implemented by `SystemClock` and `FakeClock` (with Python-compatible `monotonic`/`sleep`/`sleep_history`/`advance`/`jump_wall` defaults 1_700_000_000.0 / 1000.0).
- **L02 / helpers:** `sanitize_filename` — 200-char cap, unsafe-char table, dot-only→`download`, Windows reserved names (CON/PRN/AUX/NUL/COM1-9/LPT1-9) prefixed with `_`; byte-exact against all oracle cases.
- **Numeric parity:** 1024 multipliers, bit units `/8`, `int(bps)` truncation for aria2, and Python banker's rounding in `format_size`/`format_speed` (verified against oracle plus explicit tie checks).
- **Routing literals:** `DIRECT_EXTENSIONS` (58 entries), `MEDIA_DOMAINS` (14), `IMAGE_DOMAINS` (5) each defined once; `has_direct_extension`/`direct_extension` strip query/fragment, lowercase, and match suffix from the single constant; `is_media_domain`/`is_image_domain` mirror hostname suffix rules incl. pollinations.ai `/prompt/` special case.
- **G01 / G02:** full `config.json` field set in Python declaration order, defaults, validation/normalization (clamp 16, min grace 3, off-like rate strings→None, lowercasing, exact error messages), `set()` coercion + exact failure messages, atomic `save()` (temp file + fsync + rename, no `.tmp` leftovers), `load()` with `.bad` backup, `schema_version` migration to 2, `Duronto Internet`→`auto`, and `PARADL_HOME` isolation in `paths`.
- **Docs:** crate-level `//!` docs with doc-test, README with role/trait/feature flags/example link, runnable `examples/basics.rs`, every public item documented.

## Files changed

- `rust/crates/paradl-core/Cargo.toml` (crate `paradl-core`, 0.1.0, edition 2024, rust-version 1.98, thiserror/serde/serde_json/url; no `[workspace]`)
- `rust/crates/paradl-core/src/{lib,error,models,config,paths,clock}.rs`
- `rust/crates/paradl-core/tests/golden.rs`
- `rust/crates/paradl-core/examples/basics.rs`
- `rust/crates/paradl-core/README.md`
- `.superpowers/final-rust-framework/task-W0-3-report.md`

## Concerns / handoff notes

- `PDLConfig::get_effective_isp()` returns the configured ISP or the `"auto"` sentinel; actual detection belongs to `paradl-net` (W1-4) and is intentionally not duplicated in core.
- `parse_rate` accepts `impl Into<RateInput>` (`&str`, `String`, integers, floats, `Option<T>`, or `RateInput::None` for Python `None`); `format_size` takes `Option<f64>`. Downstream crates should use these forms.
- `PDLConfig::set()` persists through `save()` like Python; callers/tests that must not touch the real home should set/isolate `PARADL_HOME` (golden tests do this under a mutex).
- During verification, W0-2's in-progress `paradl-oracle` briefly had its own `[workspace]` marker; that transient root conflict cleared before final verification. No W0-2 files were modified.
- Root `rust/Cargo.lock` gained a `paradl-core` dependency entry during workspace builds, but that file is outside my ownership (W0-1) and the shared diff also contains W0-2's `paradl-oracle` entry, so I left it unstaged; normal (non-`--locked`) Cargo commands regenerate it.

---

## Fix loop round 1 — `fix(W0-3): review round 1`

### Changes (only `rust/crates/paradl-core/**` + this report)

1. **Important — active-engine sync.** `PDLConfig::set()` now calls
   `set_active_engine(&candidate.engine)` after validation and before `save()`/commit,
   restoring the previous engine if the save fails. `PDLConfig::load()` calls
   `set_active_engine(&config.engine)` after successful validation before returning
   (corrupt/unparseable files still fall back to defaults without touching the global).
   Added goldens: `config_set_engine_syncs_active_engine` (both `aria2` and `rust`),
   `config_load_syncs_active_engine` (config.json with `"engine":"aria2"`).
2. **Important — `parse_rate` IEC marker.** Moved `i` consumption out of the
   magnitude-prefix branch so `([kKmMgGtT])?(i)?` is honored with or without a
   prefix. Added goldens for `"1ib"` and `"1kib"` (plus existing `"1b"`).
3. **Accepted handoff — ISP golden only.** Added
   `config_get_effective_isp_explicit_branch` (`isp="Comcast Cable"` →
   `"Comcast Cable"`). No auto-detection code added, per lead ruling.
4. Test-harness robustness: `with_paradl_home` now recovers a poisoned `ENV_LOCK`
   so one failing test cannot cascade masking failures onto the rest.

### Python oracle values captured before asserting

Command: `PYTHONPATH=src .venv/bin/python -c "from paradl.core.models import parse_rate; print('1ib ->', parse_rate('1ib')); print('1b ->', parse_rate('1b')); print('1kib ->', parse_rate('1kib'))"`

Output:
```
1ib -> 0.125
1b -> 0.125
1kib -> 128.0
```

### RED evidence (new goldens before code fix)

`cargo test --offline -p paradl-core --test golden` → `test result: FAILED. 18 passed; 7 failed`
(`parse_rate_golden_values`: expected `0.125`, got unparseable error;
`config_set_engine_syncs_active_engine`: `left: "rust"`, `right: "aria2"`;
`config_load_syncs_active_engine` likewise). FIX 3's golden passed from the start,
as expected for an accepted handoff.

### GREEN verification (commands + outputs)

```
$ PYO3_PYTHON=<worktree>/.venv/bin/python cargo test --offline -p paradl-core
    Finished `test` profile [unoptimized + debuginfo] target(s)
running 9 tests   (unit)       -> test result: ok. 9 passed; 0 failed
running 25 tests  (golden)     -> test result: ok. 25 passed; 0 failed
running 5 tests   (doc-tests)  -> test result: ok. 5 passed; 0 failed

$ PYO3_PYTHON=<worktree>/.venv/bin/python cargo clippy --offline -p paradl-core -- -D warnings
    Checking paradl-core v0.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s)
    (only workspace-manifest unused-dependency warnings from W0-1; none for paradl-core)

$ cargo fmt --check -p paradl-core
OK
```

No files outside `rust/crates/paradl-core/**` and this report were modified;
`rust/Cargo.lock` remains lead-owned and uncommitted.

