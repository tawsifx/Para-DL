# Wave-3 Test Suite Centralization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Convert the flat `tests/` directory (19 files, 222 passing tests) into tiered `tests/unit/`, `tests/integration/`, `tests/e2e/` plus a shared `tests/support/` harness, with centralized fakes, zero duplicate/obsolete tests, `pythonpath = ["src"]` (no `sys.path` hacks), and `pytest tests/unit -q` in under 0.5 s.

**Architecture:** Subagent 1 first builds the shared support harness and pytest configuration; then Subagents 2 (pure unit) and 3 (integration/e2e) migrate files in parallel against that fixed harness API. The lead owns the final deletion of legacy files after both migrations are proven green. Tiers inherit the root `tests/conftest.py` autouse fixtures (`isolate_paradl_home`, `reset_isp_cache`).

**Tech Stack:** Python 3.12, pytest 9.2.0, ruff, mypy, `paradl.core.clock.FakeClock`, `unittest.mock`.

**Spec:** Wave-3 mission brief (this conversation) + inventory captured in `.superpowers/wave3-inventory.md`.

## Global Constraints

- Worktree `/Users/tawsifx/paradl-perf`, branch `perf/max-efficiency`. Do not touch `main` source. BASE `ab2629b`.
- Python: `/Users/tawsifx/paradl/.venv/bin/python`. Do NOT create commits (lead commits).
- **Zero coverage regression.** No unique behavioral assertion, security check (archive path-traversal, cookie jar `0600`, quarantine preservation, API slot concurrency), CLI exit-code test, or Wave-1/Wave-2 regression guard may be dropped. `tests/test_perf_parity.py` and `tests/test_config_shim.py` are PRESERVE-EXACT files (may move, must not lose/alter assertions).
- **No `sys.path` hacks.** `pyproject.toml` must set `testpaths = ["tests"]`, `pythonpath = ["src"]`, `python_files = ["test_*.py"]`, `markers = ["slow: end-to-end or multi-second transfer tests"]`. Root conftest must drop its `sys.path.insert`.
- All migrated/new test files pass `ruff check src tests` and `mypy src`.
- No real WAN access: support socket guard is loopback-only and autouse.
- Target: `pytest tests/unit -q` reported duration **< 0.5 s** (calibration: 80 legacy unit tests run in 0.24 s + ~0.24 s startup).
- Every test basename must be unique across all tiers (pytest prepend import mode); `tests/support/` is a package with `__init__.py`.

## Verified Inventory (Phase 1)

19 files / 222 tests. Per-file counts: `unit_test_config` 31, `unit_test_watchdog` 27, `unit_test_net` 25, `unit_test_core` 22, `unit_test_aria2` 22, `test_bug_fixes` 20, `test_wave2_classifier` 17, `test_isp_detection` 12, `test_platform_windows` 9, `test_image_probe_and_http_errors` 6, `test_perf_parity` 5, `test_wave2_sqlite` 5, `test_wave2_concurrency` 5, `test_wave2_aria2` 5, `test_platform_unification` 4, `test_engines_windows` 4, `test_pypi_transitive` 2, `test_config_shim` 1.

Important findings that shape the plan:
- **No `http.server`/socket servers exist in current tests.** `tests/support/server.py` is new reusable infrastructure for future loopback integration tests, not replacement of duplicates. Do not fabricate usages.
- **No duplicate test function names** exist across files. Dedup is about overlapping *assertions*, not names.
- `paradl.cli` is now a **package** (`src/paradl/cli/{__init__,main,parser,dispatch}.py`); imports in `test_bug_fixes.py` and `test_perf_parity.py` are valid and must stay. The deleted monolith is `src/paradl/cli.py` (file).
- `paradl.core.clock` already provides `Clock`, `FakeClock`, `get_clock`. Reuse it; do not invent a second clock.
- Slow calls: `test_platform_unification.py::...doctor_audits_windows_utilities_on_windows` 0.45 s; `test_bug_fixes.py::test_r1_probe_requests_identity_encoding` 0.31 s; `test_wave2_aria2.py::test_aria2_has_no_mypy_daemon_attribute_error` 0.26 s (spawns a real `mypy` subprocess); concurrent dispatch 0.15 s. These must land in integration/e2e, not unit.
- The only real `time.sleep()` calls are 0.02 s race-window wideners in `test_wave2_aria2.py` singleton tests; those are concurrency tests → integration tier, not FakeClock-able.

