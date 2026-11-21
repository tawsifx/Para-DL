# paradl-throttle

Pure computational core of the paraDL throttling diagnostics suite — a
byte-exact Rust port of the Python classifier, parsers, budget, probe
statistics, target helpers, and report generators, plus an optional Tokio VPS
ground-truth server.

## Public API

- `classifier`: `mann_whitney_u_test`, `compute_next_best_hours`, `classify_dataset`
  (M0–M10 verdicts, next-best-hours).
- `parsers`: `parse_ping`, `parse_traceroute`, `parse_netstat_tcp`,
  `parse_nettop`, `parse_networksetup`, `parse_scutil_env`.
- `budget`: `parse_byte_budget`, `clamp_connections`, `DataBudgetTracker`
  (Python-compatible seeded jitter via MT19937).
- `probes`: `analyze_burst_series`, `flow_statistics`, `flow_scale_ratios`,
  `throughput_cov`.
- `targets`: URL parsing, locality classification, loopback ASN simulation,
  DNS TXT query/response helpers.
- `report`: `generate_report_json` (byte-exact schema `1.0`),
  `generate_markdown_report`, `generate_html_report`, `write_report_files`.
- `events`: typed `{event, timestamp, ...redacted}` payloads for the single
  `paradl-events` JSONL emitter (never printed here).

Error handling uses `paradl_core::Error` / `ExitCode` from `paradl-core`.

## Feature flags

| Flag | Default | Behavior |
|---|---|---|
| `server` | off | Compiles the Tokio TCP/UDP VPS ground-truth server (`run_vps_server`). |

The default build has no optional runtime dependencies and never invokes Python.

## Byte-exact parity

- 12 seeded `test_classifier.py` cases + empty/edge/dataset cases match
  `rust/crates/paradl-oracle/fixtures/classifier_seeded.json` exactly.
- `report.json` matches `report_schema.json` exactly (schema `1.0`,
  `modules.m0_sanity` … `m10_local_vs_intl`).

## Example

```bash
cargo run -p paradl-throttle --example classify
cargo run -p paradl-throttle --features server --example classify  # server feature on
```

See [`examples/classify.rs`](examples/classify.rs).
