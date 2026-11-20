# paradl-resilience

Platform services around the Rust download engine: priority queue scheduling on
the shared `queue_jobs` table, deterministic launchd plist generation, isolated
result hooks, the WAKE / NET_CHANGE / STALL event source, and credential lookup.

## Single-emitter rule

This crate **never** writes stdout and never calls `json.dumps` / `serde_json`
to build an event line. Each operation returns an [`Emitted`] value wrapping a
typed `paradl_events` event; `Emitted::to_jsonl()` delegates to
`paradl_events::emit_jsonl`. This is exactly the fix required for the ledgered
single-emitter violation at `src/paradl/throttle/cli.py:296-308` (parity row E58).
Hook stdin JSON also goes through `paradl_events::to_json_line` (with redaction).

## Public modules

| Module | Role |
|---|---|
| `queue::QueueScheduler` | `add_job`, `list_jobs`, `start_job`, `defer`, `start_due_jobs`, `when_fast_gate`, `recover_crashed`, `pause`, `resume`, `remove` — all emit typed events. |
| `plist::{PlistConfig, PlistStart, generate_launchd_plist, parse_start_at_time, throttle_plist_event}` | Sorted-key XML `LaunchAgent` generation (never calls `launchctl`) and the E58 typed event. |
| `hooks::{HookRunner, HookType}` | 30 s timeout, JSON on stdin, SIGKILL on hang, `hook_executed`/`failed`/`timeout`/`error` events. |
| `events::{ResilienceEventSource, ResilienceConfig, ResilienceClock, ManualClock, SystemResilienceClock}` | Debounced WAKE / NET_CHANGE / STALL detection with injected clocks. |
| `auth::{Service, TokenSource, lookup_token, KeychainSecretStore, keychain_store_spec, ...}` | Strict env → keychain → none precedence; tokens never in argv, registered for redaction. |

## Feature flags

None. Depends only on finished framework crates (`paradl-core`, `paradl-events`,
`paradl-store`, `paradl-watchdog`) plus `serde`, `serde_json`, `chrono`. All
errors are `paradl_core::Error` with the full `ExitCode` map.

## Example

```bash
cargo run -p paradl-resilience --example resilience_demo
```

See [`examples/resilience_demo.rs`](examples/resilience_demo.rs).
