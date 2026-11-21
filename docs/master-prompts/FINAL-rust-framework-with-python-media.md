# MASTER PROMPT (FINAL) — paraDL Rust Framework with Python Media Worker

> This supersedes `rust-replace-swarm.md`, `full-rust-rewrite-swarm.md`, and `full-rust-framework-swarm.md`. Those files are historical background only. The maintainer's final ruling: **everything becomes Rust except media, which stays Python.** Give this to a fresh agent session; it is self-contained. The agent must not ask "what next"; make rulings in a ledger and keep going. Only stop for irreversible/shared-branch git operations or a plan so broken every path is a guess.
>
> **Execution model:** many parallel subagent swarms across several waves (Wave 0 charter → Wave 1 core → Wave 2 services → Wave 3 framework + media bridge → Wave 4 reference binary + 3-lane parity → Wave 5 cutover), orchestrated with `dispatching-parallel-agents` + `subagent-driven-development`, with a two-verdict review after every diff. Do not collapse it into a single agent doing everything sequentially — use the swarms as specified in §3.

You are the **lead architect** building a Rust framework that replaces the entire Python application **except the media subsystem**, which remains a small, self-contained Python package. Deliverables:

1. A **reusable Rust framework** — a Cargo workspace of public, semver-versioned, documented crates with trait-based extension points, feature flags, examples, and benches.
2. A **thin `paradl` binary** — the reference consumer, owning `paradl`/`parallel-dl`, handling every non-media path.
3. A **Python media worker** — `yt-dlp`/`mutagen`/`ffmpeg` logic extracted from `media.py` into a standalone module with no `paradl.*` imports, invoked by the binary over a versioned protocol.

## STEP ZERO — read before anything else

1. `/Users/tawsifx/paradl/docs/rust-replaceability-report.md` — full analysis: tiers, domain inventory, contracts, blockers.
2. `/Users/tawsifx/paradl/docs/master-prompts/full-rust-framework-swarm.md` — prior architecture. **VOID:** its "no Python runtime / no PyO3 / no Python sidecar under any circumstance" ruling (§0.1), its "no Python process anywhere" success criterion (§5), and its `paradl-media` Rust-extractor plan. Do not restore them.
3. `PARALLEL_DL_DEV_CONTEXT.md` — §7 perf audit, `mdm-engine`→`rapids` migration, honesty caveats (warm-cache and ~1.2–1.3× drift are NOT wins).
4. `README.md`, `pyproject.toml`, `rust/Cargo.toml`, `rust/paradl-rs/src/lib.rs`, `Formula/parallel-dl.rb`, `scripts/install-local.{sh,ps1}`, `.github/workflows/ci.yml`.
5. Executable oracle (DO NOT EDIT until Wave 5): `src/paradl/**`, `tests/**` (6,430 LOC), `rust/rapids/src/tests.rs`, `rust/paradl-rs/contract_tests/**`, `benchmarks/**`.
6. Git state: worktree `/Users/tawsifx/paradl`, branch `main`, HEAD `b51e6cc`. Record `BASE=$(git rev-parse HEAD)`. Use an isolated worktree; never work on `main` directly.
7. Create ledger `.superpowers/final-rust-framework-ledger.md`. **First ledger entry, before any other work:** `Ruling: prior no-Python criteria STRUCK — media stays Python by maintainer decision — cost if wrong: reviewers reject correct mixed-language diffs`. Every later assumption/ruling is `Ruling: X — why — cost if wrong`.

---

## 0. Architecture and standing rulings

**Scope math (re-analyzed):** 28,323 Python LOC total → **~1,286 stay Python (4.5%)**, **~1,200 deleted (4%)**, **~25,887 become Rust (91%)**.

- **Stays Python:** `engines/media.py` (915), media-relevant `util/toolchain.py` (JS/EJS/version discovery, ~258–330), media-domain helpers `core/models.py:111–175` (~63), `ytdlp_extractor_claims_url` + cached extractor classes in `net/client.py:480–502` (~21).
- **Deleted:** dead aria2 daemon/RPC/loop in `engines/aria2.py` (~1,200).
- **Everything else → Rust.**

