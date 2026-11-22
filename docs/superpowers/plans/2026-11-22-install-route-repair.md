# Install Route Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `paradl install <owner/repo>` actually install a release binary, classify bare `owner/repo` as GitHub, and repair the stale PATH wrapper on this machine.

**Architecture:** Reuse `paradl-net`'s `GithubResolver` + `HttpSiteHttp` + `EnvSecretProvider` for release resolution, `paradl-engine` for verified download, `paradl-postproc::archive::extract` for safe extraction, and `paradl-events` for the single emitter. `paradl-app::install` becomes the orchestrator with pure, testable helpers for target normalization and binary selection.

**Tech Stack:** Rust 2024, reqwest-backed site HTTP seam, Tokio runtime, RAII temp dir, POSIX permissions.

**Spec:** docs/superpowers/specs/2026-11-22-install-route-repair-design.md

## Global Constraints

- No false success: every failure emits `Event::Error` with the mapped exit code.
- `paradl-events` is the only JSONL emitter; event shapes unchanged.
- Offline mode must fail explicitly, never attempt network resolution.
- Temp files removed on every path (RAII).
- Full suite, clippy `-D warnings`, and docs green before push to `main`.
- Local rustfmt drift is environmental; do not mass-reformat sources.

## Review Focus

- Bare `owner/repo` must classify as GitHub but must not swallow URLs or paths (`github.com/owner`, `foo/bar.py`).
- Resolver asset selection must not be bypassed; the first non-empty resolved URL is used.
- Download must verify `asset.digest` when present; sink failures propagate as errors.
- Extraction must use the validated postproc helper; selected binary must be the expected name, not an arbitrary script.
- PATH wrapper must resolve to the native binary from a fresh shell.

---

### Task 1: Classify bare `owner/repo` as GitHub

**Files:**
- Modify `rust/crates/paradl-net/src/routing.rs`
- Modify `rust/crates/paradl-net/src/sites/github.rs`
- Test: existing `paradl-net` tests / `#[cfg(test)]` modules in both files

**Interfaces:**
- Produces: `classify_target("cli/cli", ...)` → `RouteDecision::Site { TargetType::Github }`; `GithubTarget::parse("cli/cli")` → `GithubTarget::Repo { owner: "cli", repo: "cli" }`.
- Consumes: nothing new.

- [ ] **Step 1: write failing tests** — `bare_owner_repo_is_github()` in `routing.rs` tests asserting `matches!(classify_target("cli/cli", None, &PureClaimsChecker), RouteDecision::Site{..})` and `!classify_target("github.com/cli/cli", ...)` being misclassified as bare; `bare_repo_parses()` in `github.rs` tests asserting `GithubTarget::parse("cli/cli")` owner/repo; assert dotted `foo/bar.py` is NOT bare-repo.
- [ ] **Step 2: verify RED** — `cargo test -p paradl-net bare_owner_repo` → assertion failures.
- [ ] **Step 3: implement** — add no-dot bare regex branches per the design's placement rulings; do not change existing URL/gist/asset branches.
- [ ] **Step 4: verify GREEN** — targeted tests, then `cargo test -p paradl-net`.
- [ ] **Step 5: full suite** — `cargo test --all -- --skip full_non_media_gate_is_clean`, clippy.
- [ ] **Step 6: commit** — `git commit -m "fix(routing): classify bare owner/repo as GitHub"`.

### Task 2: Implement the real install route

**Files:**
- Modify `rust/crates/paradl-app/Cargo.toml` (add `tempfile` if absent)
- Modify `rust/crates/paradl-app/src/app.rs`
- Test: `#[cfg(test)] mod install_tests` in `app.rs`

**Interfaces:**
- Produces: `normalize_install_target`, `find_install_binary`, `expand_bin_dir`; `install(&mut self, ...)` resolves, downloads, extracts, installs.
- Consumes: `GithubResolver`, `HttpSiteHttp`, `EnvSecretProvider`, `ResolveContext`, `NdEngine`, `Target`, `Hash::parse`, `archive::extract`.

- [ ] **Step 1: write failing tests** — `normalize_install_target("cli/cli") == "gh:cli/cli"` and leaves `gh:cli/cli` unchanged; `find_install_binary` prefers exact `gh`/`gh.exe` over `README.md`, falls back to repo-name match (`uv`), returns `None` for no candidates; `expand_bin_dir("~/bin")` uses home dir. Add a test that offline `install` returns a non-Ok exit code and emits an error event (use `App::new_offline` style constructor from existing tests).
- [ ] **Step 2: verify RED** — `cargo test -p paradl-app --lib install_tests` → missing function compile errors.
- [ ] **Step 3: implement helpers and route** exactly per design; reuse `AppProgressSink` for download telemetry; emit success only after the binary is copied.
- [ ] **Step 4: verify GREEN** — targeted tests, then `cargo test -p paradl-app`.
- [ ] **Step 5: full suite** — `cargo test --all`, clippy, docs.
- [ ] **Step 6: commit** — `git commit -m "feat(install): resolve, download, verify and install release binaries"`.

### Task 3: Repair PATH and verify end to end

**Files:** none in repo (local environment repair), plus verification notes in the report.

- [ ] **Step 1:** replace `~/.local/bin/paradl` bash shim with the native binary from `main`; point `parallel-dl` at it; verify `which -a paradl` and `paradl --version` from a fresh shell (no traceback).
- [ ] **Step 2:** run `paradl info cli/cli --json` and assert `target_type: "github"`.
- [ ] **Step 3:** run `paradl install cli/cli -y -c 16`; assert exit 0 and `~/.local/bin/gh --version` reports a real release.
- [ ] **Step 4:** remove any downloaded temp archives/verification artifacts from the test.
- [ ] **Step 5:** report any mismatch explicitly; push commits to `main`.

## Final gate

- [ ] `cargo test --all -- --skip full_non_media_gate_is_clean` → 0 failures.
- [ ] `cargo clippy --all -- -D warnings` → 0 errors.
- [ ] `paradl --version`, `paradl info cli/cli`, `paradl install cli/cli` all verified.