## Review Focus

- **Lost assertion:** a merged/parameterized test silently drops an input/output pair → dedup manifest in each report lists every absorbed assertion; reviewer cross-checks counts.
- **Broken monkeypatch target:** moving a test to a subdirectory changes `__module__` and can break `patch("paradl....")` resolution or relative paths → verify each moved file imports by absolute module path and uses `Path(__file__).resolve().parents[...]` correctly.
- **Fixture scoping:** root autouse fixtures must still apply to all tiers; a nested `conftest.py` must not shadow them.
- **`pythonpath` regression:** `pytest -q` must pass with NO `PYTHONPATH` env var set.
- **Guard erosion:** the 5 `test_perf_parity.py` guards + `test_config_shim.py` guard still exist and pass after migration.
- **Subprocess guard blocking legitimate mypy test:** the subprocess guard must be opt-in (named fixture), never autouse, or the mypy guard test breaks.

## Rulings

- Parallel implementers authorized for Tasks 2/3 despite subagent-driven-development's serial default: write scopes are disjoint by directory (`tests/unit/` vs `tests/integration/`+`tests/e2e/`); the only shared source (`test_bug_fixes.py`) is split by an explicit test-name list in this plan. Cost if wrong: lead reconciles.
- `test_bug_fixes.py` split: Subagent 2 moves 19 tests to `tests/unit/test_bug_fixes.py`; Subagent 3 moves `test_r1_probe_requests_identity_encoding` to `tests/integration/test_probe_identity.py`; **lead deletes the original only after both report DONE**.
- No obsolete tests targeting removed pre-v1.0.0 code were found; subagents must FLAG suspected obsolete tests to the lead rather than delete them unilaterally.
- `tests/support/server.py` and `macos_fake.py` are built as reusable harnesses but not retro-fitted into tests that use pure mocks (nothing to consolidate). One new e2e smoke test will exercise them.

---

### Task 1: Shared Support Harness & Pytest Configuration

**Files:**
- Modify: `tests/conftest.py`, `pyproject.toml`
- Create: `tests/support/__init__.py`, `tests/support/fakes.py`, `tests/support/server.py`, `tests/support/macos_fake.py`, `tests/support/guards.py`, `tests/support/golden/.gitkeep`, `tests/smoke_support.py` (temporary proof test — deleted by lead before final verification)
**Interfaces (produced — Tasks 2/3 import these):**
- `from tests.support.fakes import FakeClock, FakeKeychain, FakeAria2Server, make_target_info`
- `from tests.support.server import ThrottleHTTPServer, throttle_server` (worker-scoped fixture; loopback only, `127.0.0.1:0`)
- `from tests.support.macos_fake import FakeMacOSRunner` (deterministic scutil/pmset/caffeinate/codesign/xattr outputs; golden strings under `tests/support/golden/`)
- `from tests.support.guards import forbid_wan_sockets, forbid_untracked_subprocess` (both are **named** fixtures except `forbid_wan_sockets` which is autouse via conftest registration)