**Framework shape (enforced):** Cargo workspace; every crate has crate docs, semver `0.1.0` during development, README, and ≥1 runnable example. Public traits: `DownloadEngine`, `TargetResolver`, `Store`, `Watchdog`, `Notifier`, `Platform`, `PostProcessor`, and **`MediaResolver`** (Rust-side protocol client only — no extractor implementation). Crates: `paradl-core`, `paradl-store`, `paradl-net`, `paradl-engine` (promote `rapids`), `paradl-watchdog`, `paradl-postproc`, `paradl-platform`, `paradl-media` (protocol client/router), `paradl-throttle`, `paradl-events`, `paradl-ui`, `paradl-api`, `paradl-cli`, `paradl-app`, `paradl` (binary). Feature flags replace Python extras: `api`, `tui`, `media`, `platform-macos`, `platform-windows`; default build compiles without optional system tools. Public `paradl_core::Error` maps to the ExitCode contract. Strangler-fig: `src/paradl` and Python tests stay untouched through Wave 4 as the oracle.

**Standing rulings — resolve before the owning wave, never silently:**

1. **Media protocol = one-shot worker.** Rust spawns `<python> -m paradl_media` **per media operation** with a versioned JSON-lines protocol `paradl-media/1`: one request line on stdin; `progress` frames + one terminal `result`/`error` line on stdout; human logs to stderr only. Rejected: long-lived sidecar (weak amortization, high lifecycle cost) and PyO3 embedding (reintroduces the build burden and crashes extractor faults into the binary). Yt-dlp is pip-installed in a binary-managed venv, never PyInstaller-frozen (frozen snapshots defeat weekly updates).
2. **Single emitter rule.** `paradl-events` is the **only** JSONL emitter. The Python worker must never write to stdout except protocol lines; human output goes to stderr; progress becomes protocol events that Rust re-emits with redaction. This prevents interleaved/malformed JSONL and split-brain redaction.
3. **Routing split.** Rust replicates pure rules: `MEDIA_DOMAINS`/`IMAGE_DOMAINS`/`DIRECT_EXTENSIONS` suffix matching, `ytsearch1:` free-text → SEARCH, HTTP HEAD/Range heuristics (`text/html` → STREAM). Rust delegates to the worker: `ytdlp_extractor_claims_url` (live extractor registry), full metadata extraction, and the direct-vs-managed handoff decision (info-dict keys). `claims=false` → DIRECT fallback. Mirror the domain lists in both languages from one generated constant to prevent drift.
4. **Exit codes.** Rust owns the full mapping: `OK=0, FAILURE=1, USAGE=2, VERIFICATION_FAILED=3, MISSING_DEPENDENCY=4, INTERRUPTED=130`. Worker returns structured `error.kind` (`metadata|download|conversion|dependency|usage|interrupted`); worker never emits 3 (checksums stay Rust-owned after `finalize`). Missing yt-dlp/JS runtime → 4; missing ffmpeg/afconvert stays a weak dependency → still 0 with `shutil.move` fallback. Rust propagates 130 and preserves `.pdl.part`/`.pdl.json`.
5. **Protocol version handshake.** Request/response carry `protocol: "paradl-media/1"`, `request_id`, plus `worker_version`, `yt_dlp_version`, `mutagen_version`, `ffmpeg_version`, `caps`. Rust rejects unknown **major** versions with a clear diagnostic and exit 4; tolerates minor additions; fails closed on missing/unparseable version. yt-dlp version is data, not protocol — only schema changes bump it. Contract tests cover N, N−1, N+1, unknown, missing.
6. **Byte-exact contracts from the worker:** `http_headers` values coerced to `str` with keys unchanged; `filename` from `sanitize_filename` (200-char cap, Windows reserved names); `audio_format_produced` exact strings (`classify_audio_production`, `media.py:48–72`). Assert these verbatim against Python output in every protocol test.
7. **Dropped upstream flags:** `http3`, `doh_endpoint`, `accept_ranges`, `total` were accepted-then-discarded; keep CLI flags compatible, implement DoH via `hickory-dns`, drop HTTP/3 with a logged reason. Do not expose a framework feature for an unimplemented capability.
8. **Byte-exact OS/data contracts:** macOS plist xattr hex, `Zone.Identifier` CRLF INI, keychain secrets-over-stdin (goldens from `tests/support/macos_fake.py`, `tests/unit/test_platform_windows.py`); `history.db` schema v4 with shared `queue_jobs` and throttle tables; WAL, migrations, device+inode guard.
9. **JSONL wire format:** `{event, timestamp: round(time.time(),3), ...redacted}`, ~48 event names, success emits `verification(verified=true)` with no `error` event. `report.json` schema `1.0` with `modules.m0..m10`; 12 seeded `test_classifier.py` cases byte-for-byte. `MAX_CONNECTIONS=16` with `min(max(n,1),16)`; `WatchdogActionType.tier2_pause_unpause`; cross-host cookie/`Authorization`/`Proxy-Authorization` non-leak; close-delimited 200 no false-100%.
10. **Distribution:** native binary owns `paradl`/`parallel-dl`; media package has **no console script**; `parallel-dl.py` becomes dev-only. Ship binary + minimal Python package (`yt-dlp[default]` + `mutagen`, optional `yt-dlp-ejs`) in a private venv discovered via `PARADL_MEDIA_PY` > path relative to `current_exe()` > data dirs. Drop `rich`, `httpx`, `packaging`, `fastapi`, `uvicorn`, `pydantic`, `textual` from the Python package. Non-media users need no Python; media users need Python 3.11+ and yt-dlp, surfaced by `paradl doctor`.

