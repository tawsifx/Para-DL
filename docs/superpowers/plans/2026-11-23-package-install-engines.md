# Package Install Engines Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore `paradl pip install` / `paradl brew install` as real install engines that prefetch artifacts with 16 parallel channels into a cache, then invoke the real package manager offline.

**Architecture:** A new `paradl-package` crate owns the orchestration: resolve through existing `paradl-net` site resolvers, download through `paradl-engine`, verify digests, then spawn `uv`/`brew` through an injectable `ProcessRunner`. `paradl-app` wires it into the existing `pip`/`brew` handlers and emits the normal single-emitter events.

**Tech Stack:** Rust 2024 / edition workspace 1.0.0, tokio, `paradl-net` (`SiteHttp`, `BrewResolver`, `PyPIResolver`, `ResolveContext`), `paradl-engine` (`NdEngine`, `Target`), `paradl-postproc` digest checks, `paradl-core` errors/exit codes.

**Spec:** `docs/superpowers/specs/2026-11-23-package-install-engines-design.md`

## Global Constraints

- Reuse existing crates; no new dependency on PyO3 or Python for non-media paths.
- Byte-exact event shapes and the `0/1/2/3/4/130` exit-code map stay unchanged.
- `App::new_offline()` must fail explicitly and spawn nothing.
- Homebrew cache files are kept; temp wheelhouses are RAII-cleaned.
- Tests never contact PyPI/formulae.brew.sh and never invoke real `uv`, `pip` or `brew`.
- TDD for every behavioral task; full workspace suite green before push.
- Digest verification failure maps to `Error::verification_failed` (exit `3`).
- `pip:`/`brew:` download-only target shortcuts and `PipAction::Targets`/`BrewAction::Targets` stay out of scope (explicit error).

## Review Focus

- **Digest mismatch on a bottle or wheel** must produce exit 3 and must not run the installer — pinned by `brew_install_digest_mismatch_fails_with_verification` (Task 3) and `pip_install_digest_mismatch_is_verification_failed` (Task 5).
- **Missing `brew`/`uv`/`pip`** must produce exit 4 with an actionable message, never `success` — pinned by `brew_install_missing_brew_is_missing_dependency` (Task 3) and `pip_dispatch_missing_tool_is_missing_dependency` (Task 6).
- **`brew upgrade wget`** must forward the formula; upgrading everything is destructive — pinned by `brew_upgrade_forwards_formula` (Task 3).
- **`pip upgrade` on an already-installed package** must still prefetch the wheel and pass `--upgrade` — pinned by `pip_upgrade_re_resolves_installed_package` (Task 5).
- **Wrong Python ABI wheel** — the interpreter-version probe must feed `ResolveContext.py_version` — pinned by `pip_install_uses_detected_python_version` (Task 5).

---

### Task 1: `paradl-package` crate skeleton and process runner

**Files:**
- Create: `rust/crates/paradl-package/Cargo.toml`
- Create: `rust/crates/paradl-package/src/lib.rs`
- Create: `rust/crates/paradl-package/src/process.rs`
- Modify: `rust/Cargo.toml` (`default-members`)
- Test: `rust/crates/paradl-package/src/process.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces: `CommandSpec { program: String, args: Vec<String>, env: Vec<(String, String)>, cwd: Option<PathBuf> }`; `ProcessOutcome { status: i32, stdout: String, stderr: String }`; `trait ProcessRunner: Send + Sync { fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, paradl_core::Error>; }`; `SystemProcessRunner`; `PackageContext { runner: Arc<dyn ProcessRunner>, http: Arc<dyn SiteHttp>, proxy: Option<String>, doh_endpoint: Option<String>, connections: u32 }`; `InstallOutcome { success: bool, file_path: Option<PathBuf>, error: Option<String>, exit_code: ExitCode }`; local enums `BrewAction { Install, Upgrade, Update }` and `PipAction { Install, Upgrade }`.

- [ ] **Step 1: Write the failing tests**

`system_runner_captures_stdout_and_status`: run `SystemProcessRunner` with `program = "/bin/echo"`, `args = ["ok"]`; assert `status == 0` and `stdout.trim() == "ok"`.
`system_runner_missing_program_is_error`: run a nonexistent program; assert `Err(Error::MissingDependency(_))`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-package`
Expected: FAIL — crate/target does not exist yet.

