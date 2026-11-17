# Rust Engine Efficiency, Correctness & Full-Pipeline Audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Audit and harden the in-process PyO3 `paradl._rust_engine` (vendored `rapids`) across four independent workstreams: Rust core efficiency/correctness, PyO3 boundary/adapter, full URL→Rust→filesystem pipeline proof, and an honest aria2 comparison harness — with every fix proven RED→GREEN and every diff reviewed.

**Architecture:** Four disjoint-ownership implementers run concurrently in the `feat/rust-rapids` worktree. The lead owns integration, per-diff spec+quality review, the license note, and the final verification gate. No commits on shared branches; nothing committed unless the maintainer asked.

**Tech Stack:** Rust 1.100 / edition 2024 / Tokio / reqwest(rustls) / PyO3 0.27 / maturin 1.16; Python 3.12, pytest, ruff, mypy; aria2c (legacy comparison).

**Spec:** Mission brief (master prompt) + `PARALLEL_DL_DEV_CONTEXT.md` (read in full) + `docs/diagnostics/watchdog-cycle-aria2-bug.md` + `.superpowers/engine-audit-ledger.md`.

## Global Constraints

- Worktree `feat/rust-rapids` only; NEVER touch `main` or `perf/max-efficiency`. BASE = 48379b7.
- Preserve every CLI flag, `ExitCode` (0/1/3/4/130), `TransferResult` fields, JSONL event shapes, shim modules, mirror/metalink/cookie/proxy/ip_family behavior.
- Max 16 connections; all concurrency bounded; all waits have timeouts. No thread/JoinSet leaks.
- Honest benchmarks only: warm-cache hits and ~1.2–1.3× drift are NOT wins. Report medians + spreads; `null` + reason for unfair metrics.
- TDD: every new behavior gets a failing test first (Rust `#[test]` and/or Python `pytest`), watched RED then GREEN.
- Cargo commands run from `rust/` with `PYO3_PYTHON=/Users/tawsifx/paradl/.venv/bin/python`.
- Every diff gets a spec-compliance review AND a code-quality review before acceptance; fix loops stay with the owning agent; cap 5 rounds.
- Do not edit files outside your ownership while other agents run.

## Review Focus

- Close-delimited HTTP/1.1 200 without Content-Length (codeload-like): must not take 30 s (READ_TIMEOUT), must not report false 100% before EOF → S1 + S3 (L1).
- Recycle mid-transfer: committed offsets preserved, URI pool intact, no exit-130 race on terminal handle → S1 + S2.
- Native exception → TransferResult mapping with no exception escaping the Engine protocol → S2.
- CLI flags `checksum`/`cookies` reaching the native call (prior silent-drop bug) → S2 (verify, already patched).
- `recycle_connections()` no-op on terminal handles → S1 + S2.
- Cookie host-scoping across 302 to a different host → S2 + S3.
- SHA-256 mismatch → ExitCode 3; crash resume from `.pdl.part` + `.pdl.json` byte-identical → S1 + S3.

---

### Task 1 (S1): Rust core efficiency & correctness

**Files (owned):** `rust/rapids/**` only.
**Report:** `.superpowers/audit/task1-report.md`

- [ ] Step 1: Fix the 4 baseline clippy findings (download.rs:196, 759, 960; probe.rs:176). Run from `rust/`: `PYO3_PYTHON=... cargo clippy --all -- -D warnings` → exit 0. (`cargo clippy` counts as a TDD verification gate, not new behavior.)
- [ ] Step 2: Root-cause L1 (30 s close-delimited transfer). Add a Rust `#[test]` that reproduces EOF handling with a bounded read timeout on a close-delimited body; watch RED; apply the single minimal fix in `download.rs`; watch GREEN. Record root cause in report (systematic-debugging Phase 1–4).
- [ ] Step 3: Audit planner/state/limiter/verify logic drift vs the original engine originals at `/tmp/archive-ref/crates/engine/src/` (if present); list any drift with file:line in report. Do not blindly port changes.
- [ ] Step 4: Fault-injection tests (Rust `#[test]`): dropped connections; 5xx mirror failover; no-Content-Length EOF completes; recycle mid-transfer; crash-resume from `.pdl.part` + `.pdl.json`; SHA-256 mismatch → error. Verify `py.detach` releases GIL on all long calls in `paradl-rs` (report only — do not edit paradl-rs, that is S2).
- [ ] Step 5: Liveness tests: no hang when all workers retire; `wait(timeout=)` honored; `recycle_connections()` no-op on terminal handles.
- [ ] Step 6: `cargo test --all` (8+ pass) + `cargo clippy --all -- -D warnings` clean. Write report with per-finding evidence.

### Task 2 (S2): PyO3 boundary & adapter efficiency

**Files (owned):** `rust/paradl-rs/**`, `src/paradl/engines/rust_engine.py`, `src/paradl/_rust_engine.py`, `src/paradl/_rust_engine.pyi`.
**Report:** `.superpowers/audit/task2-report.md`

