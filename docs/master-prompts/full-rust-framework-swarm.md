# MASTER PROMPT — Build `paradl` as a Full Rust Framework

> Give this to a fresh agent session. It is self-contained: the agent must not ask "what next"; make rulings in a ledger and keep going. Only stop for irreversible/shared-branch git operations or a plan so broken every path is a guess.
>
> **Execution model:** many parallel subagent swarms across several waves (Wave 0 charter → Wave 1 core → Wave 2 services → Wave 3 surfaces → Wave 4 reference binary → Wave 5 cutover/release), orchestrated with `dispatching-parallel-agents` + `subagent-driven-development`, with a two-verdict review after every diff. Do not collapse it into a single agent doing everything sequentially — use the swarms as specified in §3.

You are the **lead architect** building a **full Rust framework** that replaces the entire Python application. The deliverable is two things:

1. A **reusable framework** — a Cargo workspace of public, semver-versioned, documented crates with trait-based extension points, feature flags, examples, and benches.
2. A **thin `paradl` binary** that is the reference consumer of that framework and preserves the existing CLI behavior byte-for-byte.

End state: **no Python runtime, no `.so`, no PyO3, no Python packaging.**

## STEP ZERO — read before anything else (authoritative background)

1. `/Users/tawsifx/paradl/docs/rust-replaceability-report.md` — read in full: feasibility tiers, domain inventory, contract surface, blockers.
2. `/Users/tawsifx/paradl/docs/master-prompts/full-rust-rewrite-swarm.md` — read in full. This prompt supersedes its architecture with the framework goal; its rulings and waves carry forward unless changed here.
3. `/Users/tawsifx/paradl/PARALLEL_DL_DEV_CONTEXT.md` — §7 perf audit, `mdm-engine`→`rapids` migration, honesty caveats (warm-cache and ~1.2–1.3× drift are NOT wins).
4. `README.md`, `rust/README.md`, `pyproject.toml`, `rust/Cargo.toml`, `rust/paradl-rs/src/lib.rs`.
5. Executable oracle (DO NOT EDIT until Wave 5): `src/paradl/**`, `tests/**` (6,430 LOC), `rust/rapids/src/tests.rs`, `rust/paradl-rs/contract_tests/**`, `benchmarks/**`.
6. Git state: worktree `/Users/tawsifx/paradl`, branch `main`, HEAD `b51e6cc`. Record `BASE=$(git rev-parse HEAD)`. Use an isolated worktree; never work on `main` directly.
7. Create ledger `.superpowers/full-rust-framework-ledger.md`. Every assumption and ruling is `Ruling: X — why — cost if wrong`. Record the maintainer's standing decision: **full Rust framework, Python runtime removed, binary as reference consumer**.

---

## 0. The central engineering problem — framework shape and standing rulings

**This is not a line-by-line port. It is a framework design.** The existing Python application is the behavioral specification; the Rust output must be a set of composable crates that a third party could use to build their own downloader, with the shipped `paradl` binary as one such consumer. Internal crates are not enough: public API surface, docs, feature flags, and extension traits are deliverables in their own right.

**Framework architecture (enforced):**

- Workspace crates and their **public roles** — every crate gets a crate-level doc comment, semver `0.1.0` during development, `README`, and at least one runnable example:
  - `paradl-core` — domain models, rate/filename helpers, config, paths, clock abstraction, error hierarchy. Pure, `no_std`-friendly where practical.
  - `paradl-engine` — promoted `rapids`; segmented HTTP(S) download engine behind an async `DownloadEngine` trait.
  - `paradl-net` — reqwest client, probe, DNS/DoH, and the `TargetResolver` trait (GitHub/HF/brew/PyPI/ipsw/cloud implementations).
  - `paradl-store` — rusqlite persistence behind a `Store` trait (history, queue, mirrors, throttle tables).
  - `paradl-watchdog` — anti-throttle decision state machine behind a `Watchdog` trait.
  - `paradl-postproc` — checksum, archive, ADB behind a `PostProcessor` trait.
  - `paradl-platform` — macOS/Windows integration behind a `Platform` trait; feature-gated per OS.
  - `paradl-media` — yt-dlp external driver behind a `MediaResolver` trait.
  - `paradl-throttle` — classifier/parsers/budget/probes/report as a reusable diagnostics crate.
  - `paradl-events` — JSONL event schema and emitter with redaction; a stable, documented contract.
  - `paradl-ui` — progress display and ratatui TUI, feature-gated.
  - `paradl-api` — axum server with the 7 REST endpoints, feature-gated.
  - `paradl-cli` — clap parser and dispatch, reusable by other binaries.
  - `paradl` — the thin reference binary wiring the crates together.