- [ ] **Step 3: Implement the crate and runner**

Create the manifest with path deps `paradl-core`, `paradl-net`, `paradl-engine`, `paradl-postproc` and registry deps `tokio`, `async-trait`, `tempfile`, `url`, `serde_json`, `chrono` (all workspace versions where declared). Include everything now so later tasks only add modules. `SystemProcessRunner::run` uses `std::process::Command` with `envs`, `current_dir`, `output()`, maps spawn failure to `Error::missing_dependency`. Put the shared types in `lib.rs`, the runner in `process.rs`, re-export from `lib.rs`. Add `paradl-package` to `rust/Cargo.toml` `default-members`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-package`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add rust/Cargo.toml rust/crates/paradl-package
git commit -m "feat(package): add paradl-package crate and process runner"
```

### Task 2: App injection seam and honest offline/Targets errors

**Files:**
- Modify: `rust/crates/paradl-app/Cargo.toml` (add `paradl-package` path dep)
- Modify: `rust/crates/paradl-app/src/app.rs` (field, constructors, `package_context()`, test builder)
- Modify: `rust/crates/paradl-app/src/commands.rs` (`pip`, `brew`)
- Modify: `rust/crates/paradl-app/tests/dispatch.rs` (C23/C24 goldens, new test)
- Modify: `rust/crates/paradl-app/tests/characterization.rs` (C23/C24 golden lines)

**Interfaces:**
- Consumes: `PackageContext`, `ProcessRunner`, `SiteHttp` from Task 1.
- Produces: `App::with_package_context(runner: Arc<dyn ProcessRunner>, http: Arc<dyn SiteHttp>) -> Self`; private `App::package_context(&self) -> Result<PackageContext, Error>` (real HTTP from config when not injected). `pip`/`brew` now dispatch to Task 3/5 functions once available — in this task they emit offline/Targets errors only.

- [ ] **Step 1: Write the failing tests**

Update `dispatch.rs` CASES C23/C24 to `ExitCode::Failure` and `"\"event\": \"error\""`. Update `characterization.rs` goldens to:
`=== C23 pip [pip install requests] -> 1` / `{"event": "error", "timestamp": <TS>, "message": "pip install requires network"}` and the analogous C24 line. Add `targets_routes_are_explicit_errors`: `dispatch(&["pip", "pip:foo"])` and `dispatch(&["brew", "brew:foo"])` return non-Ok and emit `"event": "error"`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-app --test dispatch --test characterization`
Expected: FAIL — goldens still show "not implemented" / exit 4.

- [ ] **Step 3: Implement the seam and errors**

Add `package_runner`/`package_http` fields to `App`, set defaults in all four constructors, add `with_package_context` and `package_context`. In `pip`/`brew`: reject `self.offline` with `Error::failure("pip install requires network")` / `"brew install requires network"`; reject `PipAction::Targets`/`BrewAction::Targets` with `Error::usage` pointing at the install form. Leave `Install`/`Upgrade`/`Update` routes as `Error::missing_dependency("not yet wired")` for this task only.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-app --test dispatch --test characterization`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/paradl-app
git commit -m "feat(app): injectable package context and honest offline errors"
```

### Task 3: Brew install engine

**Files:**
- Create: `rust/crates/paradl-package/src/brew.rs`
- Modify: `rust/crates/paradl-package/src/lib.rs` (module + re-export)
- Test: `rust/crates/paradl-package/src/brew.rs` (`#[cfg(test)]` with scripted runner/fake `SiteHttp`)

**Interfaces:**
- Consumes: `PackageContext`, `InstallOutcome`, `BrewAction` from Task 1; `BrewResolver::with_detected_tag`, `TargetResolver::resolve`, `NdEngine`, `Target`, `parse_asset_digest`.
- Produces: `pub async fn brew_install(ctx: &PackageContext, formula: &str, action: BrewAction) -> Result<InstallOutcome, Error>`; `pub async fn brew_formula_dependencies(http: Arc<dyn SiteHttp>, formula: &str) -> Vec<String>` (additive helper, best-effort, returns empty on failure).

