# Engine Efficiency Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
**Goal:** Remove blocking filesystem work from Tokio async workers and stop redundant compile/allocate/clone work in the engine, store, and postproc crates without changing any byte-exact contract.
**Architecture:** Each finding becomes one independently compilable, independently committable task (A1–A5). A1 mirrors the already-proven `verify_file` offload pattern (`tokio::task::spawn_blocking` + owned `Send + 'static` inputs); A3 makes `SqliteStore` own one `Arc<Mutex<Connection>>` so repeated SQL can use `prepare_cached`; A4 and A5 are local, type-level optimizations pinned by focused unit tests. Per-chunk `pwrite` deliberately stays on the executor (A2) because a per-write blocking round trip costs more than a page-cache-hit write.
**Tech Stack:** Rust 2024 (rustc 1.98), Tokio `task::spawn_blocking`, rusqlite 0.32 (`Connection` is `Send` not `Sync`; `prepare_cached`), `std::borrow::Cow`, `std::sync::LazyLock<regex::Regex>`, existing byte-exact golden/fault-injection suites.
**Spec:** docs/superpowers/specs/2026-11-21-post-review-hardening-design.md

## Global Constraints

- `rust/Cargo.toml:20`: "New crates MUST NOT depend on PyO3."
- `docs/superpowers/framework-charter.md:38`: "Default feature set compiles WITHOUT optional system tools."
- `docs/superpowers/framework-charter.md:172`: "single-emitter rule: `paradl-events` is the only JSONL emitter."
- `docs/superpowers/framework-charter.md:205`: "cargo doc --no-deps clean: no missing docs on public items."
- `docs/superpowers/parity-matrix.md:429+`: byte-exact parity with the deleted Python oracle is the acceptance contract; both Python benchmark harnesses are "retired" and must emit `null`+reason instead of crashing.
- Release profile is fixed: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`.

Additional hard rules for this workstream (from the design's Decisions and Anti-goals): no new dependencies; no change to the download concurrency model, emitted event shapes, CLI help text, or SQLite DDL; no lock guard held across an await or a blocking fsync; every change moves *toward* the deleted Python oracle. All commands below run from `/Users/tawsifx/paradl/rust`.

## Review Focus

- **A1 — guard-across-blocking-I/O regression.** The bug is not "sidecar missing" (the initial sidecar at `download.rs:1085-1088` already creates it) but "`resume_state` / `planner` guard held while `sync_data` + temp-file rename run". Pinned by `sidecar_blocking_helper_is_owned_send_static_and_writes_loadable_sidecar` (the helper takes no `&Mutex`/`&Shared` at all, so holding a guard is unrepresentable) plus `sidecar_write_via_spawn_blocking_is_loadable`, and by keeping `cancel_persists_sidecars_for_resume` green for the real transfer path. A source-text-only grep was rejected because a behavioral test is possible: the extracted helper can be executed on the real blocking pool and its output read back with `ResumeState::load`.
- **A1/A5 — call-site fallout from signature changes.** `maybe_save()` becomes `async` (two call sites) and `scope_headers()` becomes `Cow<'_, HeaderMap>` (two call sites, both requiring `.into_owned()` before `.headers(...)` / `probe(...)`). Pinned by plain `cargo test -p paradl-engine`: these are compile errors if any call site is missed, so RED is a build failure, not a silent behavior change.
- **A3 — re-entrant `Mutex<Connection>` deadlock.** `Queue::add_job` and `Queue::update_job_status` call `self.get_job()` internally; if they call it while holding the new mutex guard the process self-deadlocks. Pinned by the new `queue_add_then_status_update_completes` unit test and by the existing `paradl-resilience` queue tests (`tests/resilience.rs:27`) — a deadlock manifests as a hung test, which CI kills.
- **A3 — schema-guard / WAL / corrupt-DB-rotation semantics.** Collapsing many `connect()` calls into one must not double-init the schema or skip the `.bad` rotation. Pinned by the existing `guard_initializes_once_per_physical_file` and `batch_of_writes_uses_one_initialized_connection`, plus the new `with_connection_reuses_one_connection` (pointer identity) and `reused_connection_is_in_wal_mode` tests, and `cargo test -p paradl-store` (golden schema tests in `tests/golden_schema.rs`).
- **A4/A5 — byte-exact and credential-leak regressions.** Hoisting the BSD regex must keep `SHA256`/`SHA-256`/`SHA512`/`SHA-512`/`BLAKE2b` parsing identical, and `Cow::Borrowed` must be returned *only* on the same-host path so `Authorization`/`Cookie`/`Proxy-Authorization` are still stripped cross-host. Pinned by `bsd_regex_static_still_parses_all_variants` and `scope_headers_borrows_same_host_and_owns_cross_host`, plus the existing sidecar/credential goldens.

