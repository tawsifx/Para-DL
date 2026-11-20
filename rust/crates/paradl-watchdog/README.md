# paradl-watchdog

Anti-throttle watchdog state machine for paraDL: a byte-faithful Rust port of
`src/paradl/engines/watchdog.py` (v1 and v2), including rolling p90/p95,
recycle cooldown, learned per-host/hour/network profiles, and a clock-injected
supervision loop.

## Public interface

- `Watchdog` — the charter §2 trait: `observe(ThroughputSample) -> WatchdogAction` and `state() -> WatchdogState`.
- `WatchdogController` — concrete v1/v2 implementation with `new`, `with_clock`, `with_store`, and `supervise`.
- `WatchdogAction` / `WatchdogActionEvent` — action result and the serializable `watchdog_action` event field set (E60) that `paradl-events` (W2-4) consumes.
- `ProfileStore` / `InMemoryProfileStore` / `LearnedProfile` — injectable persistence port for learned profiles.
- `ThroughputSample`, `ResilienceEvent(Type)`, `WatchdogState`, `WatchdogActionType`, `WatchdogMode`, `compute_percentile`, `compute_hour_bucket`, `py_round`.

There are **no feature flags**; the default build has no optional system dependencies.

## Parity guarantees

- `WatchdogActionType.tier2_pause_unpause` (L07) is emitted verbatim.
- `WatchdogActionEvent::to_jsonl_line()` reproduces the oracle JSONL byte-for-byte, including Python `json.dumps` spacing, `round(time.time(), 3)` timestamps (banker's rounding), field order, and a trailing newline (L04).
- v2 threshold `max(user_floor, 0.5 * rolling_p90)` with at least three positive samples.
- 5-sample / 5-second recycling hysteresis, 2-second minimum cooldown, relative straggler detection, stale-profile (30-day) re-validation, and WAKE/NET_CHANGE/STALL handling.

## Example

```bash
cargo run -p paradl-watchdog --example basic
```

See [`examples/basic.rs`](examples/basic.rs) for a full scenario that drives the
controller with an injected `FakeClock` and prints the byte-exact event line.
