# Platform Wiring and Structural Refactor Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
**Goal:** Finish the Rust cutover's structural work by wiring the real Platform implementations into production behind a feature-gated runtime seam and splitting the 1,530-line `app.rs` god-module — without changing one byte of the golden/parity contract.
**Architecture:** `paradl-platform` gains `current() -> &'static dyn Platform`, selected with `#[cfg(all(feature, target_os))]` and a `NoopPlatform` fallback; `paradl-app` wraps it in a thin `SecretStore` adapter so `lookup_token`'s env → keychain → none chain and source metadata stay intact, and calls `set_origin_metadata` after a successful transfer. `paradl-app/src/app.rs` is decomposed into `transfer.rs`, `naming.rs`, and `commands.rs` behind byte-level characterization tests. `events.rs` keeps its explicit-null row structs, and the two HTTP probes are compared (consolidated only if behavior is identical, otherwise documented).
**Tech Stack:** Rust 2024 edition, Cargo features + `#[cfg(all(...))]` selection, `&'static dyn Platform`, `paradl-resilience::auth::{SecretStore, lookup_token}`, serde/serde_json byte-exact serialization, Tokio, reqwest, `paradl-events` single emitter.
**Spec:** docs/superpowers/specs/2026-11-21-post-review-hardening-design.md

## Global Constraints

- `rust/Cargo.toml:20`: "New crates MUST NOT depend on PyO3."
- `docs/superpowers/framework-charter.md:38`: "Default feature set compiles WITHOUT
  optional system tools."
- `docs/superpowers/framework-charter.md:172`: "single-emitter rule: `paradl-events` is
  the only JSONL emitter."
- `docs/superpowers/framework-charter.md:205`: "cargo doc --no-deps clean: no missing
  docs on public items."
- `docs/superpowers/parity-matrix.md:429+`: byte-exact parity with the deleted Python
  oracle is the acceptance contract; both Python benchmark harnesses are "retired"
  and must emit `null`+reason instead of crashing.
