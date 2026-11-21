# Activate the anti-throttle watchdog on real downloads

**Date:** 2026-11-22
**Status:** Design proposed — awaiting lead approval
**Target:** `main` (lead chose direct-to-main delivery)

## Intent and why

The v1/v2 anti-throttle watchdog is a complete, tested state machine, but no
production code drives it. `app.rs:534` calls `observe()` exactly once, after
the transfer finishes, and discards the action (`let _action = ...`). No
production call site invokes `Download::recycle_connections()`. The user's
Android SDK download (`-c 16`, direct HTTPS zips) therefore gets 16 parallel
channels but no adaptive throttle recovery.

Goal: make the watchdog functional for every download driven by `paradl-engine`
— including the SDK task — without changing the watchdog state machine,
breaking byte-exact event contracts, or introducing external dependencies.

## Goals

1. Feed every engine progress tick into `WatchdogController::observe()`.
2. On any action where `is_recycle()` is true, await
   `Download::recycle_connections()` inside the engine's tick loop.
3. Emit a byte-exact `watchdog_action` JSONL event through the existing single
   emitter; render a short plain-text line in non-JSON mode.
4. Pass the app's existing `watchdog_config_for` result into the engine so the
   real user config (mode, floor, cooldown, learned profiles, host/network) is
   used — no duplicated policy logic.
5. Keep the watchdog off by default for library consumers (`None` config).

## Non-goals

- No changes to `paradl-watchdog`'s v1/v2 state machine, thresholds, or events.
- No media-route (Python/yt-dlp) watchdog: that path has no in-process sockets
  to recycle. aria2 legacy paths are out of scope.
- No new external dependencies; no CLI/flag changes.
- No change to progress-bar or SQLite work from the preceding bugfixes.

## Constraints (verbatim from project docs)

- `docs/superpowers/framework-charter.md:172`: "single-emitter rule:
  `paradl-events` is the only JSONL emitter."
- `docs/superpowers/framework-charter.md:38`: "Default feature set compiles
  WITHOUT optional system tools."
- `docs/superpowers/framework-charter.md:205`: `cargo doc --no-deps` clean; no
  missing docs on public items.
- `docs/superpowers/parity-matrix.md`: E03/E60/L07 watchdog action shapes and
  `tier2_pause_unpause` literal are byte-exact.
- No new dependencies (internal crate edges are allowed: watchdog → core only).

## Approved approach: engine-owned watchdog

Chosen over app-owned control signals and channel plumbing because the engine
already owns the live `Download` handle in its tick loop; one optional field on
`Target` gives every present and future consumer the feature.

### Changes

1. **`rust/crates/paradl-engine/Cargo.toml`** — add internal dependency
   `paradl-watchdog = { path = "../paradl-watchdog" }`. No cycle (watchdog
   depends only on core).
2. **`paradl-engine/src/engine.rs`**:
   - `Target` gains `pub watchdog: Option<WatchdogConfig>` (default `None` via
     existing `Default` impl / `..Target::default()`).
   - `ProgressSink` gains a defaulted method
     `fn on_watchdog_action(&mut self, _action: &WatchdogAction) {}`; the blanket
     `impl<F: FnMut(&Snapshot)>` keeps the default (no-op), so existing closures
     remain source-compatible.
   - `NdEngine::download`: when `t.watchdog` is `Some`, clone the config and
     override `supports_range` with `plan.resumable` and `connections` with
     `clamp_connections(t.connections)` — the engine owns these authoritative
     facts, so using them avoids spurious `tier2_pause_unpause` events on
     non-Range transfers. Then build
     `WatchdogController::with_clock(config, Arc::new(SystemClock))`. On each
     progress tick: `snapshot = download.poll_snapshot()`, call
     `sink.on_progress(&snapshot)`, then
     `action = watchdog.observe(ThroughputSample::new(started.elapsed().as_secs_f64(), snapshot.speed_bps as f64))`,
     call `sink.on_watchdog_action(&action)`, and if `action.is_recycle()` await
     `download.recycle_connections()`. Errors from recycle are logged via
     `tracing` and never fail the transfer.
   - `per_conn_speeds` is `None` (engine `Snapshot` has no per-flow telemetry);
     `resilience_event` is `None` (no external event source is wired yet).
3. **`paradl-app/src/app.rs`**:
   - Build `watchdog_config_for(...)` before `run_engine`, set
     `engine_target.watchdog = Some(config.clone())`, and derive the transfer
     hour bucket from `config.hour_bucket` instead of constructing a second
     controller. Remove the one-shot `let _action = watchdog.observe(...)`.
   - Replace the plain `ProgressSink` closure with a small `AppProgressSink`
     struct implementing `ProgressSink`; `on_progress` keeps the existing bar/
     percentage rendering, and `on_watchdog_action` emits
     `Event::WatchdogAction(action.to_event())` via `emit_jsonl` in JSON mode,
     or a one-line plain render when not quiet. Emit only when
     `action.action != WatchdogActionType::None` to avoid one event per tick.
4. **`paradl-engine` unit/integration tests** — drive a fake/slow local server,
   pass a `WatchdogConfig` with a tiny `min_cooldown_sec`/`recycle_interval_sec`,
   and assert at least one recycle cycle occurs (via the action callback) and
   the download still completes byte-exactly.
5. **`paradl-app` tests** — assert the sink forwards a non-None watchdog action
   to stdout as a valid `watchdog_action` JSONL line, and that no line is
   emitted for `None` actions.

## Behavior when disabled

`Target.watchdog = None` (the default) preserves today's behavior exactly: no
`observe()` calls, no recycles, no watchdog events. Media routes remain
unchanged.

## Verification

- `cargo test --all -- --skip full_non_media_gate_is_clean` → 0 failures.
- `cargo clippy --all -- -D warnings` → 0 errors.
- `cargo doc --no-deps` clean.
- Real binary run with `--json` against a local slow range server →
  `watchdog_action` lines appear. With shipped intervals (2 s / 5 s) the
  transfer completes; with an artificially aggressive interval (50 ms) on a
  genuinely 4 KB/s server, repeated recycles can starve committed progress and
  trip the pre-existing 30 s stall guard — see the limitation below.
- SDK-style run (`-c 16`, direct URL) still shows the progress bar/percentage.

## Builder-check ruling

The builder asked whether to use the app config verbatim or override engine-owned
facts. Ruling (controller's call, recorded for review): override `supports_range`
from `plan.resumable` and `connections` from the clamped target count before
constructing the controller. Reason: the app hardcodes `supports_range = true`,
so verbatim use would emit `tier2_pause_unpause` events on no-Range transfers
where `recycle_connections()` is a no-op. Cost if wrong: one small clone-and-set
block in `NdEngine::download`; it does not change the watchdog state machine.

## Risks / mitigations

- **Recycle storms:** v2 cooldown and periodic interval already bound this;
  recycle errors are logged, not propagated.
- **Event spam:** only non-None actions emitted.
- **TDD difficulty in engine:** use a controllable sink callback plus a local
  server; no wall-clock sleeps in assertions.
- **Known limitation (accepted):** `recycle_connections()` aborts all workers and
  drops uncommitted claims. If recycle cadence is shorter than the time to commit
  a chunk, committed progress freezes and the pre-existing 30 s stall guard fails
  the transfer. Shipped defaults (2 s cooldown / 5 s periodic) make this unlikely;
  the engine test uses a fast-enough dribble so recycles land mid-transfer. A
  future change could preserve in-flight commits or recycle a rotating subset.
