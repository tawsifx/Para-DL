# MASTER PROMPT — Rust Computational Core Migration (paraDL)

> Give this to a fresh agent session. It is self-contained: the agent must not ask "what next"; make rulings in a ledger and keep going. Only stop for irreversible/shared-branch git operations, a decision only the maintainer can make, or a plan so broken every path is a guess.
>
> **Execution model:** many parallel subagent swarms across several waves (Wave 0 foundations → Wave 1 Tier A → Wave 2 Tier B → Wave 3 cleanup), orchestrated with `dispatching-parallel-agents` + `subagent-driven-development`, with a two-verdict review after every diff. Do not collapse it into a single agent doing everything sequentially — use the swarms as specified in §3.

You are a **Principal Engineer** migrating paraDL's portable Python logic into the existing Rust workspace, behind stable Python seams, preserving every observable contract byte-for-byte.

## STEP ZERO — read before anything else (authoritative background)

1. `/Users/tawsifx/paradl/docs/rust-replaceability-report.md` — read in full. It is the analysis that produced this prompt and contains the feasibility tiers and exact file inventory.
2. `/Users/tawsifx/paradl/PARALLEL_DL_DEV_CONTEXT.md` — especially §7 (perf audit), the `mdm-engine`→`rapids` migration, and the honesty caveats (warm-cache hits and ~1.2–1.3× drift are NOT wins).
3. `/Users/tawsifx/paradl/README.md`, `/Users/tawsifx/paradl/rust/README.md`, `/Users/tawsifx/paradl/pyproject.toml`, `/Users/tawsifx/paradl/rust/Cargo.toml`, `/Users/tawsifx/paradl/rust/paradl-rs/src/lib.rs`.
4. Contract oracles: `tests/test_rust_adapter_contract.py`, `tests/test_rust_engine_parity.py`, `tests/test_rust_pipeline_e2e.py`, `tests/test_rust_watchdog_recycle.py`, `rust/rapids/src/tests.rs`, `rust/paradl-rs/contract_tests/test_task2_contracts.py`, `tests/unit/test_models.py`, `tests/unit/test_classifier.py`.
5. Git state: worktree `/Users/tawsifx/paradl`, branch `main`, HEAD `b51e6cc`. Record `BASE=$(git rev-parse HEAD)`. Use an isolated worktree (`worktree_create` or `git worktree add`) for all edits; never work on `main` directly.
6. Create ledger `.superpowers/rust-core-migration-ledger.md`. Every assumption and ruling is recorded as `Ruling: X — why — cost if wrong`.

---

## 0. The central engineering problem — scope and the maintainer decision

The analysis is unambiguous: only ~24% of Python LOC is cleanly portable, ~4% should be deleted, and ~72% must stay Python (framework/UI, OS binding, REST-API product policy, external tools, SQLite persistence). The HTTP(S) transport core is **already Rust**.

- **Recommended end state (default):** Rust computational core + thin Python orchestration shell. Port Tier A/B, fix CI and bindings, then delete the dead legacy aria2 code.
- **Decision only the maintainer can make (STOP and ask):** if they demand a full rewrite including the CLI, rich/textual UI, FastAPI server, macOS/Windows platform layers, and yt-dlp media pipeline. That requires reimplementing those frameworks in Rust with no transfer-performance benefit. Present the report's §6 and ask before proceeding on that path.