---

## Task A1: Offload sidecar writes and final rename to the blocking pool

**Files:**
- Modify `rust/crates/paradl-engine/src/download.rs` — `Shared::maybe_save` (~365-379), both worker call sites (~594, ~618), final sidecar save (~830-836), final `rename`/`remove_file` (~857-860), new owned helpers + `#[cfg(test)] mod tests`.
- Test `rust/crates/paradl-engine/src/tests.rs` — strengthen `cancel_persists_sidecars_for_resume` (~1313-1343) with a post-progress refresh assertion; do not change its oracle contract.

**Interfaces:**
- Consumes: `ResumeState::save(&mut self, path: &Path, segments: Vec<SavedSegment>, part: &File) -> io::Result<()>` (`state.rs:107`); `ResumeState: Clone` (`state.rs:20`); `Shared::maybe_save(&self)` (`download.rs:365`); the sync final save block (`download.rs:830-836`) and `std::fs::rename`/`remove_file` (`download.rs:857-860`); the `verify_file` offload pattern (`download.rs:883-888`).
- Produces (private to `download.rs`):
  - `fn save_sidecar_blocking(mut state: ResumeState, path: PathBuf, segments: Vec<SavedSegment>, part: std::fs::File) -> std::io::Result<()>` — owned, `Send + 'static`; body is `state.save(&path, segments, &part)`.
  - `struct FinalizeArgs { part_path: PathBuf, dest: PathBuf, sidecar: PathBuf, state: Option<ResumeState>, segments: Vec<SavedSegment> }`
  - `fn finalize_blocking(args: FinalizeArgs) -> std::io::Result<()>` — optional final `state.save(...)`, then `std::fs::rename(&args.part_path, &args.dest)?`, then `let _ = std::fs::remove_file(&args.sidecar);`.
  - `async fn maybe_save(&self)` — snapshot `planner.save()`, clone `Option<ResumeState>` under a short guard, `self.part.try_clone()?` the descriptor on the async thread (a `dup`, not an fsync), then `let _ = tokio::task::spawn_blocking(move || save_sidecar_blocking(state, path, saved, part)).await;`. **Both guards are dropped before the await.** Cloning the state is safe: only `checksum`/`validators` are read later from the shared copy; `segments` is always re-sourced from the planner.
  - `supervise` builds `FinalizeArgs` from short-lived guard blocks (`let state = shared.resume_state.lock().unwrap().clone();`, `let segments = shared.planner.lock().unwrap().save();`) and does `tokio::task::spawn_blocking(move || finalize_blocking(args)).await` after the checksum verification, mapping the `JoinError` via `error::verification(...)` as `verify_file` does.