- Release profile is fixed: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`.

## Review Focus

- **Feature-gated selection must not leak system tools into the default build.** `default = []` stays; `current()` resolves to `NoopPlatform` unless the matching `platform-*` feature *and* `target_os` both hold. Pinned by `rust/crates/paradl-platform/tests/current.rs` (default + each feature configuration) and `cargo build -p paradl --no-default-features`.
- **Auth precedence and redaction must be preserved, not reimplemented.** `lookup_token` stays the single chain (`env → keychain → none`); `source` metadata and `register_secret` redaction must be unchanged. Pinned by `rust/crates/paradl-app/tests/auth_platform.rs` (`MapEnv` env value wins over a seeded store; source label is `env`; no raw token in the emitted JSONL) plus existing `rust/crates/paradl-app/tests/dispatch.rs` auth cases.
- **Raw-stdout nulls are a hard byte contract.** The three app row structs must keep emitting explicit `null` where schema types use `skip_serializing_if`. Pinned by `rust/crates/paradl-app/src/events.rs` unit test `explicit_null_rows_differ_from_schema_variants` and the field assertions in `dispatch.rs::info_json_emits_target_classified_then_info_with_oracle_fields` / `dispatch.rs::download_json_emits_target_classified_before_result`.
- **The `app.rs` split must be behavior-neutral, not a rewrite.** Characterization output captured before the split must be identical after it, including event order and field presence. Pinned by `rust/crates/paradl-app/tests/characterization.rs` (scrubbed-timestamp snapshot of every dispatch case) and the structural guard `app_rs_stays_under_500_lines`.
- **Probe consolidation is conditional, not assumed.** Engine `probe()` is a single bounded GET that captures final URL, validators and hash; net `probe_http_url()` is a HEAD→Range retry probe with caller-supplied timeout. They are not behaviorally identical. Pinned by the paired contract tests in `paradl-engine/src/probe.rs` (`PROBE_TIMEOUT == 15s`, cross-host header stripping) and `paradl-net/src/client.rs` (`PROBE_USER_AGENT`, `Accept-Encoding: identity`, Range fallback), plus the "why both remain" module notes.

---

## Task C1: Wire the real Platform into production

**Files:**
- Create `rust/crates/paradl-platform/tests/current.rs`
- Create `rust/crates/paradl-app/src/platform_store.rs`
- Create `rust/crates/paradl-app/tests/auth_platform.rs`
- Modify `rust/crates/paradl-platform/src/lib.rs`
- Modify `rust/crates/paradl-app/src/lib.rs` (declare `pub(crate) mod platform_store;`)
- Modify `rust/crates/paradl-app/src/app.rs` (replace `NoopPlatform` at line 1011; call origin metadata after a successful transfer)
- Verify (do not re-add if already correct) `rust/crates/paradl-app/Cargo.toml` lines 23–24 and `rust/crates/paradl/Cargo.toml` lines 21–22 already forward `platform-macos` / `platform-windows`.

**Interfaces:**
- Consumes: `paradl_platform::Platform` (`lib.rs:37`), `NoopPlatform` (`lib.rs:58`), `MacOSPlatform` (`macos.rs:407`), `WindowsPlatform` (`windows.rs:153`); `paradl_resilience::auth::{Service, SecretStore, lookup_token, OsEnv, TokenSource, TokenRecord}` (`auth.rs:35,137,203,111,79,195`); `paradl_resilience::Result<T> = std::result::Result<T, paradl_core::Error>` (`paradl-resilience/src/lib.rs:41`); `App::transfer` / `EngineOutcome` (`app.rs:487,153`).
- Produces (exact signatures):
  - `pub fn paradl_platform::current() -> &'static dyn Platform;`
  - `pub(crate) struct PlatformSecretStore(&'static dyn Platform);` with `impl paradl_resilience::auth::SecretStore for PlatformSecretStore { fn load(&self, service: Service, account: &str) -> paradl_resilience::Result<Option<String>>; fn store(&self, service: Service, account: &str, token: &str) -> paradl_resilience::Result<bool>; fn delete(&self, service: Service, account: &str) -> paradl_resilience::Result<bool>; }` — maps `Service::as_str()` → `account`, literal `"pdl"` → the platform `service` (matches `app.rs:1024` and `SiteService::keychain_service()`), and `delete` → `store_secret(service, account, "")` to preserve current Remove semantics.
  - `pub(crate) fn apply_origin_metadata(platform: &dyn Platform, dest: &std::path::Path, urls: &[String]) -> paradl_platform::Result<()>;` — best-effort seam; the production call site passes `paradl_platform::current()` and ignores the result so it emits no event and cannot change an exit code.

**Steps:**
- [ ] Write the failing interface test `rust/crates/paradl-platform/tests/current.rs` (RED is a compile error: `current` does not exist yet):
  ```rust
  use std::path::Path;
  use paradl_platform::{Platform, current};

  #[test]
  fn current_is_a_usable_platform_in_every_configuration() {
      let p = current(); // &'static dyn Platform
      assert!(p.set_origin_metadata(Path::new("/definitely/not/a/file"), &[]).is_ok());
      assert_eq!(p.load_secret("pdl", "hf").expect("load").is_none(), true);
  }
  ```
- [ ] Verify it FAILS: `cd /Users/tawsifx/paradl/rust && cargo test -p paradl-platform --test current` (unresolved import).
- [ ] Implement minimal `current()` in `paradl-platform/src/lib.rs` with three mutually exclusive arms:
  ```rust
  #[cfg(all(feature = "platform-macos", target_os = "macos"))]
  pub fn current() -> &'static dyn Platform { static P: MacOSPlatform = MacOSPlatform { keychain: None }; &P }
  #[cfg(all(feature = "platform-windows", target_os = "windows"))]
  pub fn current() -> &'static dyn Platform { static P: WindowsPlatform = WindowsPlatform { store_path: None }; &P }
  #[cfg(not(any(all(feature = "platform-macos", target_os = "macos"),
                 all(feature = "platform-windows", target_os = "windows"))))]
  pub fn current() -> &'static dyn Platform { &NoopPlatform }
  ```
  Document the deliberate `dyn` seam here: runtime OS/feature selection cannot be expressed with static dispatch without duplicating call sites; all other charter traits stay zero-cost static-dispatch bounds (spec C1). Add a `#![warn(missing_docs)]`-satisfying doc comment on `current`.
