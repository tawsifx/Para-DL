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

## Vendored file list and licensing

Ported from Mister Download Manager (c) Ahmed Nefzaoui, GPL-3.0-or-later:

| File | Upstream source | Modification |
|---|---|---|
| `mdm-engine/src/planner.rs` | `crates/engine/src/planner.rs` | near-verbatim; tests moved out |
| `mdm-engine/src/state.rs` | `crates/engine/src/state.rs` | `craft_atomic::atomic_write` → std write-temp-rename; legacy migration removed |
| `mdm-engine/src/limiter.rs` | `crates/engine/src/limiter.rs` | near-verbatim; tests moved out |
| `mdm-engine/src/verify.rs` | `crates/engine/src/verify.rs` | near-verbatim; tests moved out |
| `mdm-engine/src/naming.rs` | `crates/engine/src/naming.rs` | only `.mdm.part` helper retained |
| `mdm-engine/src/probe.rs` | `crates/engine/src/download.rs` (`probe`) | focused GET-only probe capturing final URL |
| `mdm-engine/src/download.rs` | new (informed by MDM planner/workers) | small Tokio worker pool + in-process recycle |

Every ported/adapted file carries an `SPDX-License-Identifier: GPL-3.0-or-later` header
crediting Ahmed Nefzaoui plus a modification note. New files (`error.rs`, `tests.rs`,
`paradl-rs/src/lib.rs`) carry the same notice.

> **LICENSE FLAG (for Task 4 / user review):** the vendored engine is
> **GPL-3.0-or-later**; the root `pyproject.toml` declares **MIT**. The root license was
> **not** changed. Distributing the combined product requires resolving this conflict
> (see `THIRD_PARTY_NOTICES.md`, owned by the lead).