Steps:
- [ ] **Write the failing test** in `download.rs`'s new `#[cfg(test)] mod tests`, and change the two call sites to `shared.maybe_save().await;`:
  ```rust
  #[test]
  fn sidecar_blocking_helper_is_owned_send_static_and_writes_loadable_sidecar() {
      fn assert_send_static<T: Send + 'static>(_: T) {}
      let dir = tempfile::tempdir().unwrap();
      let dest = dir.path().join("a.bin");
      let part = std::fs::File::create(dir.path().join("a.bin.pdl.part")).unwrap();
      let url: url::Url = "http://x/a.bin".parse().unwrap();
      let state = crate::state::ResumeState::new(
          url.clone(), url, dest.clone(), 4, crate::state::Validators::default(),
      );
      let segments = vec![crate::planner::SavedSegment { start: 0, cursor: 4, end: 4 }];
      let path = crate::naming::state_path(&dest);
      let mut expected = state.clone();
      expected.segments = segments.clone(); // save() records the segments it was given

      // Compile-time proof the inputs can cross into spawn_blocking.
      assert_send_static(state.clone());
      assert_send_static(segments.clone());
      assert_send_static(path.clone());
      assert_send_static(part.try_clone().unwrap());

      // Behavioral proof: owned data alone writes a byte-exact sidecar.
      crate::download::save_sidecar_blocking(
          state.clone(), path.clone(), segments.clone(), part,
      ).unwrap();
      let loaded = crate::state::ResumeState::load(&path).unwrap();
      assert_eq!(loaded, expected);
      assert_eq!(loaded.segments, segments);
  }

  #[tokio::test]
  async fn sidecar_write_via_spawn_blocking_is_loadable() {
      // Same owned inputs, but executed on Tokio's blocking pool exactly as maybe_save does.
      // ... build dir/part/state/segments/path as above ...
      let handle = tokio::task::spawn_blocking(move || {
          crate::download::save_sidecar_blocking(state, path.clone(), segments, part)?;
          Ok::<_, std::io::Error>(path)
      });
      let path = handle.await.unwrap().unwrap();
      assert_eq!(crate::state::ResumeState::load(&path).unwrap().downloaded(), 4);
  }
  ```
  This is not a source-text test: it compiles the helper as an owned `Send + 'static` function, runs it on the real blocking pool, and reads the sidecar back. The "no guard held" property is structural — the helper signature has no `&Mutex`/`&Shared` parameter, so a guard cannot be passed into the blocking work. A full `Shared` construction in a unit test would require a live reqwest client, planner and watch channels and would add no signal beyond the signature plus blocking-pool execution, so it is deliberately not built. In `tests.rs`, add to `cancel_persists_sidecars_for_resume` an assertion that after committed progress the sidecar's `downloaded()` is greater than the initial zero snapshot (the real transfer path).
- [ ] **Run and verify FAILS for the expected reason** — `cargo test -p paradl-engine sidecar_blocking -- --nocapture`. Expected RED: compile errors `E0425`/`E0433` (`cannot find function save_sidecar_blocking`) and `E0308`/`future is not Send` at `shared.maybe_save()` because `maybe_save` is still a sync `fn` being `.await`ed. A compile failure is the correct RED here: the regression is a structural (type/signature) property of the code, not a value that can be wrong at runtime.
- [ ] **Implement minimal code** in `rust/crates/paradl-engine/src/download.rs`: add the two owned helpers and `FinalizeArgs`, convert `maybe_save` to `async fn` with the short-lock snapshot above, update both worker call sites to `.await`, replace the final save + `rename`/`remove_file` with the single `finalize_blocking` spawn_blocking call. Do not alter `ResumeState::save`, `atomic_write`, or any event emission.
- [ ] **Run test, verify PASS** — `cargo test -p paradl-engine sidecar_blocking -- --nocapture` and `cargo test -p paradl-engine cancel_persists_sidecars_for_resume -- --nocapture`; both GREEN, sidecar bytes unchanged.
- [ ] **Run the crate/full suite** — `cargo test -p paradl-engine`, then `cargo test -p paradl-engine state_save_load_round_trip` and `cargo test -p paradl-engine sidecar_bytes_match_oracle_golden` (L14 byte-exactness), then `cargo clippy --all -- -D warnings`.
- [ ] **Commit** — `git add rust/crates/paradl-engine/src/download.rs rust/crates/paradl-engine/src/tests.rs && git commit -m "perf(engine): offload sidecar writes and final rename to blocking pool"`.

