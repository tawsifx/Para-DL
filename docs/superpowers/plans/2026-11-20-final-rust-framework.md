# Final Rust Framework + Python Media Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the entire paraDL Python application with a Cargo workspace of public Rust crates and a thin `paradl` binary, except the media subsystem which stays a standalone Python package invoked over `paradl-media/1`.

**Architecture:** Strangler-fig. `src/paradl/**` and Python `tests/**` remain untouched through Wave 4 as the executable oracle. New crates are built in `rust/crates/`, verified by diffing observable behavior against the Python oracle. Wave 5 deletes non-media Python. Media is a one-shot `python -m paradl_media` worker with a versioned JSON-lines protocol; Rust owns the protocol client, all JSONL emission, and the full exit-code mapping.

**Tech Stack:** Rust 2024 edition, Tokio, reqwest/rustls, clap, rusqlite, serde/serde_json, hickory-dns, ratatui, axum, blake2/zip/tar; Python 3.11+ worker with yt-dlp[default], mutagen, optional yt-dlp-ejs.

**Spec / authority:** FINAL master prompt (`docs/master-prompts/FINAL-rust-framework-with-python-media.md`); architecture charter (`docs/superpowers/framework-charter.md`); parity matrix (`docs/superpowers/parity-matrix.md`). Conflicts resolve against the FINAL master prompt.

**BASE:** `b51e6cc11c063f89c76f75c7e4aca1e794ca92b8`. Worktree: `final-rust-framework` branch. Never commit on `main`; never push unless asked.

---

## Global Constraints

- Media stays Python; prior no-Python criteria are VOID. Media = one-shot worker, not sidecar, not PyO3.
- `paradl-events` is the only JSONL emitter; worker stdout is protocol-only; humans/logs → stderr.
- Rust owns the full ExitCode map: `0/1/2/3/4/130`. Worker emits `error.kind` only, never `3`.
- Protocol `paradl-media/1`: version handshake, `request_id`, versions+caps in terminal frames; unknown major → exit 4; fail closed on missing version.
- Byte-exact worker contracts: `http_headers` values→str, keys unchanged; `filename` via sanitize_filename (200-char cap, Windows reserved); `audio_format_produced` exact strings (`media.py:48–72`).
- Routing split: Rust replicates pure rules (domain lists, extension suffix, `ytsearch1:`, HEAD/Range `text/html`→STREAM) from one generated constant; worker owns `ytdlp_extractor_claims_url`, full metadata, direct-vs-managed handoff. `claims=false`→DIRECT fallback.
- Dropped flags `http3`, `doh_endpoint`, `accept_ranges`, `total`: keep CLI flags, implement DoH via hickory-dns, log-drop HTTP/3, do NOT expose a feature for an unimplemented capability.
- Byte-exact OS/data contracts: plist xattr hex, `Zone.Identifier` CRLF INI, keychain secrets-over-stdin, `history.db` schema v4 (WAL, shared `queue_jobs`, device+inode guard), JSONL `{event, timestamp: round(time.time(),3), ...redacted}`, `report.json` schema `1.0`, `MAX_CONNECTIONS=16` with `min(max(n,1),16)`, `WatchdogActionType.tier2_pause_unpause`, close-delimited 200 with no false-100%, cross-host credential non-leak.
- Distribution: native binary owns `paradl`/`parallel-dl`; media package has NO console script; `parallel-dl.py` dev-only; Python+yt-dlp optional media dependency; no committed `.so`.
- `src/paradl/**` and `tests/**` READ-ONLY through Wave 4. TDD RED→GREEN. Disjoint file ownership. Every public item documented with a runnable example.
- Host note: cargo needs `PYO3_PYTHON=<worktree>/.venv/bin/python` (3.12) for the legacy `paradl-rs` crate; new crates must not depend on PyO3.

## Review Focus

- Protocol/version handshake: unknown/missing `paradl-media/1` major must fail closed with exit 4 and a clear diagnostic — covered by boundary lane (W4-2) and W3-4b contract tests.
- Single-emitter integrity: a chatty worker writing to stdout must not corrupt `--json` — covered by boundary lane "malformed" stub (W4-2).
- Byte-exact numeric parity: Python `round()` (bankers) vs `f64::round`, `statistics.median/fmean/pstdev`, f-string vs `format!` — covered by W2-1 12 seeded classifier cases.
- Process policy: no Python on non-media paths — covered by `PARADL_ASSERT_NO_PYTHON=1` guard in the non-media lane (W4-2).
- OS metadata goldens: plist hex / Zone.Identifier CRLF / keychain stdin — covered by W2-2 golden tests.
- Shared SQLite schema: `history.db` v4 with `queue_jobs` must open/migrate/round-trip — covered by W1-1 + success criterion 4.

---

## Wave 0 — charter, workspace, oracle, distribution (parallel)

Run only after charter + plan + parity matrix approved by lead. Disjoint ownership.