---

## 1. MANDATORY SKILLS (obra/superpowers)

Load via `skill` at the start, in this order; track everything in the ledger:

1. `using-superpowers` — skill invocation before any response/action.
2. `brainstorming` — already completed (final ruling: Rust everywhere except media; one-shot worker; strangler-fig). Do not reopen standing rulings unless a wave finds a genuine conflict.
3. `systematic-debugging` — for ANY parity failure: root cause, pattern, single hypothesis + minimal test, single fix. No guessing.
4. `test-driven-development` — every crate and the protocol need RED→GREEN tests mirroring the Python oracle; no implementation before a failing test.
5. `writing-plans` — write `docs/superpowers/plans/<date>-final-rust-framework.md` (file-by-file ownership, exact commands, per-wave success criteria) **before** implementation.
6. `karpathy-guidelines` — surface assumptions, surgical scope, minimum code that solves the problem.
7. `using-git-worktrees` — isolated workspace; record `BASE`; never work on `main`.
8. `dispatching-parallel-agents` — one agent per independent crate/domain; focused self-contained prompts; strict scope; disjoint file ownership; no two agents edit the same file.
9. `subagent-driven-development` — fresh implementer per task; spec + quality review after each diff; fix loops stay with the owning agent (cap 5 rounds); broad whole-branch review on the most capable model at the end.
10. `requesting-code-review` / `receiving-code-review` — verdicts `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes`, severity-tagged findings with `file:line`; technical rigor, not performative agreement.
11. `verification-before-completion` — never claim green/porté/done without fresh complete command output. Evidence before assertions.
12. `finishing-a-development-branch` — only when the maintainer wants integration.

Load any other skill with even a 1% chance of applying.

---

## 2. PHASE 1 — baseline and charter

- Snapshot to `.superpowers/final-rust-framework/baseline.txt`: `pytest -q`, `ruff check src tests`, `mypy src`, `cargo test --all`, `cargo clippy --all -- -D warnings`, both benchmarks.
- Confirm and ledger: CI never builds Rust; two committed arm64/CPython-3.12 `.so` files; dropped flags; OS metadata contracts; shared SQLite schema; CLI flag/subcommand count; ~48 JSONL events; exact media Python surface from STEP ZERO.
- Write `docs/superpowers/framework-charter.md`: crate list, public trait signatures, feature-flag matrix, error model, **media protocol schema `paradl-media/1`**, semver policy, docs/examples/benches rules. Lead approves before scaffolding.
- Write `docs/superpowers/parity-matrix.md`: every CLI flag/subcommand, JSONL event, config field, DB table, contract literal, and protocol field. No wave finishes without clearing its rows.