## Task A2: Document the per-chunk pwrite tradeoff (documentation only)

**Files:**
- Modify `rust/crates/paradl-engine/src/download.rs` — doc comment on `write_at` (~392-394) and one cross-reference line on `Shared::maybe_save` (~363).
- Test: none. This task intentionally has no failing test: it changes no behavior, so a RED step would be artificial. Verification is the doc build plus clippy (see step 2/5).

**Interfaces:**
- Consumes: `fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()>` (`download.rs:394`); its two call sites at `download.rs:586` and `download.rs:611`; A1's offloaded `maybe_save` / `finalize_blocking` and `verify_file` (`download.rs:883`).
- Produces: doc text only — no signature changes. Must state that each worker writes only bytes it already claimed/committed (so no lock is held across the write), that `write_all_at`/the cloned-descriptor fallback is a page-cache-hit syscall, that a `spawn_blocking` hop per chunk at up to 16 workers would cost more than the write itself, and that the expensive blocking operations (fsync + temp-file rename in the sidecar, final rename, file hashing) are the ones offloaded by A1 and `verify_file`.

Steps:
- [ ] **Write the failing test** — N/A (documentation-only task; no behavioral contract to pin). The reviewable artifact is the doc comment itself; A1's tests plus the clippy/doc checks below are the guard against accidental code movement.
- [ ] **Run and verify FAILS for the expected reason** — N/A; replaced by a baseline check that docs currently build: `cargo doc -p paradl-engine --no-deps`. Expected GREEN baseline (no new warning). There is intentionally no RED state for a comment-only change.
- [ ] **Implement minimal code** — add the tradeoff paragraph to the `write_at` doc comment in `rust/crates/paradl-engine/src/download.rs`, and a one-line `///` note on `maybe_save` pointing to it. Do not touch the `write_at` body or its call sites.
- [ ] **Run test, verify PASS** — `cargo doc -p paradl-engine --no-deps` stays clean (no `missing_docs` warnings; the crate uses `#![warn(missing_docs)]` at `lib.rs:42`).
- [ ] **Run the crate/full suite** — `cargo test -p paradl-engine` and `cargo clippy --all -- -D warnings`.
- [ ] **Commit** — `git add rust/crates/paradl-engine/src/download.rs && git commit -m "docs(engine): document per-chunk pwrite tradeoff"`.

## Task A3: Reuse one Mutex<Connection> per SqliteStore with cached statements

**Files:**
- Modify `rust/crates/paradl-store/src/store.rs` — `SqliteStore` struct (~22-26), `open_at` (~35-43), `with_connection` (~61-67), `connect`/`open_and_init` (~70-113), `record_transfer` (~129-158), `recent` (~160-188), `open_queue` (~190-192), tests (~195-255).
- Modify `rust/crates/paradl-store/src/queue.rs` — `Queue` struct (~12-20), `new` (~18-20), all `self.conn.*` calls (~26-199), especially the `get_job`-calling methods `add_job` (~26-68) and `update_job_status` (~99-119).
- Modify `rust/crates/paradl-store/src/lib.rs` — `Store` trait doc (~60-63) which currently claims "Every method opens a fresh managed connection".
- Test `rust/crates/paradl-store/tests/golden_schema.rs` — run unchanged; add no fixture edits (DDL is frozen).