- **Feature flags** replace Python's optional extras: `api`, `tui`, `media`, `platform-macos`, `platform-windows`, `http3` (if ever implemented). Default build must compile without optional system tools.
- **Extension points are public traits**, documented with a "implementing your own X" example: `DownloadEngine`, `TargetResolver`, `Store`, `Watchdog`, `Notifier`, `Platform`, `PostProcessor`, `MediaResolver`.
- **Error model:** one public `paradl_core::Error` with thiserror variants that map to the documented `ExitCode` contract; no stringly-typed errors crossing crate boundaries.
- **Strangler-fig delivery:** `src/paradl` and Python tests stay untouched and runnable through Wave 4 as the executable oracle. Every framework layer is verified by diffing observable behavior against the Python CLI. Delete Python only in Wave 5.

**Standing ledger rulings — resolve or confirm before the owning wave, never silently:**

1. **Media / yt-dlp:** no Rust extractor ecosystem exists. Drive the `yt-dlp` **external binary** via `-J`/`--dump-json` + `--print`, parse with serde_json; keep ffmpeg/afconvert/7z/hdiutil/adb as external processes. Replace `mutagen` with mature Rust crates (`id3`, `mp4ameta`) and ffmpeg fallback. **No Python sidecar under any circumstance.**
2. **Dropped flags:** `http3`, `doh_endpoint`, `accept_ranges`, `total` are accepted-then-discarded today (`lib.rs:139/186/420/452`). Keep CLI flags compatible; implement DoH via `hickory-dns`; drop HTTP/3 behind a `null`+reason ledger ruling unless overridden. Do not expose a framework feature flag for an unimplemented capability.
3. **Byte-exact OS contracts:** macOS binary-plist xattr hex (`com.apple.metadata:kMDItemWhereFroms`), `com.apple.quarantine`, Windows `Zone.Identifier` CRLF INI, keychain secrets-over-stdin. Golden fixtures from `tests/support/macos_fake.py` and `tests/unit/test_platform_windows.py`.
4. **JSONL contract:** `paradl-events` emits `{event, timestamp: round(time.time(),3), ...redacted}` — ~48 event names; success emits `verification` with `verified=true` and no `error` event. Script and LLM callers consume this; treat it as a public wire format with a version constant.
5. **Data compatibility:** shipped `history.db` is SQLite schema v4 (WAL, `ALTER TABLE` migrations, shared `queue_jobs`); throttle tables share the file. Byte-compatible migration mandatory.
6. **CLI compatibility:** all ~60 flags, 20 subcommands, `--engine {rust,aria2}`, env vars `PARADL_HOME`/`PARADL_GITHUB_ARCHIVE_MIRROR`, entry-point names `paradl`/`parallel-dl`, exit codes preserved by the reference binary.
7. **Distribution:** rewrite `Formula/parallel-dl.rb` (Rust build, not `virtualenv_install_with_resources`) and `scripts/install-local.{sh,ps1}` (native binary install, not `uv tool install`); remove both committed arm64 `.so` files. Users must not need Python.

**Contract surface — byte-identical across the cutover:**
- `ExitCode`: `OK=0, FAILURE=1, USAGE=2, VERIFICATION_FAILED=3, MISSING_DEPENDENCY=4, INTERRUPTED=130`.
- Model shapes (`TransferResult`, `TargetInfo`, `DownloadItem`), `TargetType`/`WatchdogMode`/`WatchdogActionType` values incl. `tier2_pause_unpause`; `MAX_CONNECTIONS=16` with `min(max(n,1),16)`.
- `parse_rate` (1024 multipliers, `b`/`bit(s)` ⇒ `/8`), `sanitize_filename` (200-char cap, Windows reserved names), `format_size`/`format_speed` strings.
- `.pdl.part` + `.pdl.json` sidecars: persist on interrupt, clean on success, byte-exact on resume.
- native result aliases; checksum mismatch ⇒ exit 3; generic failure ⇒ exit 1; missing engine ⇒ exit 4.
- `report.json` schema `1.0` with exact `modules.m0..m10` keys; `test_classifier.py` 12 seeded byte-for-byte cases.
- cross-host cookie / `Authorization` / `Proxy-Authorization` non-leak; close-delimited 200 must not false-100% or O(N²) re-GET.

