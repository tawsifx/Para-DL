# MASTER PROMPT — Full Native Rust Rewrite (paraDL)

> Give this to a fresh agent session. It is self-contained: the agent must not ask "what next"; make rulings in a ledger and keep going. Only stop for irreversible/shared-branch git operations or a plan so broken every path is a guess.
>
> **Execution model:** many parallel subagent swarms across several waves (Wave 0 foundations → Wave 1 data/transport → Wave 2 subsystems → Wave 3 framework → Wave 4 orchestration/parity → Wave 5 cutover), orchestrated with `dispatching-parallel-agents` + `subagent-driven-development`, with a two-verdict review after every diff. Do not collapse it into a single agent doing everything sequentially — use the swarms as specified in §3.

You are the **lead engineer** executing a full framework rewrite: replace all ~28,300 lines of Python in `src/paradl` with a single native Rust binary. The end state has **no Python runtime, no `.so`, no PyO3 shim, and no Python packaging** — one Cargo workspace, one binary, installed via Homebrew and the install scripts.

## STEP ZERO — read before anything else (authoritative background)

1. `/Users/tawsifx/paradl/docs/rust-replaceability-report.md` — read in full. It is the analysis that produced this prompt: feasibility tiers, domain inventory, contract surface, and the blockers a full rewrite must resolve.
2. `/Users/tawsifx/paradl/PARALLEL_DL_DEV_CONTEXT.md` — §7 perf audit, `mdm-engine`→`rapids` migration, honesty caveats (warm-cache and ~1.2–1.3× drift are NOT wins).
3. `README.md`, `rust/README.md`, `pyproject.toml`, `rust/Cargo.toml`, `rust/paradl-rs/src/lib.rs`.
4. Executable oracle (the Python implementation and its tests — DO NOT EDIT until Wave 5): all of `src/paradl/**`, `tests/**` (6,430 LOC), `rust/rapids/src/tests.rs`, `rust/paradl-rs/contract_tests/**`, `benchmarks/**`.
5. Git state: worktree `/Users/tawsifx/paradl`, branch `main`, HEAD `b51e6cc`. Record `BASE=$(git rev-parse HEAD)`. Use an isolated worktree; never work on `main` directly.
6. Create ledger `.superpowers/full-rust-rewrite-ledger.md`. Every assumption and ruling is `Ruling: X — why — cost if wrong`. Record the maintainer's standing decision: **full rewrite, Python runtime removed** (this supersedes the scoped option in the report).

---

## 0. The central engineering problem — architecture and standing rulings

**This is a rewrite of the whole application, not a port of the download engine.** The transport core is already Rust; the work is the surrounding framework: CLI (~60 flags, 20 subcommands), live display + TUI, FastAPI server, seven site/API engines, SQLite persistence, macOS/Windows platform integration, throttle diagnostics, and media. Expect 25–35k Rust LOC and many swarms. There is no shortcut that avoids reimplementing these layers.

**Architecture (your call, enforced): single Cargo workspace, strangler-fig delivery.**

- Crates: `paradl-domain` (models, helpers, config, paths, clock, errors), `paradl-store` (rusqlite), `paradl-net` (reqwest, probe, DNS/DoH, site APIs), `paradl-engine` (promote `rapids`), `paradl-watchdog`, `paradl-postproc`, `paradl-platform`, `paradl-media`, `paradl-throttle`, `paradl-ui` (JSONL + display + ratatui TUI), `paradl-api` (axum), `paradl-cli` (clap), `paradl-app` (orchestration + diagnostics), `paradl` (binary entry).
- `src/paradl` and the Python tests stay **untouched and runnable** through Wave 4. They are the executable oracle: every Rust layer is verified by diffing observable behavior against the Python CLI. Delete Python only in Wave 5.
- No PyO3, no maturin extension, no Python shims in the end state. Build with `cargo build --release`.

**Standing ledger rulings — resolve or confirm each before its owning wave, never silently:**