### Task W0-1 — Workspace, CI, distribution
**Owns:** `rust/Cargo.toml`, all 17 crate `Cargo.toml` manifests, `.github/workflows/ci.yml`, `Formula/parallel-dl.rb`, `scripts/install-local.sh`, `scripts/install-local.ps1`, deletion of committed `.so`, placeholder binary `rust/paradl/src/main.rs`.
- [ ] Add all workspace members + feature-flag declarations; crate manifests with semver `0.1.0`, docs, examples.
- [ ] CI jobs: `cargo build/test/clippy/fmt` on `macos-14`, `macos-13`, `windows-latest`; media-package job on Py3.11/3.12; cross-boundary integration gate incl. protocol-mismatch negative test → exit 4.
- [ ] Delete the two committed `.so` files.
- [ ] Rewrite Formula: Rust build into `bin`, media venv in `libexec`, `depends_on "rust"`, keep `python@3.12`/`ffmpeg`, `deno` recommended, drop default `aria2`, test block exercises handshake.
- [ ] Rewrite both install scripts: build/copy binary, create venv, `uv pip install` minimal media package, symlinks → binary, `PARADL_MEDIA_PY`.
- [ ] Placeholder `paradl --version`.
- **Verify:** `cargo build --all` green; CI YAML parses; no `.so` in git tree; `--version` prints.

### Task W0-2 — Contract oracle
**Owns:** `rust/crates/paradl-oracle/**`.
- [ ] RED golden fixtures mirroring `test_models.py`, `test_classifier.py`, `test_watchdog.py`, Rust parity/e2e tests, JSONL shapes, sidecars, `report.json` v1.0, and the `paradl-media/1` schema.
- **Verify:** `cargo test -p paradl-oracle` compiles with failing (ignored until implemented) assertions captured; fixtures byte-match Python output.

### Task W0-3 — `paradl-core`
**Owns:** `rust/crates/paradl-core/**`.
- [ ] Models, helpers, config, paths, clock abstraction, public `Error`→ExitCode; crate docs, README, example. Verified against `test_models.py`/`test_config.py`.
- **Verify:** `cargo test -p paradl-core`, `cargo doc -p paradl-core --no-deps`, doc-test green.

### Task W0-4 — Parity inventory (read-only)
**Owns:** `docs/superpowers/parity-matrix.md` (fill only).
- [ ] Enumerate every CLI flag/subcommand, JSONL event, config field, DB table, contract literal, protocol field from STEP ZERO; tag each row with owning wave.
- **Verify:** matrix row count covers all rows in §2 success criteria.

**Wave 0 gate:** all 4 task diffs pass spec + quality review; BASE recorded before each implementer dispatch.

---

## Wave 1 — framework core services (parallel)

- **W1-1 `paradl-store`:** `Store` trait + rusqlite; 7 history + 3 throttle tables, v4 migrations, WAL, device+inode guard, byte-compatible with shipped `history.db`. Port `test_sqlite_guard.py`.
- **W1-2 `paradl-engine`:** promote `rapids`; public `DownloadEngine` trait; planner/state/limiter/verify/probe/download; DoH via hickory, IPv6, recycle, verify-then-rename. Port `rapids/src/tests.rs` and Python parity/e2e/watchdog-recycle tests.
- **W1-3 `paradl-watchdog`:** `Watchdog` trait, v1/v2 state machine, percentile, cooldown, learned profiles, supervision loop; byte-exact `watchdog_action` events. Port `test_watchdog.py`.
- **W1-4 `paradl-net`:** reqwest client, probe, RFC 6266 filename + ETag digest, `TargetResolver` trait, DNS/DoH, proxy/IP-family, retry; pure routing rules. Port `test_net.py` + `test_engine_batch.py` routing cases.

**Wave 1 gate:** `cargo test --all` green; `paradl-store` opens shipped v4 DB; routing cases match Python oracle.

---

## Wave 2 — framework platform services (parallel)

- **W2-1 `paradl-throttle`:** classifier (Mann-Whitney, next-best-hours), parsers, budget, probes, targets, tokio VPS server, report generators. Mirror `test_classifier.py` 12 seeded cases byte-for-byte.
- **W2-2 `paradl-postproc` + `paradl-platform`:** `PostProcessor` trait; checksum (blake2b, GNU/BSD text, companion), security-validated ZIP/tar, ADB; `Platform` trait with macOS/Windows feature-gated impls, byte-exact metadata/keychain goldens. 7z/DMG/hdiutil external.
- **W2-3 site engines (`paradl-net`):** github, hf, brew, pypi, ipsw, cloud as `TargetResolver` impls; REST resolution, asset/wheel scoring, token flows, digests, direct-URL rewrite. Port `test_bug_fixes.py`, `test_github_acceleration.py`, `test_pypi_transitive.py`.
- **W2-4 resilience/auth/events:** queue scheduling, launchd plist, hooks (30s timeout, JSON stdin), resilience event source (WAKE/NET_CHANGE/STALL), auth; `paradl-events` public JSONL schema + redaction with version constant; single-emitter rule enforced. Golden tests over all ~48 event shapes.