---

## 1. MANDATORY SKILLS (obra/superpowers) — invoke and obey, do not improvise

Load via the `skill` tool at the start, in this order; follow each exactly; track everything in the ledger.

1. `using-superpowers` — skill invocation before any response/action.
2. `brainstorming` — already completed at the orchestration level (maintainer chose a full Rust framework with a thin binary, no Python sidecar, strangler-fig); do not reopen the standing rulings unless a wave reveals a genuine conflict.
3. `systematic-debugging` — for ANY parity failure: root cause, pattern, single hypothesis + minimal test, single fix. No guessing, no sleep hacks.
4. `test-driven-development` — every crate needs RED→GREEN tests mirroring the Python oracle; no implementation code without a failing test first.
5. `writing-plans` — write `docs/superpowers/plans/<date>-full-rust-framework.md` (file-by-file ownership, exact commands, exact success criteria per wave) **before** implementation.
6. `karpathy-guidelines` — surface assumptions, surgical scope, minimum code that solves the problem.
7. `using-git-worktrees` — isolated workspace; record `BASE`; never work on `main`.
8. `dispatching-parallel-agents` — one agent per independent crate/domain; focused self-contained prompts; strict scope; disjoint file ownership; no two agents edit the same file.
9. `subagent-driven-development` — fresh implementer per task; spec + quality review after each diff; fix loops stay with the owning agent (cap 5 rounds); broad whole-branch review on the most capable model at the end.
10. `requesting-code-review` / `receiving-code-review` — verdicts `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes`, severity-tagged findings with `file:line`; technical rigor, not performative agreement.
11. `verification-before-completion` — never claim green, ported, or done without fresh complete command output in the current message. Evidence before assertions.
12. `finishing-a-development-branch` — only when the maintainer wants integration.

Load any other skill with even a 1% chance of applying (`find-skills` if a capability gap appears).

---

## 2. PHASE 1 — baseline & charter (lead + parallel read-only agents)

