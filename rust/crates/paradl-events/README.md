# paradl-events

**The only JSONL emitter in paraDL.** Every observable event is serialized here
as `{ "event": <name>, "timestamp": round(time.time(), 3), ...redacted }` using
Python `json.dumps` defaults (`", "` / `": "` separators, insertion-ordered keys,
`ensure_ascii=True`) so `--json` output is byte-exact against the Python oracle.

## Contract

- **Single emitter:** no other crate formats or writes JSONL. `emit_jsonl`
  returns a newline-terminated `String`; the reference binary / API owns stdout
  and writes that string verbatim. A chatty media worker can never corrupt
  `--json` because its progress frames are re-emitted through this crate.
- **Version:** `EVENT_SCHEMA_VERSION = "paradl-events/1"` is a public capability
  constant. It is deliberately *not* a per-line field, preserving the oracle
  envelope exactly. The media protocol version `paradl-media/1` is independent.
- **Redaction:** applied at serialization time (never opt-in). Bearer tokens,
  `token=`/`token:` values, cookies, `Authorization` / `Proxy-Authorization`,
  passwords, environment credentials and registered secrets are replaced with
  `***REDACTED***`, including inside nested objects and arrays.

## Public API

| Item | Role |
|---|---|
| [`Event`] | Typed enum covering all 44 oracle names + `resilience_event`; `all()` returns one sample of each. |
| `emit_jsonl<T: Serialize>(&T) -> String` | The single serializer (with trailing newline). |
| `to_json_line<T: Serialize>(&T) -> String` | Same serializer without newline (used for hook stdin JSON). |
| `emit_jsonl_with_secrets` / `to_json_line_with_secrets` | Explicit secret sets. |
| `register_secret(&str)` | Register a runtime secret (mirrors `UI.register_secret`). |
| `Redactor` / `REDACTED` | Redaction engine and replacement literal. |
| `AdoptedEvent<T>` / `AdoptableEvent` | Adapter for typed events from crates this crate must not depend on (e.g. `paradl-throttle`, W2-1). |
| `from_watchdog` / `from_watchdog_event` | Consume `paradl_watchdog::WatchdogActionEvent` directly — no second emitter. |
| `now_timestamp()` / `py_round()` | Python banker's-rounded `round(time.time(), 3)`. |
| `names` | Canonical event-name literals (E01–E61). |
| `schema::*` | One documented, serde-serializable struct per event shape. |

## Feature flags

None. The crate is always available and depends only on `paradl-core`-adjacent
crates (`paradl-watchdog`), `serde`, `serde_json` and `regex`. It has no PyO3
dependency and no optional system tools.

## Golden tests

`tests/golden.rs` reads the read-only `paradl-oracle/fixtures/jsonl_events.json`
and asserts the registry, every field set, the byte-exact `progress` envelope,
and byte-exact consumption of the watchdog event. `tests/redaction.rs` reads
`redaction.json` and asserts every redaction golden.

## Example

```bash
cargo run -p paradl-events --example emit_events
```

See [`examples/emit_events.rs`](examples/emit_events.rs).