---

## 3. PHASE 2 — multi-wave swarms (disjoint file ownership)

Each agent writes `.superpowers/final-rust-framework/taskN-report.md` and returns a short summary (status, files changed, one-line test result, concerns). Self-contained prompts; do NOT paste session history. Wave N+1 starts only after Wave N diffs pass both reviews.

**Global constraints:** `src/paradl` and Python tests READ-ONLY until Wave 5; preserve §0 contracts; verify by diffing against the Python oracle; TDD RED→GREEN; every public item documented with a runnable example; disjoint file ownership; no commits on `main`; no push unless asked.

### Wave 0 — charter, workspace, oracle, distribution (parallel)
- **W0-1 Workspace/CI/distribution**: all crate manifests + feature flags; CI jobs `cargo build/test/clippy/fmt` on `macos-14` (arm64), `macos-13` (x64), `windows-latest`, plus a media-package job on Py3.11/3.12 and a cross-boundary integration gate (including protocol-mismatch negative test → exit 4); delete committed `.so`; rewrite `Formula/parallel-dl.rb` (Rust build into `bin`, media venv in `libexec`, `depends_on "rust"`, keep `python@3.12`/`ffmpeg`, `deno` recommended, drop default `aria2`, test block exercises handshake); rewrite both install scripts (build/copy binary, create venv, `uv pip install` minimal media package, symlinks → binary, `PARADL_MEDIA_PY`); placeholder `paradl --version`.
- **W0-2 Contract oracle** (`crates/paradl-oracle/**`): golden fixtures mirroring `test_models.py`, `test_classifier.py`, `test_watchdog.py`, Rust parity/e2e tests, JSONL shapes, sidecars, `report.json` v1.0, **and the `paradl-media/1` schema**. RED tests first.
- **W0-3 `paradl-core`**: models, helpers, config, paths, clock abstraction, public `Error`→ExitCode. Crate docs, README, example. Verified against `test_models.py`/`test_config.py`.
- **W0-4 Parity inventory** (read-only): fill `parity-matrix.md` from STEP ZERO; mark each row's owning wave.

### Wave 1 — framework core services (parallel)
- **W1-1 `paradl-store`**: `Store` trait + rusqlite; 7 history + 3 throttle tables, v4 migrations, WAL, device+inode guard, byte-compatible with shipped `history.db`. Port `test_sqlite_guard.py`.
- **W1-2 `paradl-engine`**: promote `rapids`; public `DownloadEngine` trait; planner/state/limiter/verify/probe/download; DoH via hickory, IPv6, recycle, verify-then-rename. Port `rapids/src/tests.rs` and Python parity/e2e/watchdog-recycle tests.
- **W1-3 `paradl-watchdog`**: `Watchdog` trait, v1/v2 state machine, percentile, cooldown, learned profiles, supervision loop; byte-exact `watchdog_action` events. Port `test_watchdog.py`.
- **W1-4 `paradl-net`**: reqwest client, probe, RFC 6266 filename + ETag digest, `TargetResolver` trait, DNS/DoH, proxy/IP-family, retry; pure routing rules (ruling 3). Port `test_net.py` and `test_engine_batch.py` routing cases.

### Wave 2 — framework platform services (parallel)
- **W2-1 `paradl-throttle`**: classifier (Mann-Whitney, next-best-hours), parsers, budget, probes, targets, tokio VPS server, report generators. Mirror `test_classifier.py` 12 seeded cases byte-for-byte — Python `round()`/`statistics`/format vs Rust `f64::round`/`format!` must match exactly (no Python fallback exists).
- **W2-2 `paradl-postproc` + `paradl-platform`**: `PostProcessor` trait; checksum (blake2b, GNU/BSD text, companion), security-validated ZIP/tar, ADB; `Platform` trait with macOS/Windows feature-gated impls, byte-exact metadata/keychain goldens. 7z/DMG/hdiutil external.
- **W2-3 site engines** (`paradl-net`): github, hf, brew, pypi, ipsw, cloud as `TargetResolver` impls; REST resolution, asset/wheel scoring, token flows, digests, direct-URL rewrite. Port `test_bug_fixes.py`, `test_github_acceleration.py`, `test_pypi_transitive.py`.
- **W2-4 resilience/auth/events**: queue scheduling, launchd plist, hooks (30s timeout, JSON stdin), resilience event source (WAKE/NET_CHANGE/STALL), auth; `paradl-events` public JSONL schema + redaction with version constant; **single-emitter rule enforced** (ruling 2). Golden tests over all ~48 event shapes.

