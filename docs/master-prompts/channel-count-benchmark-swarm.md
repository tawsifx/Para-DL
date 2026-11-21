# MASTER PROMPT — Local-Network Optimal Channel-Count Benchmark

> Give this to a fresh agent session. It is self-contained: the agent must not ask "what next"; make rulings in a ledger and keep going. Only stop for irreversible/shared-branch git operations, a decision only the maintainer can make (target URL / lifting the connection cap), or a plan so broken every path is a guess.
>
> **Execution model:** this is deliberately designed for **many parallel subagent swarms across several waves** (Wave A discovery → Wave B implementation → Wave C measurement/analysis), orchestrated with `dispatching-parallel-agents` + `subagent-driven-development`, with a two-verdict review after every diff. Do not collapse it into a single agent doing everything sequentially — use the swarms as specified in §3.

You are a **Principal Performance Engineer** running a benchmark study to answer one question:

**On the maintainer's actual local network, what HTTP range-channel count (1, 2, 4, 8, 16, 32, 64) gives maximum throughput for the native Rust `rapids` engine — and does throughput keep scaling past 16?**

This must be a **real measurement on the real network**, not a synthetic/fake loopback test. "Local network" here means traffic that actually traverses the maintainer's LAN/ISP path (e.g. a file served by another machine on the LAN, accessed via its LAN IP — *not* `127.0.0.1`/loopback — and/or a real reachable WAN URL the maintainer approves). Loopback-only numbers may be reported as a secondary control, but the recommendation must come from real-path data.

---

## STEP ZERO — read before anything else (authoritative background)

1. `/Users/tawsifx/paradl/PARALLEL_DL_DEV_CONTEXT.md` — read in full. Especially §7 (perf audit), the Wave-1/2/3 results, the `mdm-engine`→`rapids` migration, and the honest-benchmark caveats (warm-cache hits and ~1.2–1.3× drift are NOT wins).
2. `/Users/tawsifx/paradl/README.md` — current CLI, `--engine {rust,aria2}`, `PARADL_GITHUB_ARCHIVE_MIRROR`.
3. Source of truth for the cap you must work around:
   - `src/paradl/engines/rust_engine.py:51` `MAX_CONNECTIONS = 16`, and `safe_conn = min(max(int(connections),1), MAX_CONNECTIONS)` (`:509`, `:851`).
   - `rust/rapids/src/download.rs:1016` `connections: ... opts.connections.clamp(1, 16)`.
   - `rust/paradl-rs/src/lib.rs:84` `max_connections.unwrap_or(16).clamp(1,16)` and worker threads `max_connections.clamp(1,8)`.
   - `src/paradl/engines/aria2.py:875,1733` also cap at 16.
4. Existing harnesses: `benchmarks/bench_rust_engine.py` (engine pinning, throughput, `--runs`, `--tag`), `benchmarks/bench_baseline.py`, and diagnostics `src/paradl/diagnostics/{bench,netprobe}.py`.
5. Git state: worktree `/Users/tawsifx/paradl`, branch `main`. Record `BASE=$(git rev-parse HEAD)`. Use an isolated worktree for all edits (native `worktree_create` or `git worktree add`); never touch other branches.

Confirm in a ledger (`.superpowers/channel-benchmark-ledger.md`) that you understand the cap and the honesty caveats before dispatching anything.

---

## 0. The central engineering problem — the 16-connection cap

The engine **hard-clamps to 16 in three places**. To measure 32/64 you must lift the cap *without shipping that change*. Rulings you must make and record:

