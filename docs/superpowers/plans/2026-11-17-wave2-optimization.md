# Wave-2 Multi-Subagent Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute the remaining ranked backlog from Section 7 of `PARALLEL_DL_DEV_CONTEXT.md` inside the existing `perf/max-efficiency` worktree, with measured, parity-preserving wins.

**Architecture:** Four implementation subagents operate concurrently on strictly disjoint file sets. The lead integrates, reviews each diff for spec compliance and code quality, fixes the non-owned mypy errors, then runs the full verification suite and benchmark in isolation. SQLite schema initialization is guarded once per process; aria2 polling caches per-tick RPC results and per-transfer state; independent multi-file downloads become bounded-concurrent; the API slot counter becomes atomic.

**Tech Stack:** Python 3.12, pytest, ruff, mypy, stdlib `cProfile`/`tracemalloc`, `benchmarks/bench_baseline.py`.

**Spec:** `/Users/tawsifx/paradl/PARALLEL_DL_DEV_CONTEXT.md` §7 "Remaining ranked findings" (the referenced `_2.md` file does not exist).

## Global Constraints

- Worktree: `/Users/tawsifx/paradl-perf`, branch `perf/max-efficiency`. Do not touch `main` source.
- Python: `PYTHONPATH=$PWD/src /Users/tawsifx/paradl/.venv/bin/python`.
- Do NOT repeat Wave-1 wins (`core/models.py`, mirrors parse cache, `Aria2DaemonPool`, deleted `cli.py`).
- Preserve all backward-compatible shims (`auth.py`, `clock.py`, `config.py`, `doctor.py`, `events.py`, `history.py`, `hooks.py`, `mirrors.py`, `models.py`, `netprobe.py`, `paths.py`, `queue.py`, `server.py`, `toolchain.py`, `tui.py`).
- Preserve public APIs, CLI flags, exit codes, output parity, and SHA-256/`HF_TOKEN` verification.
- Bound all new concurrency with the existing `<= 16` global connection cap and explicit timeouts.
- TDD mandatory: write failing test first, watch it fail, then implement.
- No warm-cache-hit microbenchmark may be quoted as a general speedup.
- Baseline at plan time: `63 passed`, ruff clean, mypy 4 errors (`ui/__init__.py:33,40`, `engines/aria2.py:1359`, `tui.py:8`). The `aria2.py:1359` error is a Wave-1 regression owned by Subagent 2.

## Review Focus

- Reopened/replaced SQLite file: schema guard keyed by `(path, st_dev, st_ino)` must reinitialize. Test: create DB, close, delete, reopen, assert tables present.
- Concurrent API task bursts: active count must never exceed max. Test: N simultaneous `POST /tasks`, assert peak <= cap.
- Non-Range single streams: watchdog must not socket-cycle. Test: decision action is `NONE`.
- `HF_TOKEN` / SHA-256 parity under concurrent downloads: all workers must still verify. Test: inject fake verifier, assert call count == file count.
- Collected test count: enabling `unit_test_*.py` may expose import failures. Test: full `pytest -q` green after config change.

## Rulings

- Parallel implementers authorized despite subagent-driven-development's serial default: file ownership is disjoint by construction; no shared writable file exists across tasks. Cost if wrong: merge conflicts, resolved by lead.
- `PARALLEL_DL_DEV_CONTEXT_2.md` missing: use the existing `PARALLEL_DL_DEV_CONTEXT.md` §7 as the spec.
- Lead owns `ui/__init__.py` and `tui.py` mypy fixes (outside all four scopes) after subagents finish.

---

### Task 1: SQLite Schema Guard & Batched Persistence

**Files:** `src/paradl/storage/history.py`, `src/paradl/throttle/store.py`, new tests in `tests/`.
**Interfaces:** Produces `HistoryManager` connection reuse preserving all public classmethods; `HISTORY_SCHEMA_VERSION = 4` unchanged.
- [ ] Step 1: Write DB-dump parity + reopen-after-recreate failing tests.
- [ ] Step 2: Run them, confirm RED.
- [ ] Step 3: Implement thread-safe `(resolved_path, dev, inode)` guard in `_open_and_init`; guard `ThrottleStore._conn()` DDL; single connection + single commit for batched profile saves.
- [ ] Step 4: Run tests GREEN; full `pytest -q`, ruff, mypy on owned files.
- [ ] Step 5: Report findings + test output to lead.

### Task 2: Aria2 Poll Loop, RPC Dedup & Thread-Safe Singletons

**Files:** `src/paradl/engines/aria2.py`, `src/paradl/net/client.py`, new tests in `tests/`.
**Interfaces:** Consumes Task 1's `HistoryManager` API unchanged. Produces cached per-tick RPC result map; locked `get_http_client()` / `get_daemon_pool()`.
- [ ] Step 1: Write failing tests: (a) one `tellStatus`/`getServers` call per GID per tick, (b) concurrent singleton init yields one instance, (c) cached `ytdlp_extractor_claims_url` patterns.
- [ ] Step 2: Confirm RED.
- [ ] Step 3: Implement per-tick caches, per-transfer host-strategy cache, double-checked locking, extractor cache. Fix the `aria2.py:1359` mypy error from Wave 1.
- [ ] Step 4: Tests GREEN; full `pytest -q`, ruff, mypy on owned files.
- [ ] Step 5: Report.

### Task 3: Concurrent Multi-File Downloads & API Slot Safety

**Files:** `src/paradl/engines/hf.py`, `src/paradl/engines/pypi.py`, `src/paradl/api/server.py`, new tests in `tests/`.
**Interfaces:** Must preserve `DownloadResult` ordering and `HF_TOKEN`/SHA-256 verification parity; cap concurrency at the existing global limit (16).
- [ ] Step 1: Write failing concurrent race test for API slot dispatch + parity tests for ordered multi-file results.
- [ ] Step 2: Confirm RED.
- [ ] Step 3: Implement bounded `ThreadPoolExecutor` fan-out with `f.result()` ordering; atomic slot increment under lock before thread start.
- [ ] Step 4: Tests GREEN; full `pytest -q`, ruff, mypy on owned files.
- [ ] Step 5: Report.

### Task 4: Classifier, Dedup, Test Discovery & Docs/CI Drift

**Files:** `src/paradl/throttle/classifier.py`, `src/paradl/storage/mirrors.py` (only the three listed dedup sites), `pyproject.toml`, `README.md`, `.github/workflows/ci.yml`, `tests/unit_test_*.py`.
**Interfaces:** `next_best_hours` output byte-identical; mirror URL list order and contents identical.
- [ ] Step 1: Write failing parity tests for `next_best_hours` and mirror dedup ordering.
- [ ] Step 2: Confirm RED.
- [ ] Step 3: Compute next-best-hours directly from samples (no discarded coverage matrix); set-based dedup at the three sites; enable `unit_test_*.py` collection and fix resulting import/collection failures; reconcile `pdl` alias docs; point CI at `benchmarks/bench_baseline.py`.
- [ ] Step 4: Tests GREEN; full `pytest -q`, ruff.
- [ ] Step 5: Report, including final collected test count.

### Task 5 (Lead): Integration & Verification

- [ ] Review each subagent diff for spec compliance and code quality.
- [ ] Fix non-owned mypy errors (`ui/__init__.py`, `tui.py`).
- [ ] Run isolated benchmark: `bench_baseline.py --tag wave2 --compare benchmarks/results/baseline.json`.
- [ ] Run fresh `pytest -q`, `ruff check src tests`, `mypy src`; record raw output.
- [ ] Update Section 7 of the dev-context doc with Wave-2 results; summarize honestly.
