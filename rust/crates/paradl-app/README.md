# paradl-app

Application-level orchestration wiring every `paradl-*` framework crate behind
the byte-exact `paradl-cli` parser for the `paradl` reference binary.

## Public surface

- `App` — implements `paradl_cli::Dispatch` and routes **every** `Command`
  variant to a thin adapter over the owning crate. `App::new_offline()` disables
  network/process side effects and is used by the dispatch tests.
- `Capture` / `OutputSink` — injectable output sink so no subcommand writes to a
  real terminal in tests.
- `DoctorReport` / `ToolProbe` — `doctor` diagnostics: binary version,
  `paradl-media/1`, Python media-package version, yt-dlp version and
  JS-runtime status, gathered with **no Python** on non-media paths.
- `handoffs::{resolve_isp, watchdog_config_for, normalise_resolve_failure,
  api_resolve_failure_status, map_media_error_kind}` — the four ledgered
  cross-crate handoffs wired at integration time (W4-1).

## Crate map

| Crate | Used by | Adapter |
|---|---|---|
| `paradl-core` | config, paths, `Error`/`ExitCode` | config / download / netprobe |
| `paradl-cli` | parser, help/version bytes, dispatch | `App` |
| `paradl-net` | routing, probes, ISP | download / info / netprobe / `resolve_isp` |
| `paradl-engine` | planner, transfer, clamp | download |
| `paradl-store` | history v4, `queue_jobs` | history / queue |
| `paradl-watchdog` | v1/v2 controller, hour bucket | `watchdog_config_for` |
| `paradl-throttle` | classifier, `report.json` | throttle |
| `paradl-postproc` | checksums, archive security | hash / extract |
| `paradl-platform` | keychain / metadata | auth |
| `paradl-events` | single JSONL emitter | every `--json` event |
| `paradl-resilience` | auth lookup, queue, plist | auth / queue |
| `paradl-ui` | plain event summaries | human output (`tui` feature) |
| `paradl-media` | one-shot worker client | media route (`media` feature) |
| `paradl-api` | axum server | server (`api` feature) |

## Feature flags

All opt-in; the default build compiles with no optional system tools:

- `api` — enables `paradl-api` and the `server` command body.
- `tui` — enables the ratatui renderer in `paradl-ui`.
- `media` — enables the Python worker client for media routes. Without it,
  media targets fail closed with install guidance and **no Python is spawned**.
- `platform-macos` / `platform-windows` — feature-gated native metadata and
  keychain implementations in `paradl-platform`.

## Example

```bash
cargo run -p paradl-app --example wire_all
```

## No Python on non-media paths

`doctor` uses compile-time constants and short probes of native tool binaries
(`yt-dlp --version`, `node --version`, …). The Python worker is only discovered
and spawned for a media route when the `media` feature is compiled in.