**Interfaces:**
- Consumes: `Store` trait (`lib.rs:63-74`); `SqliteStore { path: PathBuf, history_limit: usize }` (`store.rs:22-26`); `connect() -> Result<Connection>` and `open_and_init() -> rusqlite::Result<Connection>` (`store.rs:70-113`); `with_connection<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T>` (`store.rs:61-67`); `Queue { conn: Connection }` / `Queue::new(conn: Connection)` (`queue.rs:12-20`); `SCHEMA_INIT_COUNT` (`store.rs:13`); `rusqlite 0.32` with `bundled` (`rust/Cargo.toml:34`).
- Produces:
  ```rust
  // store.rs — Connection is Send but NOT Sync, so the shared handle is Mutex-wrapped.
  #[derive(Debug, Clone)]
  pub struct SqliteStore {
      path: PathBuf,
      history_limit: usize,
      conn: Arc<Mutex<Connection>>,
  }
  impl SqliteStore {
      pub fn open() -> Result<Self>;
      pub fn open_at(path: impl AsRef<Path>) -> Result<Self>;
      pub fn with_history_limit(mut self, limit: usize) -> Self;
      pub fn path(&self) -> &Path;
      pub fn with_connection<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T>;
      fn open_connection(&self) -> Result<Connection>;         // corrupt-DB .bad rotation + retry (was connect())
      fn open_and_init(&self) -> rusqlite::Result<Connection>;  // one open: busy_timeout + WAL once + guard/init
  }
  // queue.rs
  #[derive(Debug)]
  pub struct Queue { conn: Arc<Mutex<Connection>> }
  impl Queue { pub(crate) fn new(conn: Arc<Mutex<Connection>>) -> Self; }
  ```
  - `open_and_init`: `Connection::open(&self.path)?` → `conn.busy_timeout(Duration::from_secs(10))?` on **every** open → `conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?` **once at init only** → unchanged `InitGuard` / `schema::initialize` / `SCHEMA_INIT_COUNT` / `migrate_legacy_json` block. `open_at` calls `open_connection()` exactly once and stores the result.
  - Locking helper: `fn lock_conn(&self) -> MutexGuard<'_, Connection>` using `.unwrap_or_else(std::sync::PoisonError::into_inner)`; reuse the same pattern inside `Queue`.
  - `with_connection`, `record_transfer`, `recent`: lock once, then use `conn.prepare_cached(SQL)?` for the fixed SQL strings (INSERT INTO downloads, cap DELETE, SELECT ... LIMIT ?). `open_queue`: `Ok(Queue::new(self.conn.clone()))`.
  - `queue.rs`: every method locks the shared mutex; fixed SQL (`get_job`, both `list_jobs` strings, `update_job_status`, `recover_crashed_jobs`) uses `prepare_cached`; dynamic `IN (?,?,…)` SQL in `remove_jobs`/`pause_jobs`/`resume_jobs` stays `execute(&dynamic_sql, params_from_iter(...))` because the SQL text varies. **Critical:** drop the guard before calling `get_job` in `add_job` and `update_job_status` (statement-scoped locks) to avoid self-deadlock.
  - Update the `Store` trait doc to say implementations are `Send + Sync` because they share a `Mutex<Connection>` — *not* because each method opens a fresh connection.

Steps:
- [ ] **Write the failing test** in `rust/crates/paradl-store/src/store.rs` tests:
  ```rust
  #[test]
  fn with_connection_reuses_one_connection() {
      let dir = tempfile::tempdir().unwrap();
      let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
      let ptr = { let g = store.conn.lock().unwrap(); &*g as *const Connection };
      store.recent(5).unwrap();
      store.with_connection(|c| c.execute_batch("SELECT 1")).unwrap();
      let again = { let g = store.conn.lock().unwrap(); &*g as *const Connection };
      assert_eq!(ptr, again, "operations must reuse one Connection, not reconnect");
  }

  #[test]
  fn reused_connection_is_in_wal_mode() {
      let dir = tempfile::tempdir().unwrap();
      let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
      let mode: String = store
          .with_connection(|c| c.query_row("PRAGMA journal_mode", [], |r| r.get(0)))
          .unwrap();
      assert_eq!(mode, "wal");
  }

  #[test]
  fn queue_add_then_status_update_completes() {
      let dir = tempfile::tempdir().unwrap();
      let store = SqliteStore::open_at(dir.path().join("h.db")).unwrap();
      let queue = store.open_queue().unwrap(); // must not deadlock with store's mutex
      let job = queue.add_job(&crate::QueueJob::new("https://e.test/f", "/tmp")).unwrap();
      queue.update_job_status(job.id, "running", None, None, None).unwrap();
      assert_eq!(queue.get_job(job.id).unwrap().unwrap().status, "running");
  }
  ```
  The pointer-identity assertion is behavioral proof of reuse; the queue test proves the re-entrancy hazard is absent. `prepare_cached` is verified by code review plus the existing record/recent and golden-schema behavior tests — statement reuse is not directly observable without private cache introspection, so no fake test is added for it.
