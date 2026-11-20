# paraDL Rust Framework Charter

> Lead-approved architecture charter for the final Rust framework with a Python media worker.
> Source of truth: the maintainer's FINAL master prompt (`docs/master-prompts/FINAL-rust-framework-with-python-media.md`).
> Status: APPROVED by lead (this document is the lead ruling record for Wave 0 scaffolding).

**Goal:** a Cargo workspace of public, semver-versioned, documented crates that replace the entire Python application **except the media subsystem**, which remains a standalone Python package invoked by the native binary over a versioned protocol.

**Standing scope ruling:** ~25,887 Python LOC become Rust (91%); ~1,286 stay Python (media worker + JS/EJS/version helpers + media model helpers + extractor-claim URL helper); ~1,200 dead aria2 daemon LOC deleted. The prior "no Python anywhere" criteria are VOID.

---

## 1. Workspace crates

Every crate: `version = "0.1.0"` during development, crate-level `//!` docs, `README.md`, at least one runnable `examples/` target, `#![warn(missing_docs)]` on public items, and a `tests/` or `#[cfg(test)]` suite. Workspace root: `rust/Cargo.toml` (edition 2024, rust-version 1.98 — existing values kept).

| Crate | Public role | Extension traits | Feature flags | Oracle |
|---|---|---|---|---|
| `paradl-core` | Models, helpers, config, paths, clock abstraction, `Error`→ExitCode | — | — | `test_models.py`, `test_config.py` |
| `paradl-oracle` | Golden fixtures / contract tests (dev-only, not published) | — | — | all Python oracle tests |
| `paradl-store` | SQLite persistence, v4 migrations, WAL, device+inode guard | `Store` | — | `test_sqlite_guard.py`, `test_classifier.py` store cases |
| `paradl-net` | reqwest client, probe, RFC 6266 filenames, ETag, DNS/DoH, routing, site resolvers | `TargetResolver` | `doh` | `test_net.py`, `test_engine_batch.py`, `test_bug_fixes.py`, `test_github_acceleration.py`, `test_pypi_transitive.py` |
| `paradl-engine` | promote `rapids`: planner, state, limiter, verify, probe, download | `DownloadEngine` | `doh` | `rapids/src/tests.rs`, `test_rust_engine_parity.py`, `test_watchdog_uri_restore.py` |
| `paradl-watchdog` | v1/v2 state machine, percentile, cooldown, learned profiles, supervision | `Watchdog` | — | `test_watchdog.py` |
| `paradl-throttle` | classifier, parsers, budget, probes, targets, VPS tokio server, reports | — | `server` | `test_classifier.py` (12 seeded cases), `report.json` v1.0 |
| `paradl-postproc` | checksum (blake2b, GNU/BSD text, companion), ZIP/tar security, ADB | `PostProcessor` | — | checksum/archive tests |
| `paradl-platform` | macOS/Windows byte-exact metadata + keychain | `Platform` | `platform-macos`, `platform-windows` | `macos_fake.py` goldens, `test_platform_windows.py` |
| `paradl-media` | Rust-side protocol client/router only — NO extractor logic | `MediaResolver` | `media` | media protocol contract tests |
| `paradl-events` | **only** JSONL emitter: ~48 event shapes + redaction + version constant | — | — | golden shapes over all events |
| `paradl-ui` | progress display + ratatui TUI consuming `paradl-events` | — | `tui` | golden output tests |
| `paradl-api` | axum server, 7 endpoints, serde validation, task store/runner | — | `api` | `test_api_optional_pydantic.py` |
| `paradl-cli` | clap parser: all ~60 flags, ~20 subcommands, env vars, dispatch trait | dispatch trait | — | `test_bug_fixes.py`, `test_perf_parity.py` |
| `paradl-app` | orchestration wiring all crates for the reference binary | — | — | app-level integration tests |
| `paradl` | **binary** (thin reference consumer) owning `paradl`/`parallel-dl` | — | `api`, `tui`, `media`, `platform-macos`, `platform-windows` | full CLI + differential lanes |
| `paradl-parity` | 3-lane differential harness (dev-only) | — | — | non-media / media / boundary lanes |