1. **Media / yt-dlp:** no Rust extractor ecosystem exists. Drive the `yt-dlp` **external binary** via `-J`/`--dump-json` + `--print`, parse with serde_json; keep ffmpeg/afconvert/7z/hdiutil/adb as external processes. Replace `mutagen` with mature Rust crates (`id3`, `mp4ameta`) and ffmpeg fallback. **No Python sidecar under any circumstance.**
2. **Dropped flags:** `http3`, `doh_endpoint`, `accept_ranges`, `total` are accepted-then-discarded today (`lib.rs:139/186/420/452`). Recommended: keep flags CLI-compatible; implement DoH via `hickory-dns`; drop HTTP/3 with an explicit `null`+reason in the ledger (no stable Rust stack) unless overridden.
3. **Byte-exact OS contracts:** macOS binary-plist xattr hex (`com.apple.metadata:kMDItemWhereFroms`), `com.apple.quarantine`, Windows `Zone.Identifier` CRLF INI, and keychain secrets-over-stdin must be reproduced exactly. Golden fixtures come from `tests/support/macos_fake.py` and `tests/unit/test_platform_windows.py`.
4. **JSONL contract:** `UI.emit_event` schema `{event, timestamp: round(time.time(),3), ...redacted}` — ~48 event names; success emits `verification` with `verified=true` and no `error` event. Script and LLM callers consume this.
5. **Data compatibility:** shipped `history.db` is SQLite schema v4 (WAL, `ALTER TABLE` migrations, shared `queue_jobs`); `throttle_samples`/`throttle_runs`/`asn_cache` share the same file. Byte-compatible migration is mandatory.
6. **CLI compatibility:** all ~60 flags, 20 subcommands, `--engine {rust,aria2}`, env vars `PARADL_HOME`/`PARADL_GITHUB_ARCHIVE_MIRROR`, entry-point names `paradl`/`parallel-dl`, and exit codes preserved.
7. **Distribution:** rewrite `Formula/parallel-dl.rb` (Rust build instead of `virtualenv_install_with_resources`) and `scripts/install-local.{sh,ps1}` (native binary install instead of `uv tool install`); remove the two committed arm64 `.so` files. Users must not need Python installed.

**Contract surface — must remain byte-identical across the cutover:**
- `ExitCode`: `OK=0, FAILURE=1, USAGE=2, VERIFICATION_FAILED=3, MISSING_DEPENDENCY=4, INTERRUPTED=130`.
- `TransferResult` / `TargetInfo` / `DownloadItem` shapes; `TargetType`/`WatchdogMode` StrEnum values; `WatchdogActionType` values incl. `tier2_pause_unpause`; `MAX_CONNECTIONS=16` with `min(max(n,1),16)`.
- `parse_rate` (1024 multipliers, `b`/`bit(s)` ⇒ `/8`), `sanitize_filename` (200-char cap, Windows reserved names), `format_size`/`format_speed` strings.
- `.pdl.part` + `.pdl.json` sidecars: persist on interrupt, clean on success, byte-exact on resume.
- Native result aliases, checksum mismatch ⇒ exit 3, generic failure ⇒ exit 1, missing engine ⇒ exit 4.
- `report.json` schema `1.0` with exact `modules.m0..m10` keys; `test_classifier.py` 12 seeded byte-for-byte cases.
- Cross-host cookie / `Authorization` / `Proxy-Authorization` non-leak; close-delimited 200 must not false-100% or O(N²) re-GET.

---

## 1. MANDATORY SKILLS (obra/superpowers) — invoke and obey, do not improvise

Load via the `skill` tool at the start, in this order; follow each exactly; track everything in the ledger.

**Process skills (load first):**
1. `using-superpowers` — skill invocation before any response/action.
2. `brainstorming` — already completed at the orchestration level (maintainer chose full rewrite, no Python sidecar, strangler-fig); re-read the standing rulings above, do not reopen them unless a wave reveals a genuine conflict.
3. `systematic-debugging` — for ANY parity failure: root cause, pattern, single hypothesis + minimal test, single fix. No guessing, no sleep hacks.
4. `test-driven-development` — every Rust component needs a RED→GREEN test mirroring the Python oracle; no implementation code without a failing test first.
5. `writing-plans` — write `docs/superpowers/plans/<date>-full-rust-rewrite.md` (file-by-file ownership, exact commands, exact success criteria per wave) **before** implementation.
6. `karpathy-guidelines` — surface assumptions, surgical scope, minimum code that solves the problem.

**Isolation & orchestration skills:**
7. `using-git-worktrees` — isolated workspace; record `BASE`; never work on `main`.
8. `dispatching-parallel-agents` — one agent per independent domain; focused self-contained prompts; strict scope; disjoint file ownership; no two agents edit the same file.
9. `subagent-driven-development` — fresh implementer per task; spec + quality review after each diff; fix loops stay with the owning agent (cap 5 rounds); broad whole-branch review on the most capable model at the end.
10. `requesting-code-review` / `receiving-code-review` — verdicts `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes`, severity-tagged findings with `file:line`; technical rigor, not performative agreement.

**Completion skill:**
11. `verification-before-completion` — never claim green, ported, or done without fresh complete command output in the current message. Evidence before assertions.
12. `finishing-a-development-branch` — only when the maintainer wants integration.

Load any other skill with even a 1% chance of applying (`find-skills` if a capability gap appears).

---

## 2. PHASE 1 — baseline & plan (lead + parallel read-only agents)