- [ ] **Run and verify FAILS for the expected reason** — `cargo test -p paradl-store with_connection_reuses_one_connection -- --nocapture`. Expected RED: `E0609` (`no field conn on SqliteStore`) / `E0308` (`expected Arc<Mutex<Connection>>, found Connection` at `Queue::new`) — the new field and constructor signature do not exist yet. The existing `guard_initializes_once_per_physical_file` still passes at this point, confirming the failure is specifically the missing reuse, not a schema regression.
- [ ] **Implement minimal code** across `store.rs`, `queue.rs`, and the `lib.rs` trait doc exactly as specified in Interfaces. Keep all SQL strings byte-identical (schema DDL is frozen by the parity contract).
- [ ] **Run test, verify PASS** — `cargo test -p paradl-store with_connection_reuses_one_connection`, `... reused_connection_is_in_wal_mode`, `... queue_add_then_status_update_completes`; all GREEN.
- [ ] **Run the crate/full suite** — `cargo test -p paradl-store` (includes `tests/golden_schema.rs`), then `cargo test -p paradl-resilience` (queue consumers), then `cargo clippy --all -- -D warnings`.
- [ ] **Commit** — `git add rust/crates/paradl-store/src/store.rs rust/crates/paradl-store/src/queue.rs rust/crates/paradl-store/src/lib.rs && git commit -m "perf(store): reuse one Mutex<Connection> with cached statements"`.

## Task A4: Hoist the BSD checksum regex into a LazyLock static

**Files:**
- Modify `rust/crates/paradl-postproc/src/checksum.rs` — imports (~13-26), `parse_checksum_text` regex construction (~261-264), tests module (~339-351).
- Test `rust/crates/paradl-postproc/src/checksum.rs` — add the test to the existing `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: `use regex::Regex;` (`checksum.rs:22`), the per-call `Regex::new(r"(?i)^(MD5|SHA1|SHA256|SHA-256|SHA512|SHA-512|BLAKE2b)\s*\(([^)]+)\)\s*=\s*([0-9a-fA-F]+)$").expect("static BSD checksum regex")` (`checksum.rs:261-264`), `pub fn parse_checksum_text(text: &str, target_filename: Option<&str>) -> Option<(Algorithm, String)>` (`checksum.rs:248`).
- Produces: `use std::sync::LazyLock;` and `static BSD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(MD5|SHA1|SHA256|SHA-256|SHA512|SHA-512|BLAKE2b)\s*\(([^)]+)\)\s*=\s*([0-9a-fA-F]+)$").expect("static BSD checksum regex"));` at module scope. In `parse_checksum_text`, replace the local construction with `let bsd = &*BSD_RE;` so the loop body (`bsd.captures(line)`) is unchanged. No new Cargo dependency: `LazyLock` is in std (rust-version 1.98).