- [ ] **Step 1:** Write a failing smoke test `tests/smoke_support.py` that imports all four support modules and asserts (a) `FakeClock` advances time deterministically and matches `paradl.core.clock.FakeClock` semantics, (b) `throttle_server` serves a 206/range response over loopback, (c) `FakeMacOSRunner` returns a golden `pmset -g batt` output, (d) `forbid_wan_sockets` blocks a non-loopback connect attempt.
- [ ] **Step 2:** Run it; confirm RED (ModuleNotFoundError).
- [ ] **Step 3:** Implement support modules. `FakeClock` must subclass/wrap `paradl.core.clock.FakeClock` (single source of truth). `FakeAria2Server` provides `tell_status`/`get_servers`/`get_global_stat` fakes suitable for aria2 tests. `FakeKeychain` mimics keyring get/set/delete in memory. `ThrottleHTTPServer` uses `http.server.ThreadingHTTPServer` bound to `127.0.0.1:0`, worker-scoped via fixture, with a context-managed shutdown. `FakeMacOSRunner` returns fixed strings, never invokes real subprocesses. `forbid_wan_sockets` is autouse-registered in root conftest and blocks `socket.connect` to non-loopback addresses via monkeypatch; `forbid_untracked_subprocess` is opt-in and asserts subprocess calls are on an allowlist.
- [ ] **Step 4:** Update `tests/conftest.py`: remove `sys.path` block; keep `isolate_paradl_home` and `reset_isp_cache` autouse fixtures; keep (and re-export where useful) `mock_httpx_client`, `mock_aria2_daemon`, `sample_target_info`; wire the autouse socket guard.
- [ ] **Step 5:** Update `pyproject.toml` `[tool.pytest.ini_options]` with exact keys from Global Constraints.
- [ ] **Step 6:** Run smoke test GREEN; run `env -u PYTHONPATH /Users/tawsifx/paradl/.venv/bin/python -m pytest -q` and confirm all 222 tests still pass with NO `PYTHONPATH`.
- [ ] **Step 7:** Report to lead; include final `pytest -q` count and confirmation that `pythonpath` works.

### Task 2: Pure Unit Migration & Deduplication

**Files:**
- Create: `tests/unit/test_models.py`, `test_config.py`, `test_config_shim.py` (PRESERVE-EXACT), `test_perf_parity.py` (PRESERVE-EXACT), `test_aria2.py`, `test_net.py`, `test_watchdog.py`, `test_classifier.py`, `test_pypi_transitive.py`, `test_platform_windows.py`, `test_engines_windows.py`, `test_bug_fixes.py` (19 tests only)
- Delete after proven: `tests/unit_test_*.py` (5 files), `tests/test_wave2_classifier.py`, `tests/test_pypi_transitive.py`, `tests/test_platform_windows.py`, `tests/test_engines_windows.py`, `tests/test_config_shim.py`, `tests/test_perf_parity.py` — **lead performs physical deletion**, implementer creates new files and reports a manifest.
**Interfaces:** consumes Task 1 support fixtures; produces flat unit tier that runs < 0.5 s.

- [ ] **Step 1:** Write a failing dedup-parity test file `tests/unit/test_dedup_manifest.py` (temporary, lead deletes) asserting that merged parameterized cases equal the union of the old assertions (enumerate exact input/output pairs found in the source files).
- [ ] **Step 2:** Map (exact, no exceptions):
  - `unit_test_core.py` → `tests/unit/test_models.py`
  - `unit_test_config.py` → `tests/unit/test_config.py`
  - `unit_test_aria2.py` → `tests/unit/test_aria2.py`
  - `unit_test_net.py` → `tests/unit/test_net.py`
  - `unit_test_watchdog.py` → `tests/unit/test_watchdog.py`
  - `test_wave2_classifier.py` → `tests/unit/test_classifier.py`
  - `test_pypi_transitive.py` → `tests/unit/test_pypi_transitive.py`
  - `test_platform_windows.py` → `tests/unit/test_platform_windows.py`
  - `test_engines_windows.py` → `tests/unit/test_engines_windows.py`
  - `test_config_shim.py` → `tests/unit/test_config_shim.py` (byte-for-byte assertions preserved)
  - `test_perf_parity.py` → `tests/unit/test_perf_parity.py` (byte-for-byte assertions preserved)
  - `test_bug_fixes.py` → all tests EXCEPT `test_r1_probe_requests_identity_encoding` → `tests/unit/test_bug_fixes.py`
- [ ] **Step 3:** Deduplicate ONLY verified overlaps, with proof in the report:
  - `TestWatchdogModes::test_values` (aria2) + `TestWatchdogModes::test_watchdog_mode_values` (core) → one parameterized test over `WatchdogMode` values.
  - `test_format_size` (core) + `test_l3_format_size` (bug_fixes) → one parameterized test retaining every input/output pair.
  - Do NOT merge: config default schema test vs config-shim re-export test; classifier mirror-dedup tests vs perf-parity mirror tests (different functions); zone-identifier case-insensitivity vs ADS handling.