**Contract surface — must remain byte-identical:**
- `ExitCode` IntEnum: `OK=0, FAILURE=1, USAGE=2, VERIFICATION_FAILED=3, MISSING_DEPENDENCY=4, INTERRUPTED=130`.
- `TransferResult` fields; `TargetInfo`/`DownloadItem` dataclass shapes; `TargetType`/`WatchdogMode` StrEnum values.
- `parse_rate`: binary 1024 multipliers, `b`/`bit(s)` ⇒ `/8`, `off/none/0` ⇒ 0.0; `sanitize_filename`: 200-char cap, unsafe chars → `_`, Windows reserved names → `_`-prefix; `format_size`/`format_speed` strings.
- Watchdog: `WatchdogActionType` values including `tier2_pause_unpause`; `MAX_CONNECTIONS=16` with `min(max(n,1),16)` everywhere.
- PyO3 kwarg names: `checksum` (NOT `expected_digest`), `cookies` (NOT `cookie_file`); native snapshot/result aliases.
- `.pdl.part` + `.pdl.json` sidecars: persist on interrupt, clean on success, byte-exact on resume.
- JSONL `UI.emit_event` schema `{event, timestamp: round(time.time(),3), ...redacted}` — ~48 event names; a success emits `verification` with `verified=true` and **no** `error` event.
- `report.json` schema `1.0` with exact nested `modules.m0..m10` keys; `test_classifier.py` seeded byte-for-byte cases.
- CLI entry points `paradl`/`parallel-dl` → `paradl.cli:main`; `--engine {rust,aria2}`; `PARADL_HOME`, `PARADL_GITHUB_ARCHIVE_MIRROR`.
- Cross-host cookie / `Authorization` / `Proxy-Authorization` non-leak; close-delimited 200 must not produce false 100% or O(N²) re-GETs.

---

## 1. MANDATORY SKILLS (obra/superpowers) — invoke and obey, do not improvise

Load these via the `skill` tool at the start, in this order, and follow each exactly. If a skill has a checklist, create a todo per item. Track everything in the ledger.

**Process skills (load first):**
1. `using-superpowers` — skill invocation before any response/action.
2. `brainstorming` — surface assumptions (scope tiers, dropped flags, clock seams, byte-parity) before designing; present the maintainer decision in §0.
3. `systematic-debugging` — for ANY surprising test/parity failure: root cause, pattern, single hypothesis + minimal test, single fix. No guessing, no sleep hacks.
4. `test-driven-development` — every ported function needs a RED→GREEN test that mirrors the Python oracle, plus a Python shim test proving callers are unchanged. No implementation code without a failing test first.
5. `writing-plans` — write `docs/superpowers/plans/<date>-rust-core-migration.md` (file-by-file ownership, exact commands, exact success criteria) **before** implementation.
6. `karpathy-guidelines` — surface assumptions, surgical changes, minimum code that solves the problem.

**Isolation & orchestration skills:**
7. `using-git-worktrees` — isolated workspace; record `BASE`; never work on `main`.
8. `dispatching-parallel-agents` — one agent per independent domain; focused self-contained prompts; strict scope; disjoint file ownership; no two agents edit the same file.
9. `subagent-driven-development` — fresh implementer per task; spec + quality review after each diff; fix loops stay with the owning agent (cap 5 rounds); broad whole-branch review on the most capable model at the end.
10. `requesting-code-review` / `receiving-code-review` — two explicit verdicts with severity-tagged findings and `file:line`; technical rigor, not performative agreement.

**Completion skill:**
11. `verification-before-completion` — never claim green, ported, or done without fresh complete command output in the current message. Evidence before assertions.
12. `finishing-a-development-branch` — only when the maintainer wants integration.

Load any other skill with even a 1% chance of applying (`find-skills` if a capability gap appears).

---

## 2. PHASE 1 — baseline & plan (lead + parallel read-only agents)