- Snapshot baseline, save raw output under `.superpowers/full-rust-rewrite/baseline.txt`: `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, `cargo clippy --all -- -D warnings`, `benchmarks/bench_baseline.py`, `benchmarks/bench_rust_engine.py`.
- Confirm and ledger: the CI gap (no Rust build, native tests skipped), the two committed `.so` files, the dropped flags, the OS metadata contracts, the shared SQLite schema, the CLI flag/subcommand count, and the ~48 JSONL event names.
- Produce the plan with the exact wave/task map, crate ownership, and success criteria before Phase 2.

---

## 3. PHASE 2 — multi-wave subagent swarms (disjoint file ownership)

Each agent writes a report to `.superpowers/full-rust-rewrite/taskN-report.md` and returns only a short summary (status, files changed, one-line test result, concerns). Self-contained prompts: scope, relevant STEP ZERO facts, global constraints. Do NOT paste session history. Wave N+1 starts only after Wave N diffs pass their spec + quality reviews.

**Global constraints for every agent:** `src/paradl` and Python tests are READ-ONLY until Wave 5; preserve the contract surface in §0 exactly; verify behavior by diffing against the Python oracle; TDD RED→GREEN; disjoint file ownership; no commits on `main`; no push unless asked.

### Wave 0 — foundations & oracle (dispatch in parallel)
- **W0-1 Build/CI/distribution** (workspace root `Cargo.toml`, `.github/workflows/ci.yml`, `Formula/parallel-dl.rb`, `scripts/install-local.*`, `.gitignore`): native binary that prints `paradl --version`; macOS + Windows × stable Rust CI running `cargo test --all` and `cargo clippy --all -- -D warnings`; remove committed `.so`; rewrite Homebrew formula and install scripts for a Rust build.
- **W0-2 Contract oracle** (`crates/paradl-oracle/**`): Rust golden fixtures + tests mirroring `test_models.py`, `test_classifier.py`, `test_watchdog.py`, the Rust parity/e2e tests, JSONL event shapes, sidecars, and `report.json` v1.0. RED tests first; production code untouched.
- **W0-3 Domain crate** (`crates/paradl-domain/**`): models, helpers (`parse_rate`, `format_rate_for_aria2`, `format_size`, `format_speed`, `sanitize_filename`), config, paths, clock abstraction, errors. Verified against `test_models.py`/`test_config.py` oracles.
- **W0-4 Parity inventory** (read-only): enumerate every CLI flag/subcommand, JSONL event, config field, DB table, PyO3 surface item, and env var into `docs/superpowers/parity-matrix.md`. This is the checklist no wave may finish without clearing.

### Wave 1 — data & transport (dispatch in parallel)
- **W1-1 `paradl-store`**: rusqlite, 7 history tables + 3 throttle tables, schema v4 migrations, WAL, device+inode guard, byte-compatible with shipped `history.db`. Port `test_sqlite_guard.py` invariants.
- **W1-2 `paradl-engine`**: promote `rust/rapids` to a first-class crate; native API (no PyO3), planner/state/limiter/verify/probe/download, DoH via hickory (per ruling 2), IPv6, recycle, verify-then-rename. Port `rust/rapids/src/tests.rs` and the Python parity/e2e/watchdog-recycle tests into Rust.
- **W1-3 `paradl-watchdog`**: state machine (v1/v2), percentile, cooldown, relative straggler, learned profiles, supervision loop with recycle. Byte-exact `watchdog_action` events; port `test_watchdog.py`.
- **W1-4 `paradl-net`**: reqwest client, Range/GET probe, RFC 6266 filename + ETag digest parsing, DNS/DoH, proxy/IP-family, retry; foundations for site APIs. Port `test_net.py` parser cases.

### Wave 2 — subsystems (dispatch in parallel)
- **W2-1 `paradl-throttle`**: full port — models, classifier (Mann-Whitney, next-best-hours), regex parsers, budget, probes, targets, tokio VPS server, report generators, CLI logic. Mirror `test_classifier.py` seeded cases byte-for-byte.
- **W2-2 `paradl-postproc` + `paradl-platform`**: checksum (blake2b, GNU/BSD text, companion discovery), security-validated ZIP/tar extraction, ADB, and macOS/Windows power/network/sleep/keychain/WhereFroms/Zone.Identifier with golden-file parity (ruling 3). 7z/DMG/hdiutil stay external processes.
- **W2-3 site engines** (`paradl-net`): github, hf, brew, pypi, ipsw, cloud — REST resolution, asset/wheel scoring, token flows, digest verification, direct-URL rewrite; delegate transfers to `paradl-engine`. Port `test_bug_fixes.py`, `test_github_acceleration.py`, `test_pypi_transitive.py` cases.
- **W2-4 resilience/auth**: queue scheduling (`--at`, `--when-fast`, crash recovery), launchd plist generation, hooks with 30s timeout + JSON-stdin, resilience event source (WAKE/NET_CHANGE/STALL), auth manager (env→keychain→cache, masking).

### Wave 3 — user-facing framework (dispatch in parallel)
- **W3-1 `paradl-cli`**: clap CLI with all ~60 flags and 20 subcommands; exact help/usage/error text, exit codes, env vars, `--engine`, entry-point names; `doctor`/`netprobe`/`bench` subcommands wired to `paradl-app`. Port `test_bug_fixes.py` CLI cases and `test_perf_parity.py`.
- **W3-2 `paradl-ui`**: byte-exact JSONL emitter and redaction (ruling 4), progress/live display replacing rich, ratatui TUI replacing textual (filters, task list, add-job screen, actions). Golden-output tests over all ~48 event shapes.
- **W3-3 `paradl-api`**: axum server with the 7 REST endpoints, serde validation equivalent to pydantic `Field(ge/le)`, task store/runner, uvicorn-equivalent lifecycle; always present in the binary.
- **W3-4 `paradl-media`**: yt-dlp external driver (ruling 1), metadata via `id3`/`mp4ameta` + ffmpeg fallback, ffmpeg/afconvert conversion, cover embedding, direct-URL handoff to `paradl-engine`.

### Wave 4 — orchestration, parity, performance (dispatch in parallel)
- **W4-1 `paradl-app`**: orchestrator wiring all crates; diagnostics (doctor, netprobe, bench); full subcommand coverage; verify every row of `parity-matrix.md`.
- **W4-2 Differential parity harness** (owns `crates/paradl-parity/**`): run Python CLI and Rust binary against the same loopback servers; diff stdout, stderr, JSONL events, produced files/bytes, sidecars, and exit codes; fail on any difference. This is the cutover gate.
- **W4-3 Benchmarks**: port both harnesses with pinning/sentinels, cold/warm separation, ≥3 reps, CPU/RSS, `null`+reason; cold medians only, identical URLs, drift caveat enforced.

### Wave 5 — cutover (only after all prior waves accepted and W4-2 is green)
- **W5-1** Delete `src/paradl/**`, Python tests, `pyproject.toml` Python packaging, `parallel-dl.py`, both committed `.so` files; point entry points and docs at the native binary; finalize Homebrew formula and install scripts.
- **W5-2** Final full verification, release artifact, docs update, ledger close-out with every ruling and its outcome.

Lead owns integration, per-diff spec + quality review, arbitration at the 5-round cap, and the final gate. Do not edit subagent-owned files while they run.

---

## 4. PHASE 3 — review loop (per task, before acceptance)

For each returned diff: generate a review package (`git diff -U10` + new-file contents) under `.superpowers/full-rust-rewrite/reviews/taskN.txt`; dispatch a reviewer that must output `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes` with severity-tagged findings (`Critical/Important/Minor`) and `file:line`. Failed → fix loop with the SAME owning agent (resume its session); re-review only the fix diff; cap 5 rounds, then adjudicate with a ledger ruling. Do not start the next wave until the current wave's diffs are accepted. After Wave 5: one broad whole-branch review on the most capable available model; triage every deferred finding into the ledger.

---

## 5. SUCCESS CRITERIA (acceptable evidence = raw command output)

- `cargo test --all`, `cargo clippy --all -- -D warnings`, `cargo build --release` green on macOS and Windows; binary `paradl --version` and `doctor` work with no Python installed.
- Differential harness W4-2 reports **zero differences** across stdout, stderr, JSONL, files, sidecars, and exit codes for the full oracle corpus.
- Every row of `docs/superpowers/parity-matrix.md` marked pass with proving output; every §0 contract literal verified unchanged.
- `history.db` from a shipped Python install opens, migrates, and round-trips correctly in the Rust binary.
- External tools invoked per rulings: `yt-dlp`, `ffmpeg`, `afconvert`, `7z`, `hdiutil`, `adb` — no Python process anywhere.
- Homebrew formula and both install scripts install a working native binary from a clean machine; no Python runtime dependency.
- Benchmarks: cold medians, identical URLs, pinning/sentinels, ≥3 reps; 1.2–1.3× drift NOT a win; `null`+reason for unmeasurable metrics.
- `git status` reviewed; nothing committed on `main`; no push unless asked.

---

## 6. FINAL OUTPUT (plain English)

1. **TL;DR** — what shipped, LOC replaced, what remains external, and the one-line install command.
2. **Parity table** — every CLI flag/subcommand, JSONL event, DB table, and contract literal with its proving test.
3. **Cutover audit** — Python deleted, no `.so`, no Python runtime, Homebrew/install scripts verified on a clean machine.
4. **External-tool inventory** — which binaries are invoked and for what; confirmation no Python process is spawned.
5. **Honesty notes** — HTTP/3/DoH ruling outcome, media limitations, benchmark drift caveat.
6. **Open items** — anything deferred into the ledger.

Begin now: load the skills, create the worktree, read STEP ZERO, write the plan, then dispatch Wave 0.