**Wave 2 gate:** classifier 12/12 byte-exact; event goldens pass; OS metadata goldens byte-exact.

---

## Wave 3 — framework surfaces + media bridge (parallel)

- **W3-1 `paradl-cli`:** clap parser — all ~60 flags, ~20 subcommands, exact help/usage/error text, env vars, `--engine`; dispatch trait. Port CLI cases from `test_bug_fixes.py` and `test_perf_parity.py`.
- **W3-2 `paradl-ui`:** feature-gated progress display and ratatui TUI consuming `paradl-events`; golden output tests.
- **W3-3 `paradl-api`:** feature-gated axum server, 7 endpoints, serde validation for pydantic `Field(ge/le)`, task store/runner.
- **W3-4a Python media package** (`media/**`, `media/pyproject.toml`): extract `media.py` yt-dlp/mutagen/ffmpeg logic into standalone module with NO `paradl.*` imports; move JS/EJS selection + `importlib.metadata.version("yt-dlp-ejs")` probe from `toolchain.py`; deps `yt-dlp[default]`+`mutagen` (+optional ejs); `resolve`/`download_managed`/`finalize`/`claims` ops; structured errors; SIGINT→130; stdout protocol-only. Retarget media tests; add golden request→response fixtures with yt-dlp/mutagen/ffmpeg mocked.
- **W3-4b `paradl-media` Rust crate** (`crates/paradl-media/**`): `MediaResolver` impl that discovers venv Python, spawns one-shot worker, sends request, parses progress/result lines, maps `error.kind`→ExitCode, enforces handshake, supervises lifecycle (timeout, SIGINT forwarding, no orphans, cleanup), re-emits events via `paradl-events`. No extractor logic in Rust.

**Wave 3 gate:** `--help`/error text matches oracle; media handshake contract tests pass N/N−1/N+1/unknown/missing; no `paradl.*` imports in `media/`.

---

## Wave 4 — reference binary, 3-lane parity, quality (parallel)

- **W4-1 `paradl` + `paradl-app`:** thin reference binary wiring all crates; diagnostics (`doctor` reports binary/media/protocol/yt-dlp versions + JS-runtime status; `netprobe`, `bench`); full subcommand coverage; verify every parity row.
- **W4-2 3-lane differential harness** (`crates/paradl-parity/**`):
  - Non-media lane (main gate): intact Python CLI vs Rust binary on same loopback servers; diff stdout/stderr/JSONL/files/`.pdl.*`/exit codes with `PARADL_ASSERT_NO_PYTHON=1`.
  - Media lane (golden-replay, no live network): recorded yt-dlp `-J` metadata + artifact bytes replayed via fake yt-dlp/subprocess; feed identical fixture to Python `MediaEngine` and Rust bridge; diff protocol response, JSONL, output bytes, tags.
  - Boundary lane: stub Python worker emits `ok/error/unsupported/malformed/version-mismatch`; Rust parses, re-emits, maps codes, fails closed.
- **W4-3 Framework quality + benchmarks:** `cargo doc --no-deps` clean; `cargo test --doc`; feature matrix (`--no-default-features`, each flag, all flags); both benchmark harnesses with pinning/sentinels, cold/warm separation, ≥3 reps, CPU/RSS, `null`+reason, drift caveat. Media startup measured on media lane only.

**Wave 4 gate:** W4-2 fully green (zero non-media differences); all feature combos build; docs clean.

---

## Wave 5 — cutover (only after all prior waves accepted)

- **W5-1:** Delete only non-media Python: `src/paradl/**` except extracted media surface, bulk Python tests, `parallel-dl.py`; retain `media/` + minimal `pyproject.toml`. Point entry points/docs at native binary; finalize Formula/install scripts.
- **W5-2:** Framework release: semver `1.0.0`, CHANGELOG, LICENSE headers, docs.rs metadata, README quickstart (library + CLI + optional media); run protocol version-compat matrix; final verification; ledger close-out.

---

## Verification commands (exact)

```bash
# baseline / final
uv run pytest -q
uv run ruff check src tests
uv run mypy src
(cd rust && PYO3_PYTHON="$PWD/../.venv/bin/python" cargo test --all)
(cd rust && PYO3_PYTHON="$PWD/../.venv/bin/python" cargo clippy --all -- -D warnings)
(cd rust && cargo build --release)
(cd rust && cargo doc --no-deps)
(cd rust && cargo test --doc)
# per-crate
cargo test -p <crate>; cargo doc -p <crate> --no-deps; cargo run -p <crate> --example <name>
# feature matrix
cargo build --no-default-features; cargo build --all-features
# parity
uv run pytest rust/crates/paradl-parity -q
```