- [ ] **Step 4:** Replace ad-hoc `_FakeClock` in `test_wave2_aria2.py` usages (if any land in unit) with `tests.support.fakes.FakeClock`. Preserve all `monkeypatch`/`patch` targets as absolute module paths.
- [ ] **Step 5:** Run `pytest tests/unit -q` → target < 0.5 s; `pytest -q` → all 222 tests (legacy files still present until lead deletes) still green; ruff + mypy clean.
- [ ] **Step 6:** Report with a dedup manifest (old test → new parameterized case) and final unit count/timing.

### Task 3: Integration & E2E Consolidation

**Files:**
- Create: `tests/integration/test_sqlite_guard.py`, `test_concurrency.py`, `test_aria2_rpc.py`, `test_isp_detection.py`, `test_platform_unification.py`, `test_probe_identity.py`, `test_engine_batch.py` (from `test_image_probe_and_http_errors.py`), `tests/e2e/test_slow_guards.py`
- Delete after proven: `tests/test_wave2_sqlite.py`, `tests/test_wave2_concurrency.py`, `tests/test_wave2_aria2.py`, `tests/test_isp_detection.py`, `tests/test_platform_unification.py`, `tests/test_image_probe_and_http_errors.py` — **lead performs physical deletion**; `tests/test_bug_fixes.py` handled per Rulings.
**Interfaces:** consumes Task 1 support harness; all `@pytest.mark.slow` tests land in `tests/e2e/`.

- [ ] **Step 1:** Map exactly:
  - `test_wave2_sqlite.py` → `tests/integration/test_sqlite_guard.py` (5)
  - `test_wave2_concurrency.py` → `tests/integration/test_concurrency.py` (5) — these are the API slot race + HF/PyPI order guards; do not weaken.
  - `test_wave2_aria2.py` → `tests/integration/test_aria2_rpc.py` (5). Move `test_aria2_has_no_mypy_daemon_attribute_error` to `tests/e2e/test_slow_guards.py` marked `@pytest.mark.slow` (it spawns a real mypy subprocess).
  - `test_isp_detection.py` → `tests/integration/test_isp_detection.py` (12)
  - `test_platform_unification.py` → `tests/integration/test_platform_unification.py` (4). Mark `test_doctor_audits_windows_utilities_on_windows` `@pytest.mark.slow` and move it to `tests/e2e/test_slow_guards.py` (0.45 s).
  - `test_image_probe_and_http_errors.py` → `tests/integration/test_engine_batch.py` (6)
  - `test_bug_fixes.py::test_r1_probe_requests_identity_encoding` → `tests/integration/test_probe_identity.py` (1)
- [ ] **Step 2:** Rewire any duplicated inline mocks to Task 1 support fakes where they map cleanly (do NOT force-fit: existing tests use `unittest.mock` and must keep identical behavior). At minimum, add one new e2e test in `tests/e2e/test_slow_guards.py` that drives a full watchdog recycle workflow end-to-end using `FakeClock` (zero real sleeps) and exercises `FakeAria2Server`.
- [ ] **Step 3:** Verify module-scoped/session-scoped fixtures do not weaken `PARADL_HOME` isolation — the root autouse `isolate_paradl_home` must remain per-test.
- [ ] **Step 4:** Run `pytest tests/integration tests/e2e -q`, `pytest -m "not slow" -q`, `pytest -q` (all 222 + 1 new e2e = 223), ruff + mypy clean.
- [ ] **Step 5:** Report mapping + any coverage concern flagged for lead.

### Task 4 (Lead): Deletion, Speed Pass & Verification

- [ ] Delete legacy flat files after both implementers report DONE and counts reconcile to 223 (or documented dedup-adjusted count).
- [ ] Run `pytest --durations=15` and eliminate any remaining needless sleeps/DB startups in unit/integration.
- [ ] Invoke `verification-before-completion`; run fresh: `pytest tests/unit -q` (< 0.5 s), `pytest -m "not slow" -q`, `pytest -q`, `ruff check src tests`, `mypy src`.
- [ ] Update `.superpowers/wave3-ledger.md` and the dev-context doc; produce the final plain-English summary.