- [ ] Step 1: Measure per-call PyO3 crossing overhead and snapshot poll rate; if poll rate is excessive, batch snapshots. Add a pytest contract test (RED→GREEN) for any change. Report raw numbers.
- [ ] Step 2: Verify `.so` vs shim shadowing rule: with the `.so` present, `is_available() is True`, `version()`/`__version__` correct. Verify with a fresh subprocess test.
- [ ] Step 3: Contract tests that every native exception maps to a `TransferResult`/`ExitCode` (0/1/3/4/130) with NO exception escaping the Engine protocol. Add RED tests for any gap, then GREEN.
- [ ] Step 4: Verify all CLI flags reach the native call — especially `checksum` and `cookies` through `_call_native` (prior silent-drop bug, apparently fixed at rust_engine.py:315-316). Prove with a test that asserts the native mock receives them; fix if RED.
- [ ] Step 5: Verify watchdog decisions reach `recycle_connections()` with cooldown and no post-completion recycle (terminal handle no-op). Add RED→GREEN test for any gap.
- [ ] Step 6: `pytest -q` (full suite), `ruff check src tests`, `mypy src` clean. Write report.

### Task 3 (S3): Full pipeline URL → Rust → filesystem proof

**Files (owned):** `tests/**` only.
**Report:** `.superpowers/audit/task3-report.md`

- [ ] Step 1: For each scenario below, add a TDD test (watch RED then GREEN) asserting final bytes + SHA-256 + sidecar cleanup + JSONL events + `ExitCode`:
  1. direct range-capable URL (16 streams, byte-identical);
  2. GitHub `/archive/` via auto-acceleration (URL routing in accelerate.py — assert mirror chosen; use configured mirror, no real WAN);
  3. mirror failover (500 on primary → success from mirror);
  4. cookie/header/Bearer scoping across redirect (host-scoped cookie NOT leaked cross-host);
  5. resume after interrupt (`.pdl.part` + `.pdl.json` → byte-identical);
  6. metalink/torrent still delegated to aria2 (assert `--engine aria2` / delegation path).
- [ ] Step 2: Fix L2 (loopback server teardown exception traceback) in the shared test server helper — root-cause with systematic-debugging, not a sleep hack. Watch RED→GREEN.
- [ ] Step 3: Verify the L1 fix from S1 through the Python adapter: the cookie-redirect test should drop from ~30 s to ~2 s. Report before/after timing (fresh output).
- [ ] Step 4: Report by NAME any test that skips and why; target 0 unexplained skips. Run full `pytest -q`.
- [ ] Step 5: Write report with per-scenario evidence (test names, byte/SHA-256 assertions, exit codes).

### Task 4 (S4): Honest benchmark harness

**Files (owned):** `benchmarks/bench_rust_engine.py`, `benchmarks/results/**` only.
**Report:** `.superpowers/audit/task4-report.md`

- [ ] Step 1: Extend harness to compare Rust vs REAL aria2 (force legacy `--engine aria2` / pin `get_active_engine`) across: startup, 16-stream throughput (cold + warm median), watchdog recycle latency, CPU usage, peak RSS, large-file checksum cost. Assert the pinned engine actually used (no silent fallback to native).
- [ ] Step 2: Rules: codeload comparison uses same accelerated URL for both engines or state why not; no warm-cache hit as general speedup; separate real wins from ~1.2–1.3× drift; medians + sample spreads; `null` + reason for any unfair metric.
- [ ] Step 3: Re-run 3× to quantify variance; write `benchmarks/results/<tag>.json` with both engines populated or `null`+reason, 3-run variance recorded.
- [ ] Step 4: Write a conclusion in the report: which engine wins each metric, which differences are noise. Exit 0.

### Task 5 (Lead): Integration, review, license, final gate

- [ ] Per-diff: generate review package (`git diff -U10` + new files) under `.superpowers/audit/reviews/`, dispatch reviewer with two explicit verdicts (`SPEC COMPLIANCE pass/fail`, `CODE QUALITY Approved/Needs changes`) + severity-tagged findings with `file:line`. Fix loops with the owning agent; cap 5 rounds; ledger ruling at cap.
- [ ] Broad whole-branch review on the most capable model after all 4 tasks accepted.
- [ ] Final gate: fresh `cargo test --all`, `cargo clippy --all -- -D warnings`, `pytest -q`, `ruff check src tests`, `mypy src`, benchmark exit 0. `git status` reviewed. Nothing committed.

## Success Criteria (acceptable evidence = raw command output)

- `cargo test --all`: all pass, 0 failures; `cargo clippy --all -- -D warnings`: clean.
- `pytest -q`: all pass, 0 unexplained skips; `ruff check src tests`; `mypy src`: clean.
- Pipeline: every S3 scenario byte-identical + correct SHA-256 + ExitCode.
- Benchmarks: JSON with both engines populated or `null`+reason; 3-run variance; written conclusion.
- Efficiency: before/after profile showing any optimization is real, not drift.
- `git status` reviewed; no shared branch touched; nothing committed.
