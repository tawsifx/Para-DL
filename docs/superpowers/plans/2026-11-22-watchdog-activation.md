# Watchdog Activation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive the existing v1/v2 anti-throttle watchdog from every engine download tick, recycle sockets on its actions, and emit `watchdog_action` events through the single emitter — directly on `main`.

**Architecture:** `paradl-engine` gains an internal `paradl-watchdog` dependency and an optional `WatchdogConfig` on `Target`. `NdEngine::download` creates a controller from that config (overriding `supports_range`/`connections` from authoritative plan/target data), observes a `ThroughputSample` on each 100 ms progress tick, forwards the action to the sink, and awaits `Download::recycle_connections()` when `action.is_recycle()`. `paradl-app` passes its existing `watchdog_config_for` result into `Target` and renders events through a `ProgressSink` implementation; the one-shot post-transfer `observe()` is removed.

**Tech Stack:** Rust 2024, Tokio, `paradl-watchdog` state machine, `paradl-events` single emitter, internal path dependency only.

**Spec:** docs/superpowers/specs/2026-11-22-watchdog-activation-design.md

## Global Constraints

- `paradl-events` is the only JSONL emitter; the engine never writes stdout.
- Default builds compile without optional system tools; no new **external** dependencies (internal crate edge allowed).
- Byte-exact watchdog event shapes (`E03`, `E60`, `L07`) and `tier2_pause_unpause` literal unchanged.
- `cargo doc --no-deps` clean; no missing docs on public items.
- `Target.watchdog = None` preserves today's behavior exactly.
- Implemented directly on `main`; every commit must leave the full suite green.
- Local rustfmt drift is an environment observation; do not mass-reformat sources.

## Review Focus

- **Recycle actually happens, not just an event:** the engine must await `Download::recycle_connections()` on `is_recycle()` actions; a sink-only implementation fails. Pin with an engine integration test asserting a recycle action was observed AND the download completed byte-exactly.
- **Non-Range transfers must not emit spurious actions:** config `supports_range` must be overridden from `plan.resumable`; a close-delimited local-server test must produce no recycle actions.
- **Event contract:** app emits exactly one `watchdog_action` JSONL line per non-None action, with the full E60 shape and no raw secrets; no line for `None`.
- **Backward compatibility:** `ProgressSink` closures (`impl<F: FnMut(&Snapshot)>`) must still compile via the defaulted `on_watchdog_action`; all existing engine tests use closures.
- **Progress bar/percentage from the preceding fixes must not regress** (`main` HEAD `2d79888`).

---

### Task 1: Engine drives the watchdog and recycles sockets

**Files:**
- Modify `rust/crates/paradl-engine/Cargo.toml`
- Modify `rust/crates/paradl-engine/src/engine.rs` (`Target`, `ProgressSink`, `NdEngine::download`)
- Test `rust/crates/paradl-engine/src/tests.rs` (reuse `spawn_slow_close_delimited_server` / range-server helpers)

**Interfaces:**
- Consumes: `paradl_watchdog::{WatchdogConfig, WatchdogController, WatchdogAction, WatchdogActionType, ThroughputSample}`; `paradl_core::SystemClock`; `Download::poll_snapshot()` and `Download::recycle_connections()`.
- Produces:
  - `Target::watchdog: Option<WatchdogConfig>` (default `None`).
  - `ProgressSink::on_watchdog_action(&mut self, _action: &WatchdogAction) {}` — defaulted no-op.
  - In `NdEngine::download`: cloned config with `supports_range = plan.resumable`, `connections = clamp_connections(t.connections)`; controller built with `SystemClock`; per tick `observe(ThroughputSample::new(started.elapsed().as_secs_f64(), snapshot.speed_bps as f64))`, `sink.on_watchdog_action(&action)`, and `if action.is_recycle() { let _ = download.recycle_connections().await; }`.