- [ ] Verify PASS in all four configurations:
  ```
  cargo test -p paradl-platform --test current
  cargo test -p paradl-platform --test current --features platform-macos
  cargo test -p paradl-platform --test current --features platform-windows
  cargo test -p paradl-platform
  ```
- [ ] Write failing auth test `rust/crates/paradl-app/tests/auth_platform.rs`: seed `InMemorySecretStore` with a ≥4-char keychain token, set `HF_TOKEN` in the test env via a `MapEnv`-driven helper, and assert (a) `lookup_token(Service::Hf, &env, &PlatformSecretStore(current()))` reports `TokenSource::Env`, (b) the emitted `auth_status` JSON `source` is `"env"` when both exist, (c) no raw token substring appears in captured output. Note: if `PlatformSecretStore` is `pub(crate)`, expose the precedence assertion as a `#[cfg(test)]` unit test inside `platform_store.rs` and keep the integration test at the dispatch level (`paradl auth status --json`) — do not widen the public API just for a test.
- [ ] Verify the auth test FAILS (current `app.rs` uses `InMemorySecretStore::new()` and hard-codes `NoopPlatform`, so seeded platform tokens are never consulted).
- [ ] Implement `PlatformSecretStore` in `paradl-app/src/platform_store.rs`; switch `App::auth` and `App::auth_status` (`app.rs:1004`, `1052`) to `lookup_token(service, &OsEnv, &PlatformSecretStore(paradl_platform::current()))`. Keep `register_secret` calls and the `auth_service` source-label mapping (`app.rs:1465`) unchanged. Do NOT modify `lookup_token` itself, `paradl-resilience/src/auth.rs`, or `paradl-events/src/schema.rs`.
- [ ] Replace `let platform = paradl_platform::NoopPlatform;` (`app.rs:1011`) with the `current()`-backed adapter; keep `AuthAction::Remove`'s existing `store_secret(..., "")` behavior even when the platform rejects an empty secret (pre-existing semantics; no new trait method).
- [ ] In `App::transfer`, after `run_engine` yields `success == true`, call `apply_origin_metadata(paradl_platform::current(), &dest, &[target.to_owned()])`. Place it after the history write and before event emission; never on the offline path, never on failure, never inside `cli.json` output.
- [ ] Unit-test `apply_origin_metadata` in `platform_store.rs` with a spy `Platform` that records `(path, urls)`; assert it is called once with the destination and original target on success and zero times on failure (call it directly in the test; the production call ignores errors).
- [ ] Verify PASS: `cargo test -p paradl-app --test auth_platform`, `cargo test -p paradl-app --test dispatch`, `cargo test -p paradl-app`.
- [ ] Feature-forward verification: confirm `platform-macos`/`platform-windows` chains `paradl` → `paradl-app` → `paradl-platform` are present; add them only if missing. Then run `cargo build -p paradl --no-default-features`, `cargo build -p paradl --all-features`, and on the host OS `cargo run -p paradl --features platform-macos -- auth status --json` (smoke: valid single JSONL line, no raw token).
- [ ] Run suite and docs: `cargo test --all -- --skip full_non_media_gate_is_clean`, `cargo clippy --all -- -D warnings`, `cargo doc --no-deps --all`.
- [ ] Commit on the work branch (never `main`): `git add rust/crates/paradl-platform rust/crates/paradl-app rust/crates/paradl/Cargo.toml && git commit -m "C1: wire real Platform implementations into production"`.

## Task C2: Split `app.rs` into focused modules

