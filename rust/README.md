# Rust `mdm-engine` + `paradl-rs`

In-process replacement for paradl's external `aria2c` daemon + JSON-RPC control plane.
This workspace is **Task 1** of `docs/superpowers/plans/2026-11-17-rust-mdm-engine.md`.

## Layout

| Path | Role |
|---|---|
| `mdm-engine/` | Trimmed, vendored MDM engine: pure planner, resume state, token-bucket limiter, checksum verifier, Range probe, Tokio worker pool |
| `paradl-rs/` | PyO3 crate built by maturin as `paradl._rust_engine` |

Only crates.io dependencies are used. No git-patched deps, no `wreq`/BoringSSL, no
`rquickjs`/PAC, no FTP/torrent/site modules.

## Build / test

```sh
cd rust
cargo test --all                       # Rust unit + fault-injection tests
cd paradl-rs
maturin develop --release              # into /Users/tawsifx/paradl/.venv
# fallback if maturin is unavailable:
cargo build --release
```

MSRV: Rust 1.98 (edition 2024). Verified with Rust 1.100.

## Python API (`paradl._rust_engine`)

- `RustDownloadEngine(max_connections=None, rate_limit_bps=None, ...)`
- `probe_url(url, headers=None) -> dict` with `url`, `final_url`, `size`, `file_name`,
  `resumable`, `content_type`.
- `start_download(url, out_dir, filename=None, connections=None, headers=None, cookies=None,
  referer=None, user_agent=None, proxy=None, ip_family=None, mirrors=None, checksum=None,
  rate_limit_bps=None) -> DownloadHandle`
- `resume_download(...)` — same arguments; resumes from `<dest>.mdm.part` + `<dest>.mdm.json`.
- `DownloadHandle.poll_snapshot() -> dict` — exactly
  `{downloaded, total, speed_bps, connections, state}` with states
  `queued|probing|downloading|paused|verifying|done|failed|cancelled`.
- `DownloadHandle.recycle_connections()`, `pause()`, `resume()`, `cancel()`.
- `DownloadHandle.wait() -> dict` with `success`, `error`, `path`, `downloaded`, `total`.
- Exceptions: `DownloadError`, `VerificationError` (for checksum/size mismatch).

All blocking calls wrap `py.allow_threads`; the GIL is never held during a transfer.

## Why recycle is safe here (the aria2 bug)

See `docs/diagnostics/watchdog-cycle-aria2-bug.md`. aria2's `force_pause`/`unpause` on a
download whose only URI came from a 302 lost the redirected URI pool, yielding
`errorCode=8 "No URI available"`. In this engine:

1. `probe()` records the **post-redirect** URL (`response.url()`) in the URI pool once.
2. `DownloadHandle.recycle_connections()` aborts worker tasks, calls `planner.release(w)`
   (dropping only *uncommitted* claims), then `planner.assign(w, None)` for fresh TCP
   connections **over the same immutable URI pool**.
3. Committed byte offsets and the part file are untouched; the next ranges start at those
   offsets. No URI is ever re-derived from a daemon-held handle.

`mdm-engine/src/tests.rs::fault_injected_recycle_preserves_offsets_and_uri_pool` proves
this against a loopback Range server that drops connections mid-body.

## Engine module layout

The `mdm-engine` crate contains a segmentation planner, crash-safe resume state,
token-bucket limiter, checksum verifier, Range probe, and a bounded Tokio worker
pool. `paradl-rs` exposes them to Python as `paradl._rust_engine`.

| File | Role |
|---|---|
| `mdm-engine/src/planner.rs` | dynamic segmentation planner (pure logic) |
| `mdm-engine/src/state.rs` | resume state + atomic sidecar writes |
| `mdm-engine/src/limiter.rs` | per-download token-bucket speed limiter |
| `mdm-engine/src/verify.rs` | whole-file checksum parse/verify |
| `mdm-engine/src/naming.rs` | `.pdl.part` / `.pdl.json` path helpers |
| `mdm-engine/src/probe.rs` | GET-only probe capturing the final post-redirect URL |
| `mdm-engine/src/download.rs` | Tokio worker pool + in-process socket recycle |
