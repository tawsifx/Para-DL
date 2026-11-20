# paradl-oracle

Dev-only, documentation-grade golden fixture and contract-test crate for the final Rust
framework migration. It is **not published** and contains no application logic: it stores
byte-exact outputs captured from the Python oracle (`tests/unit/*.py`, `rust/rapids/src/tests.rs`)
so later crates can prove byte-exact parity against the original implementation.

## Standing ruling

The Python application becomes Rust **except the media subsystem**, which stays Python and is
invoked as `python -m paradl_media` over the versioned JSON-lines protocol `paradl-media/1`.
Prior "no Python anywhere" criteria are void. See `docs/superpowers/framework-charter.md`.

## Public API

- `fixtures::fixture(name)` — read an embedded golden fixture as `serde_json::Value`.
- `fixtures::fixture_str(name)` — read an embedded golden fixture as raw JSON text.
- `fixtures::FIXTURE_NAMES` — the canonical list of fixture files.

## Golden fixtures

| Fixture | Oracle source |
|---|---|
| `core_models.json` | `tests/unit/test_models.py`, `src/paradl/core/models.py` |
| `config_defaults.json` | `tests/unit/test_config.py`, `src/paradl/core/config.py` |
| `classifier_seeded.json` | `tests/unit/test_classifier.py` (12 seeded cases), `report.json` §1.0 |
| `watchdog.json` | `tests/unit/test_watchdog.py`, `src/paradl/engines/watchdog.py` |
| `jsonl_events.json` | `src/paradl/ui/display.py` (43 event names), `PARALLEL_DL_DEV_CONTEXT.md` §4 |
| `redaction.json` | `src/paradl/ui/display.py::redact_secrets/_redact_obj` |
| `sidecar.json` | `rust/rapids/src/state.rs`, `rust/rapids/src/tests.rs` |
| `report_schema.json` | `src/paradl/throttle/models.py::ThrottleReport.to_dict` (schema 1.0, m0..m10) |
| `media_protocol.json` | charter §4, `src/paradl/engines/media.py:48–72` |

## Contract tests

`cargo test -p paradl-oracle` runs fixture self-tests (exact captured literals) and
`paradl-media/1` version-conformance tests. Tests that require a crate not yet implemented
(`paradl-core`, `paradl-watchdog`, `paradl-throttle`, `paradl-events`, `paradl-media`) are
marked `#[ignore = "..."]` with their exact expected goldens present and runnable once the
owning crate supplies an implementation.

## Feature flags

None. This is a dev-only crate.

## Example

```sh
cargo run -p paradl-oracle --example dump_fixtures
```