- [ ] **Step 1: Write the failing tests**

`brew_install_prefetches_bottle_then_pours`: scripted runner answers `brew --cache wget` with a cache path containing `arm64_sequoia`, `brew list --formula` with an empty list, `brew install wget` with status 0; fake `SiteHttp` resolves one bottle with a known digest. Assert the runner saw `brew --cache` → `brew list --formula` → `brew install wget` with `NONINTERACTIVE=1` and `HOMEBREW_NO_AUTO_UPDATE=1`, and `outcome.success == true`, `file_path == cache path`.
`brew_install_digest_mismatch_fails_with_verification`: make the downloaded bytes not match the asset digest; assert `Err(Error::VerificationFailed(_))` and that no install command was run.
`brew_upgrade_forwards_formula`: `brew_install(..., "wget", BrewAction::Upgrade)`; assert the runner saw `brew upgrade wget`.
`brew_install_missing_brew_is_missing_dependency`: runner fails `brew --cache`; assert `Err(Error::MissingDependency(_))`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-package brew_`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement the brew engine**

`brew --cache <formula>` → cache path and bottle tag; `BrewResolver::new(HttpSiteHttp::new(client), secrets).with_detected_tag(tag)`; resolve `brew:<formula>`; download to cache path with `connections = ctx.connections`, checksum from asset digest (use `parse_asset_digest` semantics); on engine verification failure return `Error::verification_failed`. Prefetch missing dependencies from `brew_formula_dependencies` (best-effort, skip failures). Run the final `brew` command per action with the env above; map status 0 to success, non-zero to `Error::failure(stderr)`; missing binary to `Error::missing_dependency`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-package brew_`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/paradl-package
git commit -m "feat(package): brew prefetch and pour engine"
```

### Task 4: Wire brew into the app handlers

**Files:**
- Modify: `rust/crates/paradl-app/src/commands.rs` (`brew`)
- Test: `rust/crates/paradl-app/tests/dispatch.rs`

**Interfaces:**
- Consumes: `brew_install`, `BrewAction`, `PackageContext` from Tasks 1–3; `result_line`, `Event::Result`.
- Produces: `paradl brew install|upgrade|update` emits a real `result`/`error` event.

- [ ] **Step 1: Write the failing tests**

`brew_dispatch_success_emits_result`: build `App::with_config(PDLConfig::default()).with_output(sink).with_package_context(fake_runner, fake_http)` (online, output captured); dispatch `["brew", "install", "wget", "--json"]`; assert exit 0, one `"event": "result"`, `success: true`, no `"event": "error"`. `brew_dispatch_runner_failure_emits_error`: scripted `brew install` status 1; assert exit 1 and `"event": "error"`.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-app --test dispatch brew_dispatch`
Expected: FAIL — handler still returns "not yet wired".

- [ ] **Step 3: Wire the handler**

Build `PackageContext` from `self.package_context()?`, map `BrewAction` → local `BrewAction`, call `brew_install` through `self.runtime().block_on(...)`, then publish `result_line("brew", formula, outcome.success, outcome.error)` or `self.fail(json, &error)`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-app --test dispatch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/paradl-app
git commit -m "feat(app): wire brew install engine into dispatch"
```

### Task 5: Pip wheelhouse engine

**Files:**
- Create: `rust/crates/paradl-package/src/pypi.rs`
- Modify: `rust/crates/paradl-package/src/lib.rs` (module + re-export)
- Test: `rust/crates/paradl-package/src/pypi.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `PackageContext`, `InstallOutcome`, `PipAction` from Task 1; `PyPIResolver`, `ResolveContext`, `extract_wheel_dependencies`, `NdEngine`.
- Produces: `pub async fn pip_install(ctx: &PackageContext, package: &str, action: PipAction) -> Result<InstallOutcome, Error>`.

- [ ] **Step 1: Write the failing tests**