**Files:**
- Create `rust/crates/paradl-app/tests/characterization.rs`
- Create `rust/crates/paradl-app/src/transfer.rs`
- Create `rust/crates/paradl-app/src/naming.rs`
- Create `rust/crates/paradl-app/src/commands.rs`
- Modify `rust/crates/paradl-app/src/app.rs` (orchestration + shared state only)
- Modify `rust/crates/paradl-app/src/lib.rs` (module declarations only if needed; keep `pub use app::{ApiBridge, ApiResolve, App, Capture, OutputSink, TransferSetup}` stable)

**Interfaces:**
- Consumes: `App` (`app.rs:168`), `OutputSink` (`app.rs:46`), `Dispatch` impl (`app.rs:1306`), `crate::events::{InfoRow, ProgressRow, ResultRow, TargetClassifiedRow}`, `crate::handoffs::*`, `crate::platform_store::{apply_origin_metadata, PlatformSecretStore}`, `paradl_cli::{Cli, Command, ExitCode}`.
- Produces (all moved handlers are `pub(super)` inside child modules — visible to `app.rs` dispatch but never in the public API):
  - `transfer.rs`: `pub(super) fn download(&mut self, cli: &Cli) -> ExitCode;`, `pub(super) fn media_route(&mut self, target: &str, cli: &Cli) -> Result<(), paradl_core::Error>;`, `pub(super) fn transfer(&mut self, target: &str, decision: &paradl_net::routing::RouteDecision, cli: &Cli, isp: &str) -> Result<ExitCode, paradl_core::Error>;`, `pub(super) fn run_engine(&mut self, target: &str, cli: &Cli, dest: &std::path::Path) -> Result<EngineOutcome, paradl_core::Error>;`, `pub(crate) struct EngineOutcome`, `pub(crate) struct ProgressTelemetry`, `pub struct TransferSetup`; `app.rs` re-exports `TransferSetup` so `paradl_app::TransferSetup` is unchanged.
  - `naming.rs`: `pub(super) fn fallback_name(target: &str) -> String;`, `pub(super) fn destination(cli: &Cli, file_name: &str) -> PathBuf;`, `pub(super) fn clean_orphans(directory: &std::path::Path) -> Vec<String>;`; also the pure routing-name helpers that travel with them: `decision_name`, `decision_filename`, `decision_clean_url`, `classification_info`, `ClassificationInfo`, `url_host`.
  - `commands.rs`: config/history/doctor helpers plus the remaining `App` handlers (`auth`, `auth_status`, `info`, `hash`, `extract`, `history`, `clean`, `run_config`, `profiles`, `queue`, `resume`, `netprobe`, `bench`, `throttle`, `pip`, `brew`, `install`, `server`, `tui`, `doctor`) and their pure helpers (`result_line`, `auth_service`, `HashesToHistory`, `probe_snapshot`).
  - `app.rs` retains: `App`, `OutputSink`, `stdout_sink`, `ApiResolve`, `ApiBridge`, `AppExecutor`, `Capture`, `runtime`, `line`, `publish`, `fail`, `expand_targets`, `dispatch`, and the `Dispatch` impl.

**Steps:**
- [ ] Write `characterization.rs` BEFORE moving any code. Capture every `dispatch.rs` CASES argv in `--json` mode with an injected sink, scrub only the timestamp value, and snapshot the ordered lines; assert event order and byte-level field presence. Include the structural guard.
  ```rust
  fn scrub(line: &str) -> String {
      let key = "\"timestamp\":";
      let Some(i) = line.find(key) else { return line.to_owned() };
      let rest = &line[i + key.len()..];
      let n = rest.bytes().take_while(|b| b.is_ascii_digit() || *b == b'.' || *b == b'-' || *b == b'e' || *b == b'+').count();
      format!("{}<TS>{}", &line[..i + key.len()], &rest[n..])
  }

  #[test]
  fn app_rs_stays_under_500_lines() {
      let n = include_str!("../src/app.rs").lines().count();
      assert!(n < 500, "app.rs is {n} lines after refactor");
  }
  ```
  Fill `EXPECTED` from the pre-split tree (include `info` → two lines; `download` → two lines; `auth status` → one line; `doctor` golden already covered by `tests/doctor.rs`).