Steps:
- [ ] **Write the failing test** in `checksum.rs`'s tests module:
  ```rust
  #[test]
  fn bsd_regex_static_still_parses_all_variants() {
      let a: &Regex = &BSD_RE;
      let b: &Regex = &BSD_RE;
      assert!(std::ptr::eq(a, b), "BSD_RE must be one shared static, not per-call");
      for (name, hex, expected) in [
          ("MD5",     "0".repeat(32),  Algorithm::Md5),
          ("SHA1",    "1".repeat(40),  Algorithm::Sha1),
          ("SHA256",  "2".repeat(64),  Algorithm::Sha256),
          ("SHA-256", "3".repeat(64),  Algorithm::Sha256),
          ("SHA512",  "4".repeat(128), Algorithm::Sha512),
          ("SHA-512", "5".repeat(128), Algorithm::Sha512),
          ("BLAKE2b", "6".repeat(64),  Algorithm::Blake2b),
      ] {
          let text = format!("{name} (a.bin) = {hex}\n");
          let (algo, digest) = parse_checksum_text(&text, Some("a.bin")).unwrap();
          assert_eq!(algo, expected, "algorithm for {name}");
          assert_eq!(digest, hex);
      }
  }
  ```
  The `ptr::eq` line is a structural assertion that the regex is a single initialize-once static; parsing all seven spellings is the behavioral assertion that hoisting did not change the accepted grammar.
- [ ] **Run and verify FAILS for the expected reason** — `cargo test -p paradl-postproc bsd_regex_static_still_parses_all_variants -- --nocapture`. Expected RED: `E0425`/`E0432` (`cannot find value BSD_RE in this scope`) — the static does not exist yet. Regex compile count is not directly observable at runtime, so honest RED is the absent static plus the parse assertions; no timing-based test is used.
- [ ] **Implement minimal code** in `rust/crates/paradl-postproc/src/checksum.rs`: add the `LazyLock` import and `static BSD_RE`, replace the local `Regex::new(...)` block with `let bsd = &*BSD_RE;`. Keep the regex string byte-identical.
- [ ] **Run test, verify PASS** — `cargo test -p paradl-postproc bsd_regex_static_still_parses_all_variants -- --nocapture`; GREEN, all seven variants parse as before.
- [ ] **Run the crate/full suite** — `cargo test -p paradl-postproc`, then `cargo clippy --all -- -D warnings`.
- [ ] **Commit** — `git add rust/crates/paradl-postproc/src/checksum.rs && git commit -m "perf(postproc): hoist BSD checksum regex into LazyLock static"`.

## Task A5: Borrow HeaderMap on the same-host scope path

**Files:**
- Modify `rust/crates/paradl-engine/src/probe.rs` — imports (~10-20), `scope_headers` (~106-120), new `#[cfg(test)] mod tests` at EOF.
- Modify `rust/crates/paradl-engine/src/download.rs` — two call sites (~462 and ~920) to pass `Cow::into_owned()` where a `HeaderMap` by value is required.
- Test `rust/crates/paradl-engine/src/probe.rs` — new inline test module.

**Interfaces:**
- Consumes: `pub fn scope_headers(headers: &HeaderMap, allowed_host: Option<&str>, target_host: Option<&str>) -> HeaderMap` (`probe.rs:106`); call site `shared.client.get(uri.clone()).headers(headers)` (`download.rs:462-463`); call site `probe::probe(client, candidate.clone(), scoped, cookie)` with `probe(..., headers: HeaderMap, ...)` (`download.rs:920-921`, `probe.rs:145-150`); public re-export `pub use probe::{..., scope_headers}` (`lib.rs:66`).
- Produces:
  ```rust
  use std::borrow::Cow;
  pub fn scope_headers<'a>(
      headers: &'a HeaderMap,
      allowed_host: Option<&str>,
      target_host: Option<&str>,
  ) -> Cow<'a, HeaderMap> {
      let same_host = matches!((allowed_host, target_host), (Some(a), Some(t)) if a.eq_ignore_ascii_case(t));
      if same_host {
          return Cow::Borrowed(headers);
      }
      let mut out = headers.clone();
      let _ = out.remove(reqwest::header::AUTHORIZATION);
      let _ = out.remove(reqwest::header::COOKIE);
      let _ = out.remove(reqwest::header::PROXY_AUTHORIZATION);
      Cow::Owned(out)
  }
  ```
  Call sites become `.headers(headers.into_owned())` and `probe::probe(client, candidate.clone(), scoped.into_owned(), cookie)`. The public re-export in `lib.rs` needs no change (same item name).

