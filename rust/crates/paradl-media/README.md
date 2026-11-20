# paradl-media

Rust-side **protocol client/router** for the Python media worker. Media stays
Python as a one-shot `python -m paradl_media` process over `paradl-media/1`;
this crate contains no extractor logic, no PyO3, and no long-lived sidecar.

## Public surface

- `MediaResolver` — the authoritative trait: `domains()`, `resolve()`,
  `download_managed()`, `claims()`. `WorkerMediaResolver` is the concrete
  one-shot-process implementation.
- `WorkerLaunch` / `discover_python` — interpreter discovery with the mandated
  precedence `PARADL_MEDIA_PY` > path relative to `current_exe()` > data dirs,
  plus an explicit-launch seam for embeddings and tests.
- `check_protocol`, `WorkerFrame`, `WorkerVersions`, `ProtocolRequest`,
  `ProtocolOp` — wire types and the fail-closed handshake. Minor additions
  (`1.1`) are tolerated; unknown majors and missing/unparseable versions fail
  with exit code `4`.
- `ErrorKind` / `error_kind_to_exit_code` / `error_kind_to_error` — Rust owns
  the full mapping (`metadata|download|conversion`→`1`, `dependency`→`4`,
  `usage`→`2`, `interrupted`→`130`, never `3`).
- `ProgressSink` / `reemit_progress` — worker progress becomes a redacted
  `paradl-events` line; this crate never writes to stdout. Worker human logs
  are inherited on stderr.
- `MediaRoute` / `classify_route` — pure routing rules from `paradl-core`
  constants. The worker owns `claims` and the direct-vs-managed decision;
  `claims=false` returns a direct-fallback result.

## Feature flags

`media` is the workspace opt-in marker consumed by the `paradl` binary. The
client API itself is always compiled and has no optional system tools; a
missing worker Python is reported at operation time with install guidance.

## Example

```bash
cargo run -p paradl-media --example protocol_client
```

The example uses a `sh -c` stub worker, so it needs no Python or yt-dlp.
Contract tests live in [`tests/protocol.rs`](tests/protocol.rs) (golden-driven
handshake/error matrix) and [`tests/worker.rs`](tests/worker.rs) (subprocess
stub, progress re-emission, timeouts, fail-closed parsing).