- [ ] Verify RED correctly: characterization snapshot PASSES on the current tree; `app_rs_stays_under_500_lines` FAILS at 1,530 lines. If the snapshot fails before any code move, fix the harness first — never bless a snapshot after the split.
- [ ] Move code mechanically, in three commits-free increments within the task (no logic edits, no import cleanup beyond what compiles): `transfer.rs` first, then `naming.rs`, then `commands.rs`. Keep each handler body byte-identical; only change `fn` visibility to `pub(super)` and add `use super::{App, ...}` / `use crate::...` imports.
- [ ] Keep `app.rs` as orchestration only: `dispatch` still owns the `match`, and all shared state/methods remain on `App`. Add a short module note explaining that the other charter traits stay static-dispatch bounds while only `Platform` uses `dyn` (spec C1).
- [ ] Verify PASS: `cargo test -p paradl-app --test characterization`, `cargo test -p paradl-app --test dispatch`, `cargo test -p paradl-app --test handoffs`, `cargo test -p paradl-app --test doctor`, `cargo test -p paradl-app --test online_transfer`, `cargo test -p paradl-app`.
- [ ] Run suite and docs: `cargo test --all -- --skip full_non_media_gate_is_clean`, `cargo fmt --all -- --check`, `cargo clippy --all -- -D warnings`, `cargo doc --no-deps --all` (new `pub(super)` items are not public API; re-exports keep docs clean).
- [ ] Commit on the work branch: `git add rust/crates/paradl-app && git commit -m "C2: split app.rs into transfer, naming and commands modules"`.

## Task C3: Keep explicit-null rows; dedup only serde-compatible types

**Files:**
- Modify `rust/crates/paradl-app/src/events.rs` (add `#[cfg(test)] mod tests` and the exception doc comment)
- Verify untouched: `rust/crates/paradl-events/src/schema.rs`, `rust/crates/paradl-events/tests/golden.rs`, all golden fixtures under `rust/crates/paradl-app/tests/`
- Modify (only if a byte-identical candidate is proven): `rust/crates/paradl-app/src/app.rs` / `commands.rs` call sites