Default feature set compiles WITHOUT optional system tools. `media`, `api`, `tui`, `platform-macos`, `platform-windows` are opt-in.

---

## 2. Public trait signatures (authoritative interfaces)

```rust
// paradl-core
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode { Ok=0, Failure=1, Usage=2, VerificationFailed=3, MissingDependency=4, Interrupted=130 }

pub trait Clock { fn now(&self) -> std::time::SystemTime; }

// paradl-engine
#[async_trait]
pub trait DownloadEngine: Send + Sync {
    async fn plan(&self, target: &Target) -> Result<Plan>;
    async fn download(&self, plan: Plan, sink: &mut dyn ProgressSink) -> Result<TransferResult>;
    fn capabilities(&self) -> EngineCapabilities;
}

// paradl-net
#[async_trait]
pub trait TargetResolver: Send + Sync {
    fn scheme(&self) -> &'static str;
    async fn resolve(&self, target: &str, ctx: &ResolveContext) -> Result<Vec<ResolvedAsset>>;
}

// paradl-store
pub trait Store: Send + Sync {
    fn record_transfer(&self, r: &TransferRecord) -> Result<()>;
    fn recent(&self, limit: usize) -> Result<Vec<TransferRecord>>;
    fn open_queue(&self) -> Result<Queue>;
}

// paradl-watchdog
pub trait Watchdog: Send + Sync {
    fn observe(&mut self, sample: ThroughputSample) -> WatchdogAction;
    fn state(&self) -> WatchdogState;
}

// paradl-postproc
pub trait PostProcessor: Send + Sync {
    fn verify(&self, path: &Path, expected: Option<&Digest>) -> Result<Verification>;
    fn postprocess(&self, ctx: &PostContext) -> Result<PostResult>;
}

// paradl-platform
pub trait Platform: Send + Sync {
    fn set_origin_metadata(&self, path: &Path, urls: &[String]) -> Result<()>;
    fn store_secret(&self, service: &str, account: &str, secret: &str) -> Result<()>;
    fn load_secret(&self, service: &str, account: &str) -> Result<Option<String>>;
}

// paradl-media (protocol client only — no extractor implementation)
#[async_trait]
pub trait MediaResolver: Send + Sync {
    fn domains(&self) -> &'static [&'static str];
    async fn resolve(&self, req: MediaRequest) -> Result<MediaResult>;
    async fn download_managed(&self, req: MediaRequest, sink: &mut dyn ProgressSink) -> Result<MediaResult>;
    async fn claims(&self, url: &str) -> Result<bool>;
}
```

---

## 3. Error model / ExitCode contract

`paradl_core::Error` is the single public error type. Variants map to `ExitCode`:

| Error variant | ExitCode |
|---|---|
| `Ok` / terminal success | `0` |
| `Failure` (transfer/runtime/guard) | `1` |
| `Usage` (bad CLI/config/rate) | `2` |
| `VerificationFailed` (checksum/size/archive security) | `3` |
| `MissingDependency` (yt-dlp / JS runtime) | `4` |
| `Interrupted` (SIGINT/SIGTERM) | `130` |

Rust owns the ENTIRE mapping. The Python worker emits structured `error.kind` only; it never emits exit code 3 (checksums are Rust-owned after `finalize`).

---

## 4. Media protocol schema `paradl-media/1`

**Transport:** one-shot worker. Rust spawns `<python> -m paradl_media` per media operation. One request line on stdin; `progress` frames + one terminal `result`/`error` line on stdout; human logs to stderr only. `stdout` is protocol-only. Rejected: long-lived sidecar, PyO3 embedding.

**Request (stdin, one JSON line):**
```json
{
  "protocol": "paradl-media/1",
  "request_id": "<uuid-v4>",
  "op": "resolve" | "download_managed" | "finalize" | "claims",
  "target": "<url-or-search>",
  "out_dir": "<absolute path>",
  "options": { "audio_only": false, "format": null, "cookies_from_browser": null }
}
```