- [ ] **Step 1: write the failing engine test.** In `tests.rs`, add `watchdog_actions_drive_recycle_and_preserve_bytes`: local range server with per-chunk delay, `Target { watchdog: Some(WatchdogConfig { mode: WatchdogMode::V2, user_floor_bps: <high enough to look throttled>, min_cooldown_sec: 0.0, recycle_interval_sec: 0.2, supports_range: true, ..Default::default() }), .. }`, a sink struct capturing `WatchdogAction`s, then `NdEngine::new().download(plan, &mut sink)`; assert at least one action with `action.is_recycle()`, and the downloaded bytes equal the payload. Also add `close_delimited_watchdog_never_recycles` using `spawn_slow_close_delimited_server` asserting no recycle actions.
- [ ] **Step 2: verify RED** — `cd rust && cargo test -p paradl-engine watchdog_actions_drive_recycle` → compile errors (`Target` has no `watchdog`, `ProgressSink` has no `on_watchdog_action`).
- [ ] **Step 3: implement minimal code** exactly per Interfaces. Log recycle errors with `tracing` instead of propagating. Do not change watchdog internals, event shapes, or progress rendering.
- [ ] **Step 4: verify GREEN** — both new tests pass; `cargo test -p paradl-engine` green.
- [ ] **Step 5: run full suite** — `cargo test --all -- --skip full_non_media_gate_is_clean`, `cargo clippy --all -- -D warnings`, `cargo doc --no-deps`.
- [ ] **Step 6: commit** — `git add rust/crates/paradl-engine && git commit -m "feat(engine): drive watchdog from progress ticks and recycle sockets"`.

### Task 2: App supplies config and emits watchdog events

**Files:**
- Modify `rust/crates/paradl-app/src/app.rs`
- Test `rust/crates/paradl-app/src/app.rs` (`#[cfg(test)] mod`)

**Interfaces:**
- Consumes: `paradl_watchdog::{WatchdogAction, WatchdogActionType}`; `paradl_events::Event::WatchdogAction`; `watchdog_config_for`.
- Produces:
  - `AppProgressSink` implementing `ProgressSink`: `on_progress` keeps the existing bar/percentage logic; `on_watchdog_action` emits `emit_jsonl(&Event::WatchdogAction(action.to_event()))` in JSON mode and a one-line plain render otherwise, only when `action.action != WatchdogActionType::None`.
  - `run_engine` sets `engine_target.watchdog = Some(config.clone())`; `transfer` derives `hour_bucket` from `config.hour_bucket` and deletes the one-shot `let _action = watchdog.observe(...)` block and now-unused controller imports.

- [ ] **Step 1: write failing unit tests** — feed `AppProgressSink` a synthetic non-None `WatchdogAction` and assert stdout contains `"event": "watchdog_action"` and the `tier2_pause_unpause` literal; feed a `None` action and assert no output; assert a progress tick still renders a bar with a percentage.
- [ ] **Step 2: verify RED** — `cargo test -p paradl-app --lib watchdog_action` → missing type / method / field compile errors.
- [ ] **Step 3: implement minimal code** exactly per Interfaces; keep `emit_jsonl` as the only emitter; no new CLI flags or config fields.
- [ ] **Step 4: verify GREEN** — targeted tests pass, then `cargo test -p paradl-app`.
- [ ] **Step 5: run full suite** — `cargo test --all -- --skip full_non_media_gate_is_clean`, clippy, docs; verify `git diff` shows no change to progress-bar tests from the earlier fix.
- [ ] **Step 6: commit** — `git add rust/crates/paradl-app && git commit -m "feat(app): feed real watchdog config and emit watchdog events"`.

### Task 3: End-to-end verification on main

**Files:** none (verification only; docs updated only if a public doc is stale).

- [ ] **Step 1:** build `paradl` and run a JSON-mode download against the local slow range server with a short recycle interval; assert stdout contains at least one `watchdog_action` line and the transfer succeeds (`result: success`).
- [ ] **Step 2:** repeat in plain mode; assert the progress bar/percentage still renders on its own line and a short watchdog line appears.
- [ ] **Step 3:** confirm a no-Range/close-delimited URL emits no `watchdog_action` events.
- [ ] **Step 4:** `git status --short` shows only intended changes; all commits pushed to `main`.

## Final gate

- [ ] `cargo test --all -- --skip full_non_media_gate_is_clean` → 0 failures.
- [ ] `cargo clippy --all -- -D warnings` → 0 errors.
- [ ] `cargo doc --no-deps` clean.
- [ ] `git diff` confirms no watchdog state-machine changes and no new external dependencies.