**Interfaces:**
- Consumes: `paradl_events::{emit_jsonl, TargetClassifiedEvent, ProgressEvent, ResultEvent, InfoEvent, Event}` (`paradl-events/src/schema.rs:823,535,771,408,928`; `paradl-events/src/lib.rs:127`); `paradl_platform` not involved.
- Produces: a failing-if-broken byte-comparison test proving the rows and schema variants are NOT interchangeable, plus a documented exception block in `events.rs`:
  ```rust
  /// EXCEPTION (post-review C3): these row structs are deliberately NOT replaced
  /// by `paradl_events` schema types. `emit_jsonl` with Python separators writes
  /// explicit `null` on raw stdout; schema types use `#[serde(skip_serializing_if)]`
  /// and omit the key. Pinned by `paradl-app/tests/dispatch.rs`
  /// (`info_json_emits_target_classified_then_info_with_oracle_fields`,
  /// `download_json_emits_target_classified_before_result`) and the doctor/parity
  /// goldens. Do not change schema serde attributes or golden nulls.
  ```
  Test snippet (inside `events.rs`):
  ```rust
  #[test]
  fn explicit_null_rows_differ_from_schema_variants() {
      let row = paradl_events::emit_jsonl(&TargetClassifiedRow::new(1.0, "t", "direct", "u", None, None, None));
      assert!(row.contains("\"filename_hint\": null"));
      let schema = paradl_events::emit_jsonl(&paradl_events::TargetClassifiedEvent::new(
          1.0, "t".into(), "direct".into(), "u".into(), None, None, None));
      assert!(!schema.contains("filename_hint"), "schema drops the key; row must not");
  }
  ```
- No new public types; no schema attribute edits; no golden edits.

**Steps:**
- [ ] Add the byte-comparison test above plus analogous assertions for `ProgressRow` (`total_bytes`, `eta_seconds`, `peer_count` → `null`) and `ResultRow` (`file_path`, `audio_format_produced`, `checksum_*`, `error` → `null`).
- [ ] Verify it PASSES on the current tree and documents the divergence; confirm `cargo test -p paradl-events --test golden` is green and `git diff -- rust/crates/paradl-events` is empty.
- [ ] Audit each remaining `events.rs` type (`InfoRow`, `InfoProperties`) against its schema counterpart. Replace with a schema type ONLY if a byte-comparison test proves identical output for both `None` and `Some` cases. Given `InfoProperties` uses `skip_serializing_if` on `Filename Hint` and schema `InfoEvent` carries `properties: Value`, expect zero replacements. Record the outcome in the `events.rs` exception comment.
- [ ] Document the three kept structs explicitly by name (`TargetClassifiedRow`, `ProgressRow`, `ResultRow`) with the reason (`skip_serializing_if` incompatibility) and the link to the raw-stdout goldens. Do not touch `schema.rs` or delete nulls from goldens — that would move away from the Python oracle.
- [ ] Verify PASS: `cargo test -p paradl-app --test dispatch`, `cargo test -p paradl-app`, `cargo test -p paradl-events`.
- [ ] Run suite and docs: `cargo test --all -- --skip full_non_media_gate_is_clean`, `cargo clippy --all -- -D warnings`, `cargo doc --no-deps --all`.
- [ ] Commit on the work branch: `git add rust/crates/paradl-app && git commit -m "C3: document explicit-null row exception; dedup only compatible types"`.

## Task C4: Compare and document the two HTTP probes

**Files:**
- Modify `rust/crates/paradl-engine/src/probe.rs` (module doc note + `#[cfg(test)] mod tests` contract test)
- Modify `rust/crates/paradl-net/src/client.rs` (module doc note + contract test in the existing `#[cfg(test)] mod tests`)
- Verify untouched: `rust/crates/paradl-engine/src/engine.rs`, `rust/crates/paradl-engine/src/download.rs`, `rust/crates/paradl-net/src/lib.rs` re-exports
- No source consolidation unless the behavior comparison proves identical (it does not — see decision below).

**Interfaces:**
- Consumes: `pub async fn paradl_engine::probe::probe(client: &reqwest::Client, url: url::Url, headers: reqwest::header::HeaderMap, cookie: Option<&str>) -> paradl_engine::error::Result<ProbeInfo>` (`probe.rs:145`, `ProbeInfo` at `:123`); `pub const PROBE_TIMEOUT: Duration = 15s` (`probe.rs:29`); `pub fn scope_headers(...)` (`probe.rs:106`); `pub async fn paradl_net::client::probe_range_support(client: &HttpClient, url: &str, headers: &HashMap<String,String>, timeout: Duration) -> Result<HttpProbeResult>` (`client.rs:520`); `pub async fn probe_http_url(client: &HttpClient, url: &str, headers: &HashMap<String,String>, timeout: Duration) -> Result<HttpProbeResult>` (`client.rs:584`); `PROBE_USER_AGENT` (`client.rs:27`).
- Produces: a shared decision table embedded as a short module note in both files, and two contract tests. No changed signatures, no deleted functions.

**Decision to encode (do not force a merge):**