**Progress frame (stdout):**
```json
{"protocol":"paradl-media/1","request_id":"<id>","frame":"progress","downloaded_bytes":0,"total_bytes":null,"speed_bps":0,"eta_seconds":null}
```

**Terminal result (stdout, exactly one):**
```json
{"protocol":"paradl-media/1","request_id":"<id>","frame":"result","ok":true,
 "versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"7.0"},
 "caps":["ejs","mutagen","ffmpeg"],
 "data":{"filename":"...","http_headers":{"Referer":"..."},"audio_format_produced":"passthrough_aac","paths":[]}}
```

**Terminal error (stdout, exactly one):**
```json
{"protocol":"paradl-media/1","request_id":"<id>","frame":"error","ok":false,
 "error":{"kind":"metadata|download|conversion|dependency|usage|interrupted","message":"..."},
 "versions":{...}}
```

**Handshake rules:**
- Both request and every response frame carry `protocol` and `request_id`.
- Major version must match (`paradl-media/1` ↔ `paradl-media/1`). Unknown major → clear diagnostic + exit `4`.
- Minor additions tolerated (`paradl-media/1.1` interoperates with `1.0`). Missing/unparseable version → fail closed → exit `4`.
- `versions` must be present and parseable in terminal frames; missing/unparseable → fail closed → exit `4`.
- yt-dlp version is DATA, not protocol; only schema changes bump the protocol version.
- Contract tests cover N, N−1, N+1, unknown, missing.

**`error.kind` → Rust ExitCode:** `metadata|download|conversion` → `1`; `dependency` → `4`; `usage` → `2`; `interrupted` → `130`. Worker never emits `3`.

**Byte-exact worker contracts (asserted verbatim in every protocol test):**
- `http_headers`: values coerced to `str`, keys unchanged.
- `filename`: from `sanitize_filename` — 200-char cap, Windows reserved names.
- `audio_format_produced`: exact strings from `classify_audio_production` (`media.py:48–72`).

**Single-emitter rule:** `paradl-events` is the only JSONL emitter. Progress frames from the worker become `paradl-events` events re-emitted by Rust with redaction. Worker human output → stderr only.

**Process policy:** ffmpeg/afconvert/7z/hdiutil/adb/JS runtimes remain external processes invoked as before; a Python process is spawned ONLY for media routes via `python -m paradl_media`.

---

## 5. Feature-flag matrix

| Flag | Crates affected | Default | Behavior when off |
|---|---|---|---|
| `api` | `paradl-api`, `paradl` | off | no HTTP server subcommand |
| `tui` | `paradl-ui`, `paradl` | off | plain progress display only |
| `media` | `paradl-media`, `paradl` | off | media routes error with install guidance |
| `platform-macos` | `paradl-platform` | off | macOS impl compiled out on non-macOS |
| `platform-windows` | `paradl-platform` | off | Windows impl compiled out on non-Windows |
| `doh` | `paradl-net`, `paradl-engine` | off | system resolver only |
| `server` | `paradl-throttle` | off | no VPS tokio server |

Default build: `cargo build` compiles with NO optional system tools. `paradl --version` and `doctor` work with no Python for all non-media paths.

---

## 6. Semver policy

- Development: `0.1.0` across all crates. Breaking changes allowed freely.
- Release (Wave 5): `1.0.0` for the framework and binary; CHANGELOG, LICENSE headers, docs.rs metadata.
- Public API stability begins at `1.0.0`. The media protocol version `paradl-media/1` is independent of crate semver.

---

## 7. Docs / examples / benches rules

- `cargo doc --no-deps` clean: no missing docs on public items.
- `cargo test --doc` green for every crate.
- Each crate's `README.md`: one-line role, public trait list, feature flags, link to example.
- Each crate's `examples/` has ≥1 runnable target (`cargo run -p <crate> --example <name>`).
- Benchmarks: cold medians, identical URLs, pinning/sentinels, ≥3 reps, CPU/RSS; warm-cache and ~1.2–1.3× drift are NOT wins; unmeasurable metrics use `{"value": null, "reason": "..."}`. Media process-startup measured on the media lane only.