- Snapshot baseline, saving raw output under `.superpowers/rust-core-migration/baseline.txt`: `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, `cargo clippy --all -- -D warnings`, current benchmark exit status.
- Confirm and ledger: (a) CI has no Rust/maturin step and native tests skip; (b) `http3`/`doh_endpoint`/`accept_ranges`/`total` are accepted then discarded at `rust/paradl-rs/src/lib.rs:139/186/420/452`; (c) `etag_sha256` never reaches Rust; (d) `get_clock()` injection seams; (e) the two committed arm64/CPython-3.12 `.so` files; (f) missing PyO3 bindings (`set_limit`), `PROBING`, `.pyi` drift, shim candidate-name mismatch.
- Produce the plan with the exact wave/task map and success criteria before Phase 2.

---

## 3. PHASE 2 — multi-wave subagent swarms (disjoint file ownership)

Each agent writes a report to `.superpowers/rust-core-migration/taskN-report.md` and returns only a short summary (status, files changed, one-line test result, concerns). Self-contained prompts: scope, relevant STEP ZERO facts, global constraints. Do NOT paste session history. Wave N+1 starts only after Wave N diffs pass their spec + quality reviews.

**Global constraints for every agent:** preserve the contract surface in §0 exactly; keep the Python API surface as the stable seam (never make callers import Rust types directly); TDD RED→GREEN; mirror Python oracles one-for-one with exact literals; disjoint file ownership; no commits on `main`; no push unless asked.

### Wave 0 — foundations (dispatch in parallel)
- **W0-1 CI/build owner** (`.github/workflows/ci.yml`, `rust/paradl-rs/pyproject.toml`, `Formula/parallel-dl.rb`, `scripts/install-local.*`, `.gitignore`): add maturin build + `cargo test --all` + native-gated pytest to a macOS/Windows × Py3.11/3.12 matrix; stop committing the `.so` (remove from git, ignore it); keep the shim graceful-degradation path.
- **W0-2 Contract-oracle owner** (`tests/**`, new golden fixtures only): freeze byte-for-byte oracles for ExitCode, rate strings, filename sanitize, ~48 JSONL event shapes, `.pdl` sidecars, `report.json` v1.0. Write RED tests first. Do not change production code.
- **W0-3 PyO3 helper-module owner** (NEW `rust/paradl-core/**` crate + `rust/paradl-rs/src/lib.rs`): establish the helper binding module `paradl._rust_core` with its own Python shim `src/paradl/_rust_core.py`, availability/version gate, and a clock-callback protocol for injected `get_clock()`.
- **W0-4 Binding-hygiene owner** (`rust/paradl-rs/src/lib.rs`, `_rust_engine.pyi`, `rust/README.md`, `src/paradl/_rust_engine.py`): fix `.pyi`/README drift, align shim candidate names, add `set_limit` binding and `PROBING` constant; decide each dropped flag (implement or document as intentionally ignored). No engine behavior change without a test.

### Wave 1 — Tier A ports (dispatch in parallel)
- **W1-1 Throttle pure core** (`rust/paradl-core/src/throttle/**`; shims `src/paradl/throttle/{models,classifier,parsers,budget}.py` + non-rich parts of `report.py`/`probes.py`): port models, classifier (Mann-Whitney, next-best-hours), regex parsers, budget accounting + `parse_byte_budget`, pure statistics, Markdown/HTML/SVG generators and `report_from_dict`. Add crates `regex`, `toml`, MT19937-compatible RNG. Mirror `test_classifier.py` 12 seeded cases exactly (Python `round`/`statistics`/format parity). Keep jitter in a Python wrapper if exact MT parity is not achievable; keep the rich terminal renderer in Python.
- **W1-2 curl parser** (`rust/paradl-core/src/curl.rs`; shim `src/paradl/util/curl_parser.py`): tokenizer + existing `url` crate + 0600 cookie jar; preserve POST/data rejection; golden tests on real `Copy as cURL` samples.
- **W1-3 core model helpers** (`rust/paradl-core/src/model_helpers.rs`; shim `src/paradl/core/models.py`): `parse_rate`, `format_rate_for_aria2`, `format_size`, `format_speed`, `sanitize_filename`, domain lists, enum constants. Re-export from `src/paradl/models.py`.
- **W1-4 display/JSONL helpers** (`rust/paradl-core/src/events.rs`; shim `src/paradl/ui/display.py` helpers): redaction + JSONL serialization; golden output over all ~48 event shapes. Keep rich rendering Python.
- **W1-5 checksum dedup** (`rust/rapids/src/verify.rs`, `rust/paradl-rs/src/lib.rs`, `src/paradl/postproc/checksum.py`): add blake2b, GNU/BSD checksum-text parsing, companion discovery, bind `digest`/`matches`; remove the redundant Python re-verification at `engines/rust_engine.py:231` only after parity is proven.

### Wave 2 — Tier B gated ports (start only after Wave 1 diffs accepted)
- **W2-1 watchdog + supervision** (`rust/paradl-core/src/watchdog.rs`, `src/paradl/engines/watchdog.py`): port pure decision logic with Rust clock callback + JSONL emitter + injected learned profiles (or rusqlite). Preserve `tier2_pause_unpause` and parity with both the Rust loop and legacy aria2 loop.
- **W2-2 archive ZIP/tar** (`rust/paradl-core/src/archive.rs`, shim `src/paradl/postproc/archive.py`): add `zip`/`tar`/compression crates; replicate canonicalize + `starts_with` + symlink/device rejection; security tests first; keep 7z/DMG subprocess wrappers.
- **W2-3 mirrors/Metalink pure subset** (`rust/paradl-core/src/mirrors.rs`, shim `src/paradl/storage/mirrors.py`): XML crate with XXE guard + regex/glob/prefix matching; keep HTTP probing at the Python boundary or via `rapids::probe`.
- **W2-4 throttle store + toolchain** (`rust/paradl-core/src/{store,toolchain}.rs`, shims): `rusqlite` with shared `history.db` schema guard; keep `importlib.metadata` yt-dlp-ejs probe in Python; keep the coverage-matrix function reachable from Python (`test_classifier.py` requires it).

### Wave 3 — cleanup (after Wave 2 accepted)
- **W3-1** Delete dead legacy aria2 daemon/RPC/loop code (`engines/aria2.py`, ~1,200 LOC) **only after** the torrent/metalink/magnet decision and the no-native fallback policy are retired in the ledger; move the selector to the app/config layer.
- **W3-2** Consolidate selector, update `README.md`/`rust/README.md`/`pyproject.toml` metadata, record which features remain intentionally Python.

Lead owns integration, per-diff spec + quality review, arbitration at the 5-round cap, the maintainer decision, and the final gate. Do not edit subagent-owned files while they run.

---

## 4. PHASE 3 — review loop (per task, before acceptance)

For each returned diff: generate a review package (`git diff -U10` + new-file contents) under `.superpowers/rust-core-migration/reviews/taskN.txt`; dispatch a reviewer that must output `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes` with severity-tagged findings (`Critical/Important/Minor`) and `file:line`. Failed → fix loop with the SAME owning agent (resume its session); re-review only the fix diff; cap 5 rounds, then adjudicate with a ledger ruling. Do not start the next wave until the current wave's diffs are accepted. After all waves: one broad whole-branch review on the most capable available model; triage every deferred finding into the ledger.

---

## 5. SUCCESS CRITERIA (acceptable evidence = raw command output)

- `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, `cargo clippy --all -- -D warnings` all green — **with the `.so` built and native-gated tests actually running**. Report the count of executed native tests; zero unexplained skips (any skip needs `null`+reason in the ledger).
- Every literal in §0 verified unchanged, with the command output that proves it.
- Each ported function has a Rust `#[test]` mirroring the Python oracle plus a Python shim test proving callers are unchanged.
- CI builds the extension on macOS + Windows (Py3.11/3.12) and runs `cargo test --all` + native-gated pytest; no committed `.so`.
- Benchmarks: cold medians with identical URLs and pinning/sentinel harness; 1.2–1.3× drift is NOT a win; warm-cache numbers never presented as wins; `null`+reason for unmeasurable metrics.
- No product behavior change except explicitly ruled items in the ledger. `git status` reviewed; nothing committed on `main`; no push unless asked.

---

## 6. FINAL OUTPUT (plain English)

1. **TL;DR** — LOC moved to Rust, LOC deleted, LOC intentionally kept Python, with the ratio.
2. **Migration table** — each component: before/after owner, tests now running in Rust, tests that stay Python.
3. **Contract audit** — every §0 literal verified unchanged, with the proving command output.
4. **CI/build reality** — matrix, no committed `.so`, native tests executing (not skipped).
5. **Honesty notes** — unported surfaces and why; performance claims with the drift caveat.
6. **Open items** — maintainer decisions remaining.

Begin now: load the skills, create the worktree, read STEP ZERO, write the plan, then dispatch Wave 0.