| Dimension | engine `probe()` | net `probe_http_url()` |
|---|---|---|
| HTTP method | one GET, `Range` removed, body not read | HEAD, then `Range: bytes=0-0` fallback on 400/403/405/501/transport error/no range confirmation (2 retries) |
| Timeout | fixed `PROBE_TIMEOUT = 15s` via `tokio::time::timeout` | caller-supplied `Duration` (`app.rs:760` passes `max(1.0, cli.duration)`) |
| Client | `reqwest::Client` from `build_client` (proxy, IP family, `doh` feature) | `HttpClient` wrapper (`Arc`-backed, retry config) |
| Result | `ProbeInfo {url, final_url, size, resumable, file_name, content_type, validators, checksum}` drives planner, workers and socket recycles | `HttpProbeResult {ok, final_url, status_code, content_type, content_length, filename, accept_ranges, etag_sha256, verified_sha256, headers}` drives the `netprobe` event |
| Headers/cookie | raw cookie attached to the original URL; reqwest redirect stripping plus `scope_headers` at worker/recycle sites | `User-Agent` + `Accept-Encoding: identity` via `probe_request_headers`; no cookie field |
| Filename/validators | RFC 6266 `filename*`/`filename`, percent-decode, URL tail, `Validators::from_response`, `Hash::from_headers` | `parse_content_disposition_filename`, `extract_etag_sha256`, `extract_verified_sha256` |

  → **Not identical. Keep both and document why.** Consolidating into `paradl-net` would change the engine's probe method, timeout semantics, final-URL capture and validator/hash outputs, moving away from the oracle. Only revisit if a future change makes the two contracts literally the same.

**Steps:**
- [ ] Write the engine contract test first (inside `probe.rs`, RED only if someone has already changed the contract):
  ```rust
  #[test]
  fn engine_probe_contract_is_a_bounded_get() {
      assert_eq!(PROBE_TIMEOUT, Duration::from_secs(15));
      let mut h = HeaderMap::new();
      h.insert(reqwest::header::AUTHORIZATION, HeaderValue::from_static("secret"));
      let scoped = scope_headers(&h, Some("a.test"), Some("b.test"));
      assert!(scoped.get(reqwest::header::AUTHORIZATION).is_none());
  }
  ```
  (Deref-based `.get(...)` works whether `scope_headers` returns `HeaderMap` or the Workstream-A `Cow<'_, HeaderMap>`.)
- [ ] Write the net contract test first (inside `client.rs::tests`):
  ```rust
  #[test]
  fn net_probe_contract_is_head_then_range() {
      assert_eq!(PROBE_USER_AGENT, /* existing constant value */);
      let headers = probe_request_headers(&HashMap::new());
      assert_eq!(headers.get("Accept-Encoding").map(String::as_str), Some("identity"));
  }
  ```
- [ ] Verify both PASS on the current tree — the tests pin today's behavior; if either fails, the probe has already drifted and must be restored before C4 proceeds.
- [ ] Add the decision table above as a concise module note in `paradl-engine/src/probe.rs` and `paradl-net/src/client.rs`, each pointing at the other function and stating "kept separate intentionally; see the post-review hardening design, C4". No code moves, no re-exports changed.
- [ ] Verify no behavioral drift: `cargo test -p paradl-engine`, `cargo test -p paradl-net`, `cargo test -p paradl-app --test online_transfer`.
- [ ] Run suite and docs: `cargo test --all -- --skip full_non_media_gate_is_clean`, `cargo clippy --all -- -D warnings`, `cargo doc --no-deps --all`, `git diff --check`.
- [ ] Commit on the work branch: `git add rust/crates/paradl-engine/src/probe.rs rust/crates/paradl-net/src/client.rs && git commit -m "C4: document engine/net probe divergence and pin both contracts"`.

---

## Workstream C exit gate

- [ ] `cd /Users/tawsifx/paradl/rust && cargo test --all -- --skip full_non_media_gate_is_clean` green.
- [ ] `cargo clippy --all -- -D warnings` and `cargo fmt --all -- --check` green.
- [ ] `cargo doc --no-deps --all` clean (no missing docs on public items).
- [ ] `cargo build -p paradl --no-default-features` and `cargo build -p paradl --all-features` green; `paradl doctor` and `paradl auth status --json` smoke tests emit valid single-emitter JSONL.
- [ ] `git status` shows only the four C1–C4 commits' intended files; no schema attributes, golden fixtures, help fixtures or parity-matrix rows were changed.