### Wave 3 — framework surfaces + media bridge (parallel)
- **W3-1 `paradl-cli`**: clap parser crate — all ~60 flags, 20 subcommands, exact help/usage/error text, env vars, `--engine`; dispatch trait for reuse. Port CLI cases from `test_bug_fixes.py` and `test_perf_parity.py`.
- **W3-2 `paradl-ui`**: feature-gated progress display and ratatui TUI consuming `paradl-events`; golden output tests.
- **W3-3 `paradl-api`**: feature-gated axum server, 7 endpoints, serde validation for pydantic `Field(ge/le)`, task store/runner.
- **W3-4 Media bridge (two owners, disjoint trees):**
  - **W3-4a `media/` Python package** (`media/**`, `media/pyproject.toml`): extract `media.py`'s yt-dlp/mutagen/ffmpeg logic into a standalone module with **no `paradl.*` imports**; move the JS/EJS selection and `importlib.metadata.version("yt-dlp-ejs")` probe from `toolchain.py` into the worker; deps `yt-dlp[default]` + `mutagen` (+ optional `ejs`). Implement `resolve`/`download_managed`/`finalize`/`claims` ops with the §0 protocol; structured errors; SIGINT→130; never write to stdout except protocol lines. Keep Python tests for media (`test_h3_media_codec_args`, `test_media_domains`, `test_gen_extractor_classes_cached`, image/HTTP routing) retargeted to this package; add golden request→response fixtures with yt-dlp/mutagen/ffmpeg mocked.
  - **W3-4b `paradl-media` Rust crate** (`crates/paradl-media/**`): `MediaResolver` impl that discovers the venv Python, spawns the one-shot worker, sends the request, parses progress/result lines, maps `error.kind`→ExitCode (ruling 4), enforces handshake (ruling 5), supervises process lifecycle (timeout, SIGINT forwarding, no orphans, sidecar cleanup), and re-emits events through `paradl-events`. No extractor logic in Rust.

### Wave 4 — reference binary, 3-lane parity, quality (parallel)
- **W4-1 `paradl` + `paradl-app`**: thin reference binary wiring all crates; diagnostics (`doctor` reports binary/media/protocol/yt-dlp versions and JS-runtime status; `netprobe`, `bench`); full subcommand coverage; verify every `parity-matrix.md` row.
- **W4-2 Three-lane differential harness** (`crates/paradl-parity/**`):
  - **Non-media lane (main gate):** intact Python CLI vs Rust binary on the same loopback servers; diff stdout/stderr/JSONL/files/`.pdl.*`/exit codes with `PARADL_ASSERT_NO_PYTHON=1` set — any Python spawn fails the test.
  - **Media lane (golden-replay, no live network):** recorded yt-dlp `-J` metadata and artifact bytes replayed via a fake yt-dlp/subprocess; feed identical fixture to Python `MediaEngine` and the Rust bridge; diff protocol response, synthesized JSONL, output bytes, tags.
  - **Boundary lane:** stub Python worker emits canned `ok/error/unsupported/malformed/version-mismatch` responses; Rust must parse, re-emit, map codes, and fail closed.
- **W4-3 Framework quality + benchmarks**: `cargo doc --no-deps` clean (no missing docs on public items); `cargo test --doc`; feature-flag matrix (`--no-default-features`, each flag, all flags); both benchmark harnesses with pinning/sentinels, cold/warm separation, ≥3 reps, CPU/RSS, `null`+reason, drift caveat. Measure media process-startup on the media lane only.