Steps:
- [ ] **Write the failing test** at the bottom of `probe.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use std::borrow::Cow;
      use reqwest::header::{AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION};

      #[test]
      fn scope_headers_borrows_same_host_and_owns_cross_host() {
          let mut headers = HeaderMap::new();
          headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
          headers.insert(COOKIE, HeaderValue::from_static("sid=1"));
          headers.insert(PROXY_AUTHORIZATION, HeaderValue::from_static("Basic x"));
          headers.insert("x-trace", HeaderValue::from_static("t"));

          let same = scope_headers(&headers, Some("Example.COM"), Some("example.com"));
          assert!(matches!(same, Cow::Borrowed(_)), "same host (case-insensitive) must not clone");
          assert_eq!(same.get(AUTHORIZATION).unwrap(), "Bearer secret");

          let cross = scope_headers(&headers, Some("a.example"), Some("b.example"));
          assert!(matches!(cross, Cow::Owned(_)), "cross host must clone and strip");
          for sensitive in [AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION] {
              assert!(cross.get(sensitive).is_none(), "sensitive header leaked cross-host");
          }
          assert_eq!(cross.get("x-trace").unwrap(), "t");
          assert!(headers.get(AUTHORIZATION).is_some(), "caller's map must be untouched");

          // No host on either side keeps the existing (stripping) behavior.
          let unknown = scope_headers(&headers, None, None);
          assert!(matches!(unknown, Cow::Owned(_)));
          assert!(unknown.get(AUTHORIZATION).is_none());
      }
  }
  ```
- [ ] **Run and verify FAILS for the expected reason** — `cargo test -p paradl-engine scope_headers_borrows_same_host_and_owns_cross_host -- --nocapture`. Expected RED: `E0308` (`expected HeaderMap, found Cow<'_, HeaderMap>` at `matches!` / return type) — `scope_headers` still returns an owned `HeaderMap`. If only the signature is changed first, the two `download.rs` call sites fail to compile (`E0308`), which is the companion RED for the call-site updates.
- [ ] **Implement minimal code** in `rust/crates/paradl-engine/src/probe.rs` and `download.rs` exactly as in Interfaces. Do not touch `paradl-rs`/`rapids`: they carry their own `scope_headers` copies (slated for deletion in Workstream B) and never import this function.
- [ ] **Run test, verify PASS** — `cargo test -p paradl-engine scope_headers_borrows_same_host_and_owns_cross_host -- --nocapture`; GREEN, with `Borrowed` only on the same-host branch.
- [ ] **Run the crate/full suite** — `cargo test -p paradl-engine` (includes cross-host credential goldens), then `cargo clippy --all -- -D warnings`.
- [ ] **Commit** — `git add rust/crates/paradl-engine/src/probe.rs rust/crates/paradl-engine/src/download.rs && git commit -m "perf(engine): borrow HeaderMap on same-host scope path"`.

---

## Workstream completion gate

After A1–A5 are merged in order: `cargo test -p paradl-engine`, `cargo test -p paradl-store`, `cargo test -p paradl-postproc`, `cargo test -p paradl-resilience`, `cargo clippy --all -- -D warnings`, and `cargo doc --no-deps` must all be green; sidecar and schema golden fixtures must be unmodified (`git diff -- rust/crates/paradl-engine/tests rust/crates/paradl-store/tests/golden_schema.rs` is empty apart from the intentional A1 assertion in `src/tests.rs`). Do not start Workstream C until this gate passes.
