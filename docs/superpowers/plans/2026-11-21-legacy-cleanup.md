# Legacy Cleanup Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
**Goal:** Finish the Python→Rust cutover by removing the orphaned legacy crates, their CI/root-Python scaffolding and tracked process artifacts, and make the retired benchmark harnesses immune to leftover `__pycache__` residue.
**Architecture:** This is Workstream B only: shrink the Cargo workspace to the 18 shippable `crates/*` members with an explicit 16-path `default-members` list, leaving dev-only `paradl-oracle`/`paradl-parity` buildable via `--all`. CI and docs lose all PyO3/rapids guidance, the root Python stubs and process artifacts leave git, and the two benchmark harnesses switch from directory-existence to oracle-source detection. No golden, feature flag, media protocol, or dependency version is changed.
**Tech Stack:** Cargo workspace (Rust 1.98 / edition 2024), `cargo metadata` + `cargo tree` as executable gates, GitHub Actions YAML, Python 3 benchmark harnesses with pytest, git tracked-file hygiene.
**Spec:** docs/superpowers/specs/2026-11-21-post-review-hardening-design.md

## Global Constraints

Verbatim from the design (`docs/superpowers/specs/2026-11-21-post-review-hardening-design.md`, "Constraints (verbatim from project docs)"):

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

Additional binding decisions (from the design's Workstream B / "Builder-check resolutions" and from grounding this plan):

- **Scope:** findings 2/5/S6 only. Workstreams A/C/D are untouched, even where their files contain leftover PyO3/rapids *strings*.
- Keep `docs/superpowers/plans/` (active process location — this plan lives there). Delete `.superpowers/reviews/*`, `docs/master-prompts/*`, `benchmarks/results/*.json`.
- Keep `.python-version` (media tooling targets 3.12) and `media/pyproject.toml` (the only real Python manifest). Delete only root `pyproject.toml` and `uv.lock`.
- Leave local-only residue in place: `src/`, `tests/`, `test.jpg`, `BUG_REPORT.md`, `PARALLEL_DL_DEV_CONTEXT.md`. Grounding: `src/paradl/` currently contains ONLY `__pycache__/*.pyc` (76 files, no `.py`, no `__init__.py`), which is precisely why the directory-based guard fails.
- `default-members` is an explicit 16-path list (Cargo documents paths, not globs, for this key — Cargo Book "Workspaces", confirmed via Context7). `members` stays `["crates/*"]`. `--all`/`--workspace` still selects all 18 crates.
- **Verification gate:** (G1) `cargo tree --workspace | grep -i pyo3` is empty; (G2) `cargo tree --workspace -i base64@0.22.1 | grep -iE 'rapids|paradl-rs'` is empty (no base64 duplicate *edge through the deleted crates*); (G3) `syn` 2/3 is allowed and never a failure. Grounded nuance: `cargo tree --workspace -d` currently lists base64 `0.22.1` and `0.23.1`; the `0.23.1` copy comes from `hyper-util` via `reqwest` and survives this workstream. Removing it needs a dependency upgrade (forbidden: "no new dependencies"), so it is explicitly accepted. The design's "no duplicate base64" is enforced as "no duplicate attributable to `rapids`", not as an absolute single-version claim.
- CI: edit **only** the `rust-native` job. The `media-package` and `cross-boundary` jobs stay byte-for-byte unchanged per the design ("keep the media-package/cross-boundary Python jobs unchanged"), including `cross-boundary`'s PyO3-named setup step and its `PYO3_PYTHON` env vars (they are inert for the Rust build and the Python pin still serves the media venv).
- Do **not** touch `rust/crates/paradl-cli/src/dispatch.rs:645` (`"PYO3_PYTHON"` in `CLI_ENV_VARS`) or `rust/crates/paradl-cli/tests/oracle_parity.rs:386` (asserts that list), `rust/crates/paradl-parity/**`, any `doctor` golden containing `"Rust Engine (native rapids)"`, or `docs/superpowers/framework-charter.md:4` (stale master-prompt provenance after artifact deletion). Those are Workstream C/D or historical-provenance matters; changing them here would move goldens or exceed B.
- The only surviving `rapids` string in Rust source is an inert crates.io keyword at `rust/crates/paradl-engine/Cargo.toml:13`; no crate manifest has a path dependency on the deleted crates. This is the proof that disk deletion cannot break the build.

## Review Focus

1. **Residue directory defeats the retirement guard.** `benchmarks/bench_baseline.py:317`, `bench_rust_engine.py:430,507,1047` test `ORACLE_DIR.is_dir()`, which is TRUE for today's `src/paradl/__pycache__`-only residue. Pin: `benchmarks/tests/test_retirement_guard.py` builds a temp `src/paradl/__pycache__/` with no `__init__.py`, monkeypatches `ORACLE_DIR`, and asserts `run()` returns `status == "retired"` with all `value is None` and `run_benchmark_suite()` returns `retired is True` — no import attempted.
2. **Glob/`default-members` drift.** `members = ["crates/*", "rapids", "paradl-rs"]` silently re-admits deleted paths; a glob in `default-members` would also pull in dev-only harnesses. Pin: `cargo metadata --no-deps --format-version 1` must report exactly 18 package names (no `rapids`/`paradl-rs`) and exactly 16 `workspace_default_members` excluding `paradl-oracle`/`paradl-parity`; bare `cargo build` from `rust/` must succeed.
3. **Transitive dependency re-entry / gate over-claim.** PyO3 can only enter via `paradl-rs`; base64 must not acquire a second *legacy* edge; `syn 2` must not be mistaken for PyO3 residue. Pin: G1/G2/G3 above, run before and after deletion.
4. **CI step-output dangling reference.** Removing `actions/setup-python` (id `py`) while leaving `${{ steps.py.outputs.python-path }}` breaks the job at runtime; leaving the scoped `-p` fmt list hides unformatted crates. Pin: a `grep` assertion over the `rust-native` block forbids `PYO3_PYTHON|steps\\.py|PyO3|rapids|paradl-rs` and requires literal `cargo fmt --check --all`.
5. **Deleting the wrong Python files / stale docs.** Root `pyproject.toml`/`uv.lock` are stubs, but `.python-version` and `media/pyproject.toml` are load-bearing; README/stale artifact references must not be "fixed" by touching goldens or protected docs. Pin: `test -f .python-version && test -f media/pyproject.toml`, `python -c "import tomllib; ..."` on the media manifest, `git ls-files` counts of 0/0/0/20 for the deleted globs, and `! grep -qE 'PYO3_PYTHON|rapids|paradl-rs' README.md`.

## Task 1: Remove legacy workspace members and pin explicit defaults

**Files:**
- Modify: `rust/Cargo.toml` (lines 1-10: header comment + `members` + new `default-members`; keep line 20 unchanged)
- Modify (regenerated): `rust/Cargo.lock` (drops `rapids`, `paradl-rs`, `pyo3` packages)
- Delete: `rust/rapids/` (30-line `Cargo.toml` + all sources), `rust/paradl-rs/` (`Cargo.toml`, `pyproject.toml`, `src/lib.rs`, `contract_tests/`)
- Test (command-level, no new file): `rust/Cargo.toml`, `rust/Cargo.lock`, `cargo metadata`/`cargo tree` output

**Interfaces:**
- Consumes: `cargo metadata --no-deps --format-version 1` → `packages[].name`, `workspace_default_members`; `cargo tree --workspace`, `cargo tree -d`, `cargo tree -i base64@0.22.1`
- Produces: `members = ["crates/*"]`; `default-members` = `crates/paradl`, `crates/paradl-api`, `crates/paradl-app`, `crates/paradl-cli`, `crates/paradl-core`, `crates/paradl-engine`, `crates/paradl-events`, `crates/paradl-media`, `crates/paradl-net`, `crates/paradl-platform`, `crates/paradl-postproc`, `crates/paradl-resilience`, `crates/paradl-store`, `crates/paradl-throttle`, `crates/paradl-ui`, `crates/paradl-watchdog`; G1/G2/G3 green.

- [ ] **1. Write the failing test.** From `/Users/tawsifx/paradl/rust`, use these exact assertions (capture the baseline build first, so deletion is proven non-breaking):
```bash
# Baseline needs the legacy PyO3 override on hosts whose system Python is newer
# than PyO3 0.27 supports (this host: 3.15). Use the repo's 3.12 venv by absolute
# path so the command also works from a worktree:
PYO3_PYTHON=/Users/tawsifx/paradl/.venv/bin/python cargo build --all   # baseline: must be green BEFORE any deletion
cargo metadata --no-deps --format-version 1 | python3 -c '
import json,sys; d=json.load(sys.stdin); n={p["name"] for p in d["packages"]}
assert not ({"rapids","paradl-rs"} & n), sorted(n)
assert len(d["workspace_default_members"]) == 16, d["workspace_default_members"]'
test -z "$(cargo tree --workspace | grep -i pyo3)"
test -z "$(cargo tree --workspace -i base64@0.22.1 | grep -iE "rapids|paradl-rs")"
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** The metadata assertion raises `AssertionError` listing `rapids`/`paradl-rs` (20 members, 20 default members today); `grep -i pyo3` shows `pyo3 v0.27.2` via `paradl-rs`; `cargo tree -i base64@0.22.1` shows `rapids v1.0.0 → paradl-rs v1.0.0`. `cargo build --all` itself is green — record that as the pre-deletion baseline.
- [ ] **3. Implement minimal code.** In `rust/Cargo.toml`, replace the stale lines 3-7 with a one-line note that the media subsystem is the only Python component; set `members = ["crates/*"]`; insert the 16-path `default-members` list above. Delete both directories with `git rm -r rust/rapids rust/paradl-rs`. Regenerate the lockfile (`cargo metadata --no-deps --format-version 1 >/dev/null`) and stage `rust/Cargo.lock`. Do not touch the line-20 PyO3 prohibition comment.
- [ ] **4. Run the test, verify PASS.** Re-run step 1's four assertions: metadata has 18 members / 16 defaults, no pyo3 line, no rapids/paradl-rs base64 edge. `cargo tree --workspace -d` may still list `base64 v0.23.1` from `hyper-util` — accepted per Global Constraints.
- [ ] **5. Run the crate/full suite.** `cd /Users/tawsifx/paradl/rust && cargo build --all && cargo test --all -- --skip full_non_media_gate_is_clean && cargo clippy --all -- -D warnings`. All green; the single skip is the documented W4-2 oracle-gated test.
- [ ] **6. Commit.** `git commit -m "chore(rust): drop legacy rapids/paradl-rs members and pin default-members"`.

## Task 2: Remove PyO3 scaffolding from the rust-native CI job

**Files:**
- Modify: `.github/workflows/ci.yml` (lines 28-35 setup step; `env:` blocks at 49-51, 55-57, 65-67, 83-85, 92-94; rustfmt step 69-79 with comment). Leave lines 100-217 unchanged.
- Test (command-level): `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `rust-native` job matrix `${{ matrix.os }}`, the removed `id: py` step output, per-step `env:` mappings
- Produces: `rust-native` block containing no `actions/setup-python`, no `id: py`, no `PYO3_PYTHON`; rustfmt step exactly `cargo fmt --check --all`

- [ ] **1. Write the failing test.**
```bash
cd /Users/tawsifx/paradl
block=$(awk '/^  rust-native:/,/^  media-package:/' .github/workflows/ci.yml)
test -z "$(printf '%s' "$block" | grep -E 'PYO3_PYTHON|steps\.py|PyO3|rapids|paradl-rs')"
printf '%s' "$block" | grep -qF 'cargo fmt --check --all'
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** First `test -z` fails: matches at lines 28-30 (PyO3 0.27 comment), 31 (`name: Set up Python 3.12 for the legacy PyO3 member`), 50/56/66/84/93 (`PYO3_PYTHON`), 69-70 (rapids/paradl-rs fmt-scope comment); second `grep` fails because the fmt step is `-p`-scoped.
- [ ] **3. Implement minimal code.** Delete the setup-python step and its comment (28-35). Remove only the `env:` stanzas carrying `PYO3_PYTHON` from Build, Test, Clippy, Version smoke test, and Distribution steps. Replace the scoped fmt step (including its comment) with `working-directory: rust` + `run: cargo fmt --check --all`. Keep the `--skip full_non_media_gate_is_clean` comment and all media/cross-boundary steps verbatim.
- [ ] **4. Run the test, verify PASS.** Step 1 assertions both succeed; `awk` over `cross-boundary` still shows its unchanged Python setup (intentional).
- [ ] **5. Run the crate/full suite.** From `rust/`: `cargo fmt --check --all && cargo test --all -- --skip full_non_media_gate_is_clean`. This proves the simplified fmt gate is valid across all 18 members and no Rust behavior changed.
- [ ] **6. Commit.** `git commit -m "ci: remove PyO3 scaffolding from rust-native job"`.

## Task 3: Update contributor documentation (root and Rust READMEs)

**Files:**
- Modify: `README.md` (lines 20, 263, 283-295, 303-304)
- Modify: `rust/README.md` (line 128 layout-table row)
- Test (command-level): `README.md`, `rust/README.md`

**Interfaces:**
- Consumes: contributor build commands, engine-name prose, root-stub note
- Produces: no `PYO3_PYTHON`, `rapids`, `paradl-rs`, or "root `pyproject.toml`" claims in either README; media setup commands unchanged

- [ ] **1. Write the failing test.**
```bash
cd /Users/tawsifx/paradl
! grep -qE 'PYO3_PYTHON|rapids|paradl-rs|root `pyproject.toml`' README.md
! grep -qF 'legacy crates retained' rust/README.md
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** README matches at lines 20 and 263 (`rapids` engine), 286 (legacy PyO3 member note), 292-294 (`PYO3_PYTHON` commands), 303-304 (root stub note); `rust/README.md:128` matches the retained-legacy-crates row.
- [ ] **3. Implement minimal code.** README:20 → "Native in-process Rust (`paradl-engine`) engine"; README:263 → "uses the native Rust engine (`paradl-engine`) by default". Replace 283-295 with a short PYO3-free contributor block stating no Python is required for the Rust build and showing plain `cd rust`, `cargo build --all`, `cargo test --all`, `cargo clippy --all -- -D warnings`; keep the `media/` pytest snippet. Delete 303-304 (root files are deleted in Task 4). In `rust/README.md`, delete only the `rapids/`, `paradl-rs/` row. Do not edit flag documentation (Workstream D) or media setup.
- [ ] **4. Run the test, verify PASS.** Both greps find nothing; `grep -n 'cargo build --all' README.md` still shows the updated command block.
- [ ] **5. Run the crate/full suite.** `cd /Users/tawsifx/paradl/rust && cargo test --all -- --skip full_non_media_gate_is_clean` (docs-only change; confirms no fixture references were disturbed).
- [ ] **6. Commit.** `git commit -m "docs: remove legacy PyO3 and rapids contributor guidance"`.

## Task 4: Delete the root Python packaging stubs

**Files:**
- Delete: `pyproject.toml` (root, 2185-byte Hatchling stub), `uv.lock` (root, 83098-byte lock)
- Keep: `.python-version`, `media/pyproject.toml`, `scripts/install-local.sh`, `scripts/install-local.ps1`
- Test (command-level): those paths + `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: media venv commands in `ci.yml:121` and `ci.yml:176`, `scripts/install-local.*`
- Produces: no root Python manifest; media-manifest TOML parses; `.python-version` intact

- [ ] **1. Write the failing test.**
```bash
cd /Users/tawsifx/paradl
test ! -e pyproject.toml && test ! -e uv.lock
test -f .python-version && test -f media/pyproject.toml
python3 -c 'import tomllib; tomllib.load(open("media/pyproject.toml","rb"))'
grep -qF 'media/pyproject.toml' .github/workflows/ci.yml
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** The first assertion fails because both root files exist (the remaining checks already pass and should be recorded as such).
- [ ] **3. Implement minimal code.** `git rm pyproject.toml uv.lock`. Nothing else — in particular do not remove `.python-version` or anything under `media/`, and do not edit the media install steps.
- [ ] **4. Run the test, verify PASS.** All five assertions succeed; `git status --short` shows only the two deletions plus prior staged work.
- [ ] **5. Run the crate/full suite.** `cd /Users/tawsifx/paradl/rust && cargo test --all -- --skip full_non_media_gate_is_clean`; plus re-run the media-manifest parse check from step 1 to prove the real Python manifest survived.
- [ ] **6. Commit.** `git commit -m "chore: delete root Python packaging stubs"`.

## Task 5: Fix the benchmark retirement guards

**Files:**
- Modify: `benchmarks/bench_baseline.py` (line 317)
- Modify: `benchmarks/bench_rust_engine.py` (lines 430, 507, 1047)
- Create: `benchmarks/tests/test_retirement_guard.py` (new tracked test dir; `git check-ignore` confirms `benchmarks/tests/` is not ignored)
- Do not modify: oracle reason strings, `null` row shapes, `HONESTY`/`FRAMEWORK_*` texts, local residue under `src/`

**Interfaces:**
- Consumes: module-level `ORACLE_DIR` (`bench_baseline.py:47-58`, `bench_rust_engine.py:70-79`), `RESULTS_DIR`, and functions `run(tag, only, reps=3)`, `run_benchmark_suite(root)`, `worker(kind, url="")`, `self_test_pin()`
- Produces: guard expression `(ORACLE_DIR / "__init__.py").is_file()` at all four sites; tests pass with no `paradl.*` import attempted

- [ ] **1. Write the failing test.** Create `benchmarks/tests/test_retirement_guard.py` with two tests (short form):
```python
def _residue(tmp_path):
    oracle = tmp_path / "src" / "paradl"
    (oracle / "__pycache__").mkdir(parents=True)
    (oracle / "__pycache__" / "stale.pyc").write_bytes(b"")
    return oracle

def test_baseline_retires_when_only_pycache_remains(tmp_path, monkeypatch):
    import bench_baseline as bb
    monkeypatch.setattr(bb, "ORACLE_DIR", _residue(tmp_path))
    monkeypatch.setattr(bb, "RESULTS_DIR", tmp_path / "results")
    out = bb.run("guard-test", None)
    assert out["status"] == "retired"
    assert out["results"] and all(r["value"] is None for r in out["results"])

def test_rust_engine_retires_when_only_pycache_remains(tmp_path, monkeypatch):
    import bench_rust_engine as br
    monkeypatch.setattr(br, "ORACLE_DIR", _residue(tmp_path))
    out = br.run_benchmark_suite(tmp_path / "bench")
    assert out["retired"] is True
    assert out["benchmarks"]["checksum_large_file"]["value"] is None
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** `cd /Users/tawsifx/paradl && .venv/bin/python -m pytest benchmarks/tests/test_retirement_guard.py -q` → 2 failed with `ModuleNotFoundError: No module named 'paradl...'` (from `bench_baseline._checksum` / `bench_rust_engine.bench_checksum`), because the residue dir satisfies `ORACLE_DIR.is_dir()` and the live import path is taken.
- [ ] **3. Implement minimal code.** Replace all four `if not ORACLE_DIR.is_dir():` with `if not (ORACLE_DIR / "__init__.py").is_file():` — `bench_baseline.py:317`, `bench_rust_engine.py:430` (`self_test_pin`), `:507` (`worker`), `:1047` (`run_benchmark_suite`). Use a literal replacement at each site; no helper, no reason-text edits.
- [ ] **4. Run the test, verify PASS.** The same pytest command → 2 passed. Add one worker assertion if desired: `monkeypatch` + `br.worker("import_aria2")` prints `RESULT_ERROR=oracle retired...` and returns without importing.
- [ ] **5. Run the crate/full suite.** End-to-end smoke against the real residue: `.venv/bin/python benchmarks/bench_baseline.py --tag guard-check` and `.venv/bin/python benchmarks/bench_rust_engine.py --tag guard-check`, both exit 0 with `retired: true`/`ORACLE RETIRED`; then `rm benchmarks/results/guard-check.json` so no new untracked result is left. Also run `cd rust && cargo test --all -- --skip full_non_media_gate_is_clean` to confirm the Rust suite is unaffected.
- [ ] **6. Commit.** `git commit -m "fix(bench): retire harnesses on missing oracle source, not directory"`.

## Task 6: Delete tracked process artifacts and run the final gate

**Files:**
- Delete: `.superpowers/reviews/{task1,task2,task3,task4,wave3-task1,wave3-task3}-package.txt` (6)
- Delete: `docs/master-prompts/{FINAL-rust-framework-with-python-media,full-rust-framework-swarm,full-rust-rewrite-swarm,rust-replace-swarm,channel-count-benchmark-swarm}.md` (5)
- Delete: `benchmarks/results/*.json` (20 tracked files: `after`, `after_mirror_cold`, `after_mirror_disk`, `after_sanitize`, `audit`, `baseline`, `baseline_sanitize`, `diag_checksum`, `diag_classifier`, `diag_mirror_disk`, `diag_parsers`, `final`, `final2`, `final3`, `rust`, `verifyfix`, `verifyfix_before`, `w43_baseline`, `w43_rust_framework`, `wave2`)
- Keep: `docs/superpowers/plans/2026-11-20-final-rust-framework.md` and this plan
- Test (command-level): git index + Tasks 1-5 outputs

**Interfaces:**
- Consumes: `git ls-files` glob results, prior task gates
- Produces: zero tracked files under the three deleted globs; final composite gate green; `git status` shows only intended deletions/additions

- [ ] **1. Write the failing test.**
```bash
cd /Users/tawsifx/paradl
test -z "$(git ls-files '.superpowers/reviews/*' 'docs/master-prompts/*' 'benchmarks/results/*.json')"
test -n "$(git ls-files 'docs/superpowers/plans/*')"
```
- [ ] **2. Run it and verify it FAILS for the expected reason.** First assertion fails: `git ls-files` prints exactly 6 + 5 + 20 = 31 tracked files. Second assertion already passes (plans dir retained).
- [ ] **3. Implement minimal code.** `git rm -r .superpowers/reviews docs/master-prompts` and `git rm benchmarks/results/*.json`. Do not delete `docs/superpowers/plans/`, the local residue, or `.python-version`. Do not "repair" the historical master-prompt reference at `docs/superpowers/framework-charter.md:4` (out of B scope).
- [ ] **4. Run the test, verify PASS.** Both assertions green; `git ls-files` returns empty for all three globs.
- [ ] **5. Run the final gate (full suite + all workstream-B gates).** From `/Users/tawsifx/paradl/rust`:
```bash
cargo build --all
cargo test --all -- --skip full_non_media_gate_is_clean
cargo fmt --check --all
test -z "$(cargo tree --workspace | grep -i pyo3)"                                   # G1
test -z "$(cargo tree --workspace -i base64@0.22.1 | grep -iE 'rapids|paradl-rs')"   # G2
cargo tree --workspace | grep -qE 'syn v[23]\.'                                     # G3: allowed, not a failure
cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; d=json.load(sys.stdin); assert len(d["packages"])==18 and len(d["workspace_default_members"])==16'
```
Then from the repo root: the CI-block grep from Task 2, the README greps from Task 3, `.venv/bin/python -m pytest benchmarks/tests/test_retirement_guard.py -q`, and `git status --short` must contain only intended deletions/additions (no stray `guard-check.json`, no touched residue).
- [ ] **6. Commit.** `git commit -m "chore: remove tracked review packages, master prompts, and benchmark results"`.
