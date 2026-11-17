# Native Rust `mdm-engine` Replacement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace the external `aria2c` daemon + JSON-RPC transport with an in-process PyO3 extension built on vendored `mdm-engine` code, preserving every `paradl` CLI flag, `ExitCode` contract, Watchdog v2 recycle behavior, JSONL events, mirror/metalink/cookie support, and the Python test suite.

**Architecture:** A trimmed `mdm-engine` crate (pure planner, resume state, token-bucket limiter, checksum verifier + a new Tokio worker pool and probe) is wrapped by a PyO3 crate `paradl-rs` exposing `RustDownloadEngine`/`DownloadHandle`. Python's GIL is released during transfers; `Snapshot` progress is pushed into the existing `WatchdogController`, and `recycle_connections()` reassigns segments at committed offsets instead of pause/unpausing a daemon URI pool. A Python `RustEngine` adapter conforms to `engines.base.Engine` and preserves `TransferResult`. `aria2.py` stays as an opt-in `--engine aria2` fallback.

**Tech Stack:** Rust (edition 2024, MSRV as MDM requires), Tokio, reqwest(rustls), PyO3, maturin; Python 3.12, pytest, ruff, mypy.

**Spec:** Mission brief (this conversation); MDM reference at `/tmp/mdm-ref/src`; diagnosis at `docs/diagnostics/watchdog-cycle-aria2-bug.md`.

## Global Constraints

- Worktree `feat/rust-mdm-engine` only; do not touch `main` or `perf/max-efficiency`.
- Preserve all CLI flags, `ExitCode` values, `TransferResult` fields, JSONL event names/shapes, mirror/metalink/cookie/header/proxy/ip_family behavior, and the shim modules.
- Max 16 connections per transfer; all new concurrency bounded and timeouts present.
- **License:** MDM is `GPL-3.0-or-later`; `paradl` declares `MIT` in `pyproject.toml`. Preserve MDM copyright/GPL headers on every ported file and FLAG the license conflict for user review. Do not silently change the root license.
- No WebAssembly/GPU claims. Honest benchmarks only; separate real wins from noise.
- TDD: every new behavior gets a failing test first (Rust `#[test]` and Python `pytest`).
- Verification gate: `cargo test`, `pytest -q`, `ruff check src tests`, `mypy src`, throughput benchmark — all with fresh raw output.

## Review Focus

- Redirects: a 302 to a host lacking `Accept-Ranges` must not lose the URI or falsely report 100% (the exact bug diagnosed in `docs/diagnostics/watchdog-cycle-aria2-bug.md`).
- Recycle mid-transfer: committed offsets preserved; uncommitted claims re-fetched; no URI loss.
- Resume: `.mdm.part` + `.mdm.json` round-trip; legacy `.aria2` still cleaned by `paradl clean`.
- Checksum: SHA-256 mismatch yields `ExitCode.VERIFICATION_FAILED`.
- Cookie scoping: host-scoped jar never leaks across a cross-domain redirect.
- GIL: no long-held GIL during transfer; `py.allow_threads` wraps the Tokio runtime handoff.

## Rulings