`pip_install_prefetches_root_and_dependency_wheels`: fake `SiteHttp` serves a root wheel whose METADATA lists one dependency; scripted runner answers `uv --version`, `<python> -c ...` with `3.12`, `uv pip list` with empty list, `uv pip install` with status 0. Assert two wheel downloads happened, the final command contains `--no-index --find-links`, and `outcome.success == true`.
`pip_upgrade_re_resolves_installed_package`: `uv pip list` reports the package installed; assert the root wheel is still downloaded and the command contains `--upgrade`.
`pip_install_uses_detected_python_version`: assert `ResolveContext.py_version == Some((3, 12))` was used for scoring (fake HTTP records the request context or the test asserts via a resolver spy).
`pip_install_digest_mismatch_is_verification_failed`: assert `Err(Error::VerificationFailed(_))` and no install command.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-package pip_`
Expected: FAIL — function not defined.

- [ ] **Step 3: Implement the pip engine**

Detect `uv` via runner; else `<VIRTUAL_ENV>/bin/python` or `python3` with `-m pip`; missing both → `Error::missing_dependency`. Probe interpreter version via runner `python -c "import sys; print(...)"`, set `ResolveContext.py_version` (best-effort). Resolve root with `PyPIResolver`, download wheel into `tempfile::tempdir()` with checksum; BFS over `extract_wheel_dependencies` plus optional `uv pip compile`, skipping installed/visited packages; download each wheel into the wheelhouse. For `PipAction::Upgrade`, do not exclude the target package from resolution and add `--upgrade`. Final runner command uses `--no-index --find-links <wheelhouse>`; status 0 → success, non-zero → `Error::failure(stderr)`. Verification failure → `Error::verification_failed`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-package pip_`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/paradl-package
git commit -m "feat(package): pip wheelhouse and transitive prefetch engine"
```

### Task 6: Wire pip into the app handlers

**Files:**
- Modify: `rust/crates/paradl-app/src/commands.rs` (`pip`)
- Test: `rust/crates/paradl-app/tests/dispatch.rs`

**Interfaces:**
- Consumes: `pip_install`, `PipAction`, `PackageContext` from Tasks 1/5.
- Produces: `paradl pip install|upgrade` emits a real `result`/`error` event.

- [ ] **Step 1: Write the failing tests**

`pip_dispatch_success_emits_result`: injected fakes, dispatch `["pip", "install", "requests", "--json"]`; assert exit 0, one result event with `success: true`.
`pip_dispatch_missing_tool_is_missing_dependency`: scripted runner fails `uv --version` and python probe; assert exit 4 and `"missing dependency"` in the error event.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p paradl-app --test dispatch pip_dispatch`
Expected: FAIL — handler still returns "not yet wired".

- [ ] **Step 3: Wire the handler**

Same shape as Task 4: build `PackageContext`, map action, `block_on(pip_install(...))`, publish `result_line("pip", package, ...)` or `self.fail`.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p paradl-app --test dispatch`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add rust/crates/paradl-app
git commit -m "feat(app): wire pip install engine into dispatch"
```

### Task 7: Full verification, rebuild and push

**Files:** none (verification only)

- [ ] **Step 1: Run the complete suite**

Run: `cd rust && cargo test --workspace && cargo test -p paradl-app --features media`
Expected: all tests pass, zero failures.

- [ ] **Step 2: Build and install the release binary**

Run: `cd rust && cargo build --release --manifest-path Cargo.toml --bin paradl && install -m 0755 target/release/paradl /Users/tawsifx/.local/bin/paradl && paradl --version`
Expected: `parallel-dl (paradl) v1.0.0`.

- [ ] **Step 3: Manual smoke (with cleanup)**

Run a real `paradl brew install` of a small formula, then `brew uninstall` it, and a real `paradl pip install` of a tiny package into a temp venv, then delete the venv. Record actual output. If the host blocks either (network/permissions), report the reason instead of claiming success.

- [ ] **Step 4: Push**

Run: `git status --short && git push origin main`
Expected: clean tree, `HEAD == origin/main`.

- [ ] **Step 5: Report**

Summarize the real commands run, their exit codes, and the smoke-test evidence (or the stated reason it was skipped).