- Snapshot baseline to `.superpowers/full-rust-framework/baseline.txt`: `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, `cargo clippy --all -- -D warnings`, both benchmarks.
- Confirm and ledger: CI gap, committed `.so` files, dropped flags, OS metadata contracts, shared SQLite schema, CLI flag/subcommand count, ~48 JSONL event names.
- **Write the framework charter** (`docs/superpowers/framework-charter.md`): crate list, public trait signatures, feature-flag matrix, error model, semver policy, docs/examples/benches requirements. The lead approves it before any crate is scaffolded.
- Produce the plan with exact wave/task map, crate ownership, and success criteria.

---

## 3. PHASE 2 — multi-wave subagent swarms (disjoint file ownership)

Each agent writes a report to `.superpowers/full-rust-framework/taskN-report.md` and returns only a short summary (status, files changed, one-line test result, concerns). Self-contained prompts: scope, relevant STEP ZERO facts, global constraints. Do NOT paste session history. Wave N+1 starts only after Wave N diffs pass their spec + quality reviews.

**Global constraints:** `src/paradl` and Python tests are READ-ONLY until Wave 5; preserve §0 contracts exactly; verify behavior by diffing against the Python oracle; TDD RED→GREEN; every public item documented with a runnable example; disjoint file ownership; no commits on `main`; no push unless asked.

### Wave 0 — charter, workspace, oracle (dispatch in parallel)
- **W0-1 Workspace/CI/distribution**: Cargo workspace skeleton with all crate manifests and feature flags from the charter; CI running `cargo test --all` + `cargo clippy --all -- -D warnings` on macOS + Windows; remove committed `.so`; rewrite Homebrew formula and install scripts for a native binary; produce a placeholder `paradl --version`.
- **W0-2 Contract oracle** (`crates/paradl-oracle/**`): Rust golden fixtures mirroring `test_models.py`, `test_classifier.py`, `test_watchdog.py`, Rust parity/e2e tests, JSONL shapes, sidecars, `report.json` v1.0. RED tests first.
- **W0-3 `paradl-core`**: models, helpers, config, paths, clock abstraction, public `Error` enum mapped to `ExitCode`. Crate docs, README, example. Verified against `test_models.py`/`test_config.py`.
- **W0-4 Parity inventory** (read-only): every CLI flag/subcommand, JSONL event, config field, DB table, PyO3 surface item, env var → `docs/superpowers/parity-matrix.md`. No wave finishes without clearing its rows.

### Wave 1 — framework core services (dispatch in parallel)
- **W1-1 `paradl-store`**: `Store` trait + rusqlite impl; 7 history tables + 3 throttle tables, schema v4 migrations, WAL, device+inode guard, byte-compatible with shipped `history.db`. Port `test_sqlite_guard.py` invariants. Example: open/migrate/round-trip a copied DB.
- **W1-2 `paradl-engine`**: promote `rapids`; public `DownloadEngine` trait; planner/state/limiter/verify/probe/download; DoH via hickory (ruling 2), IPv6, recycle, verify-then-rename. Port `rust/rapids/src/tests.rs` and Python parity/e2e/watchdog-recycle tests. Example: download one URL with N channels.
- **W1-3 `paradl-watchdog`**: `Watchdog` trait, v1/v2 state machine, percentile, cooldown, learned profiles, supervision loop with recycle; byte-exact `watchdog_action` events. Port `test_watchdog.py`.
- **W1-4 `paradl-net`**: reqwest client, Range/GET probe, RFC 6266 filename + ETag digest parsing, `TargetResolver` trait, DNS/DoH, proxy/IP-family, retry. Port `test_net.py` parser cases. Example: custom `TargetResolver`.

### Wave 2 — framework platform services (dispatch in parallel)
- **W2-1 `paradl-throttle`**: classifier (Mann-Whitney, next-best-hours), parsers, budget, probes, targets, tokio VPS server, report generators; mirror `test_classifier.py` seeded cases byte-for-byte. Example: classify a recorded dataset.
- **W2-2 `paradl-postproc` + `paradl-platform`**: `PostProcessor` trait; checksum (blake2b, GNU/BSD text, companion), security-validated ZIP/tar, ADB; `Platform` trait with macOS/Windows feature-gated impls for power/network/sleep/keychain/WhereFroms/Zone.Identifier; golden-file parity (ruling 3). 7z/DMG/hdiutil external.
- **W2-3 `paradl-net` resolvers**: github, hf, brew, pypi, ipsw, cloud as `TargetResolver` impls — REST resolution, asset/wheel scoring, token flows, digest verification, direct-URL rewrite; delegate to `paradl-engine`. Port `test_bug_fixes.py`, `test_github_acceleration.py`, `test_pypi_transitive.py`.
- **W2-4 resilience + `paradl-events`**: queue scheduling, launchd plist, hooks with 30s timeout + JSON-stdin, resilience event source (WAKE/NET_CHANGE/STALL), auth, and the public JSONL event schema/redaction with version constant (ruling 4). Golden tests over all ~48 event shapes.

### Wave 3 — framework surfaces (dispatch in parallel)
- **W3-1 `paradl-cli`**: clap parser as a reusable crate — all ~60 flags, 20 subcommands, exact help/usage/error text, env vars, `--engine`; dispatch trait so other binaries can reuse it. Port `test_bug_fixes.py` CLI cases and `test_perf_parity.py`.
- **W3-2 `paradl-ui`**: feature-gated progress display and ratatui TUI (filters, task list, add-job screen, actions), consuming `paradl-events`; golden output tests.
- **W3-3 `paradl-api`**: feature-gated axum server with the 7 REST endpoints, serde validation equivalent to pydantic `Field(ge/le)`, task store/runner, lifecycle. Example: spawn the server with a custom `Store`.
- **W3-4 `paradl-media`**: feature-gated `MediaResolver` impl driving external yt-dlp (ruling 1), metadata via `id3`/`mp4ameta` + ffmpeg fallback, ffmpeg/afconvert conversion, cover embedding, direct-URL handoff to `paradl-engine`.

### Wave 4 — reference binary, parity, performance (dispatch in parallel)
- **W4-1 `paradl` binary + `paradl-app`**: thin reference consumer wiring all crates; diagnostics (doctor, netprobe, bench); full subcommand coverage; verify every row of `parity-matrix.md`.
- **W4-2 Differential parity harness** (owns `crates/paradl-parity/**`): run Python CLI and Rust binary against the same loopback servers; diff stdout, stderr, JSONL, files/bytes, sidecars, exit codes; fail on any difference. This is the cutover gate.
- **W4-3 Framework quality**: `cargo doc --no-deps` clean with no missing docs on public items; one runnable example per crate passing; feature-flag matrix builds (`--no-default-features`, each flag, all flags); benchmark crates with pinning/sentinels, cold/warm separation, ≥3 reps, `null`+reason, drift caveat.

### Wave 5 — cutover & release (only after all prior waves accepted and W4-2 green)
- **W5-1** Delete `src/paradl/**`, Python tests, `pyproject.toml`, `parallel-dl.py`, both committed `.so` files; point entry points and docs at the native binary; finalize Homebrew formula and install scripts.
- **W5-2** Framework release prep: semver `1.0.0`, CHANGELOG, LICENSE headers, `docs.rs` metadata, README quickstart showing the framework being used as a library and as the `paradl` CLI; final full verification; ledger close-out.

Lead owns integration, per-diff spec + quality review, arbitration at the 5-round cap, and the final gate. Do not edit subagent-owned files while they run.

---

## 4. PHASE 3 — review loop (per task, before acceptance)

For each returned diff: generate a review package (`git diff -U10` + new-file contents) under `.superpowers/full-rust-framework/reviews/taskN.txt`; dispatch a reviewer that must output `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes` with severity-tagged findings (`Critical/Important/Minor`) and `file:line`. Failed → fix loop with the SAME owning agent (resume its session); re-review only the fix diff; cap 5 rounds, then adjudicate with a ledger ruling. Do not start the next wave until the current wave's diffs are accepted. After Wave 5: one broad whole-branch review on the most capable available model; triage deferred findings into the ledger.

---

## 5. SUCCESS CRITERIA (acceptable evidence = raw command output)

- `cargo test --all`, `cargo clippy --all -- -D warnings`, `cargo build --release`, `cargo doc --no-deps` green on macOS and Windows; `paradl --version` and `doctor` work with no Python installed.
- Every public item has rustdoc; `cargo test --doc` passes all crate examples; the feature-flag matrix in the charter builds with no warnings.
- Differential harness W4-2 reports **zero differences** across stdout, stderr, JSONL, files, sidecars, exit codes for the full oracle corpus.
- Every row of `docs/superpowers/parity-matrix.md` marked pass with proving output; every §0 contract literal verified unchanged.
- `history.db` from a shipped Python install opens, migrates, and round-trips through the `paradl-store` API.
- External tools invoked per rulings: `yt-dlp`, `ffmpeg`, `afconvert`, `7z`, `hdiutil`, `adb` — no Python process anywhere.
- Homebrew formula and both install scripts install a working native binary from a clean machine; no Python runtime dependency.
- Benchmarks: cold medians, identical URLs, pinning/sentinels, ≥3 reps; 1.2–1.3× drift NOT a win; `null`+reason for unmeasurable.
- `git status` reviewed; nothing committed on `main`; no push unless asked.

---

## 6. FINAL OUTPUT (plain English)

1. **TL;DR** — the framework crate map, what the binary does, and the one-line install command.
2. **Framework table** — each crate: public role, extension traits, feature flags, example path, test count.
3. **Parity table** — every CLI flag/subcommand, JSONL event, DB table, and contract literal with its proving test.
4. **Cutover audit** — Python deleted, no `.so`, no Python runtime, Homebrew/install scripts verified on a clean machine.
5. **External-tool inventory** — which binaries are invoked and for what; confirmation no Python process is spawned.
6. **Honesty notes** — HTTP/3/DoH ruling outcome, media limitations, benchmark drift caveat.
7. **Open items** — anything deferred into the ledger.

Begin now: load the skills, create the worktree, read STEP ZERO, write the framework charter and plan, then dispatch Wave 0.