- **Preferred:** add a benchmark-only escape hatch gated by an explicit opt-in that is NOT reachable from normal CLI use — e.g. a Rust `#[cfg(feature = "bench-high-concurrency")]` feature or a dedicated benchmark entry point that constructs `StartOptions` with `connections=32/64`, plus a Python harness that calls `_rust_engine.start_download(...)` directly (bypassing the adapter's `MAX_CONNECTIONS` clamp). The Tokio worker-thread count (`clamp(1,8)`) must also be lifted for the test, or it becomes a hidden bottleneck — record the exact value used.
- **Never** raise the cap in the shipped default path, `--engine aria2`, or the public CLI as part of this task. Revert any such change before finishing; the deliverable is data, not a product change.
- Prove actual concurrency: instrument the test server to count **simultaneous open connections** and **distinct `Range` chunks served**, and assert the observed peak matches the requested count. A run that *asks* for 64 but only opens 16 is invalid — discard it and fix the harness.

Ledger every decision as `Ruling: X — why — cost if wrong`.

---

## 1. MANDATORY SKILLS (obra/superpowers) — invoke and obey, do not improvise

Load these via the `skill` tool at the start, in this order, and follow each exactly. If a skill has a checklist, create a todo per item. Track everything in `.superpowers/channel-benchmark-ledger.md`.

**Process skills (set the approach — load first):**
1. `using-superpowers` — establishes that skill invocation comes before any response/action; never skip a skill because a task "looks simple".
2. `brainstorming` — before designing the harness, surface assumptions (cap sites, real-path target, link ceiling) and present the target choice as a maintainer decision.
3. `systematic-debugging` — for ANY surprising result (throughput drop at 32/64, cap not lifted, server saturation, inconsistent reps): Phase 1 root cause (read errors, reproduce, gather evidence at each boundary), Phase 2 pattern, Phase 3 single hypothesis + minimal test, Phase 4 single fix. No guessing, no sleep hacks, no "rerun and hope".
4. `test-driven-development` — the harness itself needs tests first (peak-concurrency reporting from a synthetic server, null+reason recording, pinning). Watch RED, then GREEN. No production/measurement code without a test that failed first.
5. `writing-plans` — write `docs/superpowers/plans/<date>-channel-count-benchmark.md` (file-by-file, exact commands, exact channel set, success criteria) **before** writing measurement code.

**Isolation & orchestration skills:**
6. `using-git-worktrees` — verify/create an isolated workspace; record `BASE=$(git rev-parse HEAD)`; never work on `main` directly.
7. `dispatching-parallel-agents` — group the work into independent problem domains and dispatch them concurrently (below); one agent per domain; each gets a focused, self-contained prompt, strict scope, and a report path.
8. `subagent-driven-development` — fresh implementer per task, a task review (spec compliance + code quality) after each diff, fix loops staying with the owning agent (cap 5 rounds), then a broad whole-branch review on the most capable model. Never dispatch multiple implementers to the SAME file.
9. `requesting-code-review` / `receiving-code-review` — formal two-verdict reviews (`SPEC COMPLIANCE pass/fail`, `CODE QUALITY Approved/Needs changes`) with severity-tagged findings and `file:line`; technical rigor when receiving findings, not performative agreement.

**Completion skill:**
10. `verification-before-completion` — never claim a number, a win, or "done" without fresh, complete command output in the current message. Evidence before assertions, always.

**Optional supporting skills if they apply:** `karpathy-guidelines` (surface assumptions, surgical changes), `finishing-a-development-branch` (only when the maintainer wants to integrate), `find-skills` (if a capability gap appears). Load any skill you think has even a 1% chance of applying.

---

## 2. PHASE 1 — discovery & target (lead + subagents, parallel)

- Snapshot baseline: `git rev-parse HEAD`, `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, existing benchmark exit status. Save raw output under `.superpowers/channel-bench/baseline.txt`.
- Inventory the three cap sites above and the Tokio worker-thread setting; draw the exact call path from CLI → adapter → native for `connections=64`.
- **Target selection (decision only the maintainer can make):** the real-path endpoint and payload size. Ask the maintainer for: a LAN file URL served by another machine (LAN IP, *not* loopback) and/or an approved real WAN URL, plus a large file (≥256 MiB) with `Accept-Ranges: bytes`. If not supplied, stop and ask — do **not** substitute loopback and call it the local network. Record the chosen target, its bandwidth ceiling (link speed), and how you verified it serves 206 responses.
- Produce the plan with the task map and success criteria before Phase 2.

---

## 3. PHASE 2 — multi-wave subagent swarms (disjoint file ownership)

Run this as **several waves of parallel subagent swarms**, not three isolated agents. Wave N+1 starts only after Wave N's diffs pass their spec + quality reviews. Each agent writes a report to `.superpowers/channel-bench/taskN-report.md` and returns only a short summary (status, files changed, one-line test result, concerns). Self-contained prompts: give each agent its scope, the relevant STEP ZERO facts, the exact channel set, and the global constraints — do NOT paste the whole session history.

**Global constraints for every agent:** max 16 is a shipped default — do not change it; only the gated benchmark path may exceed it; prove real concurrency with server-side counters; honest numbers only (drift ≠ win); TDD RED→GREEN; disjoint file ownership (no two agents edit the same file).

### Wave A — discovery & design (dispatch in parallel)
- **A1 — Cap-call-path mapper** (read-only + ledger): trace CLI → adapter → native for `connections=64`, enumerate all three clamp sites + Tokio worker-thread cap, document the minimal gated escape hatch and how to revert it. Report exact file:line.
- **A2 — Real-path target analyst** (read-only): verify the maintainer's chosen endpoint serves 206/Range, measure the link ceiling (link speed / a known baseline transfer), and document how to prove traffic traverses the real interface (LAN IP, not loopback).
- **A3 — Harness spec writer**: draft the exact harness interface (CLI flags, JSON schema, metrics: cold/warm median, spread, CPU, RSS, observed peak connections, distinct ranges) without implementation code.
- **A4 — Test-plan author**: enumerate the RED→GREEN tests the harness needs (concurrency reporting, null+reason, pinning, cap-revert verification).

### Wave B — implementation (dispatch in parallel, disjoint files)
- **B1 — Native gated cap lift** (`rust/paradl-rs/**`, `rust/rapids/src/**` ONLY behind `#[cfg(feature="bench-high-concurrency")]`): a feature-gated entry point that accepts 32/64 and raises the Tokio worker threads; a test proving the default build still clamps at 16.
- **B2 — High-concurrency harness** (`benchmarks/harness/**`): drives the native engine at 1/2/4/8/16/32/64 on the real target via the gated path; warm-up, ≥3 reps, cold/warm separation, medians + min/max/stdev, CPU/RSS sampling.
- **B3 — LAN server observer** (`benchmarks/support/**`): LAN-reachable file server bound to the LAN IP (loopback only as labeled control); per-request range/concurrency/bytes counters; JSONL observations; TDD tests (RED→GREEN).
- **B4 — Adapter-bypass validator** (`tests/**` for harness tests only): a test that `RustEngine.execute(connections=64)` through the normal path still clamps to 16 while the gated path reaches 64.

### Wave C — measurement & analysis (dispatch in parallel)
- **C1 — Real-path run operator** (owns `benchmarks/results/raw/**`): executes the harness on the real target, 3 full sweeps, archives raw JSON + server JSONL; no editing of harness code.
- **C2 — Curve/knee analyst** (owns an analysis script + `benchmarks/results/channel-counts.json`): consumes raw JSON, produces the channel-vs-throughput curve, classifies real vs drift (≥1.2–1.3× band + spreads), finds the knee and any 32/64 regression, writes `null`+reason for unmeasurable metrics.
- **C3 — Honesty reviewer** (read-only + a review report): audits every claimed win against the drift rule and warm-cache caveat; verifies both engines/all counts are present or `null`+reason; flags any loopback number presented as a real-path result.
- **C4 — Reproduction-doc writer** (owns `docs/superpowers/plans/**` for a runbook): exact re-run commands, target setup, and how to verify concurrency/cap-revert.

**Lead** owns integration, per-diff spec + quality review (two explicit verdicts, severity-tagged findings with `file:line`), the maintainer decision, arbitration at the 5-round cap (ledger ruling), and the final gate. Do not edit subagent-owned files while they run.

---

## 4. PHASE 3 — review loop (per task, before acceptance)

For each returned diff (in every wave): generate a review package (`git diff -U10` + new-file contents) under `.superpowers/channel-bench/reviews/taskN.txt`; dispatch a reviewer that must output `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes` with severity-tagged findings (`Critical/Important/Minor`) and `file:line`. Failed → fix loop with the SAME owning agent (resume its session); re-review only the fix diff (scoped); cap 5 rounds, then adjudicate with a ledger `Ruling: X — why — cost if wrong`. Do not start the next wave until the current wave's diffs are accepted. After all waves: one broad whole-branch review on the most capable available model (e.g. the largest reasoning model from `opencode.models`), triaging every deferred/parked finding in the ledger.

---

## 5. SUCCESS CRITERIA (acceptable evidence = raw command output)

- Harness proves **actual** concurrency at each requested count (server-side peak connections / distinct ranges), not a clamped 16.
- Real-path endpoint traversed (LAN IP or approved WAN), documented with how you verified 206/Range support and link ceiling. Loopback results labeled as control only.
- For each count in {1,2,4,8,16,32,64}: median throughput + spread, cold and warm reported separately, CPU, peak RSS, and observed concurrency. ≥3 reps per count.
- `benchmarks/results/channel-counts.json` with all counts populated or `null`+reason, plus 3-run variance.
- Efficiency claim only if backed by before/after numbers that exceed drift; warm-cache hits never presented as wins.
- Shipped code still clamps at 16; a verification command proves it. No product behavior change committed.
- `pytest -q`, ruff, mypy, cargo tests/clippy all green after the benchmark-only changes.
- `git status` reviewed; nothing committed on shared branches; no push unless the maintainer asked.

---

## 6. FINAL OUTPUT (plain English)

1. **TL;DR** — 3–5 bullets: the recommended channel count on the maintainer's real network, the measured speed at that count vs 16, and whether >16 helps at all.
2. **Benchmark table** — counts 1→64 vs median MB/s (cold/warm), spread, CPU, RSS, observed concurrency; mark drift vs real.
3. **Curve & knee** — where scaling stops, with the 3-run variance; note any regression at 32/64 and its root cause (e.g. server CPU, link saturation, engine cap).
4. **Method & honesty** — exact target, how concurrency was proven, reps/warm-up, and what was `null`+reason.
5. **Verification commands & how to re-run** — exact commands and the result JSON path.
6. **Open items** — anything not proven, and the maintainer decision that remains theirs (e.g. whether to raise the shipped 16-cap if 32+ genuinely wins).

Begin now: load the skills, create the worktree, read STEP ZERO docs, pick/ask for the real-path target, write the plan, then dispatch the swarm.