- **Trimmed vendor, not full workspace.** MDM's workspace pins git forks (storytold/craft-libs, anefzaoui/egui/rqbit) and `rust-version = 1.98`. Vendor only `planner.rs`, `state.rs`, `limiter.rs`, `verify.rs` (+ new `probe.rs`, `download.rs`, `error.rs`, `lib.rs`) into `rust/mdm-engine`, replacing `craft-atomic::atomic_write` with std temp-file+rename and using crates.io deps only. Do NOT pull `wreq`/BoringSSL (needs CMake/NASM/libclang), `rquickjs` (C build), FTP, torrent, or site modules.
- **Maturin config lives at `rust/paradl-rs/pyproject.toml`**, not the root, so the existing Python build backend stays intact. The built module is imported as `paradl._rust_engine` via a Python shim + `.pyi` stub.
- **aria2 stays for torrent/magnet/metalink** in v1 (MDM's `mdm-torrent` is out of scope); Rust becomes the default for HTTP(S) only. This avoids silently breaking those CLI contracts.
- Toolchain install via `brew install rust` (documented fallback: the mandated `paradl` downloader binary is absent at `/opt/homebrew/bin/paradl`, and the in-repo `paradl` CLI cannot fetch the MDM archive because of the diagnosed watchdog bug).
- Parallel implementers authorized: S1 writes `rust/`, S2 writes `src/paradl/`, S3 writes `tests/`+`benchmarks/` — disjoint.

---

## Capability Mapping Matrix

| Aria2Engine capability | MDM mechanism | Migration action |
|---|---|---|
| ≤16 segmented HTTP(S) connections | `Planner` + worker pool | vendor planner; new Tokio worker pool |
| Up-front Range probe / 200 fallback | `probe()` + `Planner::make_single()` | new `probe.rs`; single-stream path |
| Watchdog v2 socket recycle | `Planner::release(w)` + `assign()` | `recycle_connections()` on `DownloadHandle` |
| Resume `.part` + sidecar | `state.rs` + `naming::part_path` | `.mdm.part` / `.mdm.json`; clean handles `.aria2` |
| Multi-mirror / failover | URI pool + rotate on worker error | mirror list passed into Rust |
| Custom headers/UA/Referer/Bearer/cookies | reqwest `RequestBuilder` | host-scoped cookie injection |
| `ip_family`, proxy, rate limit | reqwest + `Limiter` | options on `start_download`/`set_limit` |
| Size + SHA-256 verification | `verify.rs` (`Hash::parse`, `matches`) | preserve `VERIFICATION_FAILED` |
| Torrent/magnet/.meta4/.metalink | `mdm-torrent`, aria2 metalink | keep aria2 fallback (ruling) |
| Poll snapshots / JSONL events | `Event`/`Snapshot` → Python dict | callback into `LiveTransferDisplay`/`UI.emit_event` |
| Watchdog v2 integration | in-process `recycle_connections()` | wire `WatchdogController` action to handle method |

### Task 1: Rust `mdm-engine` port + PyO3 bindings (`rust/`)

**Files:** `rust/Cargo.toml`, `rust/mdm-engine/{Cargo.toml,src/*.rs}`, `rust/paradl-rs/{Cargo.toml,pyproject.toml,src/lib.rs}`, `rust/README.md`.
**Interfaces → Task 2:** `paradl._rust_engine` exposing `RustDownloadEngine`, `DownloadHandle`, `probe_url(url, headers) -> dict`, `start_download(...) -> DownloadHandle`, `resume_download(...)`, `Snapshot` fields `{downloaded, total, speed_bps, connections, state}`, and `DownloadHandle.{poll_snapshot, recycle_connections, pause, resume, cancel, wait}`.

- [ ] Step 1: TDD Rust tests for `Planner` (split/resume/release), `state` round-trip, `verify` SHA-256, and `recycle_connections` offset preservation. Watch RED.
- [ ] Step 2: Vendor + adapt the five engine modules with GPL headers intact.
- [ ] Step 3: Implement worker pool, probe, and PyO3 crate with `py.allow_threads`.
- [ ] Step 4: `cargo test` green; `maturin develop --release` builds into the venv.
- [ ] Step 5: Report + scoped review.

### Task 2: Python adapter, Watchdog v2, Doctor, CLI (`src/paradl/`)

**Files:** `src/paradl/engines/rust_engine.py`, `src/paradl/_rust_engine.py` (shim), `src/paradl/_rust_engine.pyi`, `src/paradl/engines/aria2.py` (delegation shim), `src/paradl/engines/watchdog.py`, `src/paradl/diagnostics/doctor.py`, `src/paradl/app/commands.py`, `src/paradl/cli/parser.py` (`--engine` flag), `src/paradl/core/config.py`, `src/paradl/util/toolchain.py`, `src/paradl/cli/main.py`.

- [ ] Step 1: TDD parity tests for `RustEngine` → `TransferResult` mapping (ExitCode, error, checksum) and watchdog recycle callback. Watch RED.
- [ ] Step 2: Implement adapter; default engine = rust for HTTP(S); `--engine aria2` forces legacy.
- [ ] Step 3: Doctor reports native engine version and no longer blocks on missing aria2c.
- [ ] Step 4: `pytest -q`, `ruff`, `mypy src` green.
- [ ] Step 5: Report + scoped review.

### Task 3: Parity tests + benchmarks (`tests/`, `benchmarks/`)

**Files:** `tests/test_rust_engine_parity.py`, `tests/test_rust_watchdog_recycle.py`, `benchmarks/bench_rust_engine.py`, `benchmarks/results/rust.json`.

- [ ] Step 1: RED tests using `tests/support/server.py` loopback throttling simulator: 16-channel throughput, mid-throttle recycle, crash resume, mirror failover, cookie redirect scoping, ExitCode parity.
- [ ] Step 2: Wire GREEN against the built extension.
- [ ] Step 3: Benchmark startup, throughput, recycle latency, peak RSS vs aria2 path; record honestly.
- [ ] Step 4: Report + scoped review.

### Task 4 (Lead): Integration, license, final gate

- [ ] Flag the GPL-3.0-or-later vs MIT conflict for maintainer decision (per-file SPDX headers retained).
- [ ] Final `cargo test`, `pytest -q`, `ruff`, `mypy src`, benchmark; plain-English summary.