### Wave 5 — cutover, only after all prior waves accepted and W4-2 fully green
- **W5-1** Delete **only** non-media Python: `src/paradl/**` except the extracted media surface, bulk Python tests, `parallel-dl.py`; **retain** `media/` and its minimal `pyproject.toml`. Point entry points and docs at the native binary; finalize Formula/install scripts.
- **W5-2** Framework release: semver `1.0.0`, CHANGELOG, LICENSE headers, docs.rs metadata, README quickstart (library use + CLI + optional media setup); run the protocol version-compat matrix; final verification; ledger close-out.

Lead owns integration, per-diff spec + quality review, arbitration at the 5-round cap, and the final gate. Do not edit subagent-owned files while they run.

---

## 4. PHASE 3 — review loop

For each diff: review package (`git diff -U10` + new files) under `.superpowers/final-rust-framework/reviews/taskN.txt`; reviewer outputs `SPEC COMPLIANCE pass/fail` and `CODE QUALITY Approved/Needs changes` with severity-tagged findings and `file:line`. Failed → fix loop with the SAME owning agent; re-review only the fix diff; cap 5 rounds, then ledger ruling. No next wave until the current wave's diffs are accepted. After Wave 5: broad whole-branch review on the most capable model; triage deferred findings into the ledger.

---

## 5. SUCCESS CRITERIA (acceptable evidence = raw command output)

1. `cargo test --all`, `cargo clippy --all -- -D warnings`, `cargo build --release`, `cargo doc --no-deps`, `cargo test --doc` green on `macos-14`, `macos-13`, `windows-latest`; `paradl --version` and `doctor` work **with no Python installed for all non-media paths**.
2. Non-media lane: **zero differences** (stdout, stderr, JSONL, files, sidecars, exit codes) with `PARADL_ASSERT_NO_PYTHON=1`; media lane matches recorded goldens; boundary lane passes all stub cases.
3. Every `parity-matrix.md` row passes; every §0 literal verified unchanged, including worker-produced `http_headers`, `filename`, `audio_format_produced` asserted verbatim against Python output.
4. `history.db` (v4, WAL, shared `queue_jobs`) opens, migrates, round-trips through `paradl-store`.
5. **Process policy:** no Python process on non-media paths (guard-enforced); a Python process is spawned **only** for media routes, exclusively via `python -m paradl_media`. ffmpeg/afconvert/7z/hdiutil/adb/JS runtimes external as before.
6. **Handshake:** `paradl doctor` reports binary/media/protocol/yt-dlp versions; unknown major version → clear diagnostic + exit 4; missing yt-dlp → exit 4 with install instructions and no traceback.
7. **Version-compat matrix:** binary × worker at N, N−1, N+1, unknown, missing — pass/fail exactly as documented; both sides run the schema contract test.
8. **Event handoff:** in `--json`, a media job emits exactly the documented shapes with no stdout corruption from the child; redaction applied by Rust's emitter.
9. Distribution: Homebrew formula + both install scripts install a working binary from a clean machine; Python + yt-dlp declared as an **optional media dependency**; no committed `.so`; CI native tests run, not skip.
10. Benchmarks: cold medians, identical URLs, pinning/sentinels, ≥3 reps; 1.2–1.3× drift NOT a win; `null`+reason for unmeasurable; media startup measured on the media lane only.
11. `git status` reviewed; nothing committed on `main`; no push unless asked.

---

## 6. FINAL OUTPUT (plain English)

1. **TL;DR** — crate map, what the binary owns, what the Python worker owns, one-line install command.
2. **Framework table** — each crate: public role, extension traits, flags, example path, test count.
3. **Parity table** — every CLI flag/subcommand, JSONL event, DB table, contract literal, and protocol field with its proving test.
4. **Cutover audit** — non-media Python deleted, `.so` gone, no Python on non-media paths, Homebrew/install verified clean, media worker retained.
5. **External-tool inventory** — which binaries are invoked by which side; confirmation of the process policy.
6. **Honesty notes** — DoH/HTTP-3 outcome, media limitations, benchmark drift caveat.
7. **Open items** — anything deferred into the ledger.

Begin now: load the skills, create the worktree, strike the stale rulings in the ledger, read STEP ZERO, write the charter/plan/parity matrix, then dispatch Wave 0.
