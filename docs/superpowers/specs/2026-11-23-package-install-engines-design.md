# Restore `paradl pip` / `paradl brew` install engines

**Date:** 2026-11-23
**Status:** Design proposed — approach 1 (full parity port) chosen by lead

## Intent and why

`paradl pip install` and `paradl brew install` used to accelerate package
installation by downloading artifacts with 16 parallel channels into a cache,
then invoking the real package manager to install from that cache. The W5-1
cutover deleted the Python engines that did this (`engines/pypi.py`,
`engines/brew.py`); the Rust rewrite ported only the resolution half and left
the CLI handlers as success-printing stubs. Those stubs were recently changed
to fail honestly, but the capability itself is gone.

Goal: restore the original "prefetch into cache, then install offline" pipeline
in Rust so these two commands do real work again, with honest results and
hermetic tests.

## Goals

1. `paradl brew install <formula>` resolves the bottle, prefetches it and any
   uninstalled dependency bottles into Homebrew's cache with 16 channels,
   verifies digests, then runs `brew install` non-interactively.
2. `paradl pip install <pkg>` resolves the wheel, prefetches it and its
   transitive dependency wheels into a temporary wheelhouse with 16 channels,
   then runs `uv pip install --no-index --find-links <wheelhouse>` (fallback
   `python -m pip install`).
3. `pip upgrade`, `brew update` and `brew upgrade` retain their documented
   meaning and map real exit codes.
4. Every path emits exactly one terminal event (`result` or `error`) with the
   real outcome; no false success.
5. Tests never invoke real `uv`, `pip` or `brew`.

## Non-goals

- No changes to the existing `paradl install <owner/repo>` GitHub release path.
- No changes to media routes or the download engine internals.
- No new CLI flags, and no auto-installation of `uv`/`brew`/Python.
- No fix for the download-only `pip:<pkg>` / `brew:<formula>` target shortcuts
  (they are classified as `Site` and still fail in the engine target path —
  separate follow-up).
- No vendoring of Python packaging libraries into the Rust binary.

## Constraints

- Reuse existing crates: `paradl-net` site resolvers (`BrewResolver`,
  `PyPIResolver`), `paradl-engine` (`NdEngine`, `Target`), `paradl-postproc`
  digest helpers, `paradl-events` single emitter, `paradl-core` errors/exit codes.
- Preserve byte-exact event shapes and the `0/1/2/3/4/130` exit-code map.
- `App::new_offline()` must fail explicitly and spawn nothing.
- Temp wheelhouses are RAII-cleaned; Homebrew cache files are deliberately
  kept (that is the point of the prefetch).
- Injectable process runner so tests can fake `uv`/`brew`/`python`.
- TDD for each behavioral slice; full workspace suite green before push.

## Approaches considered

| Approach | Trade-off | Verdict |
|---|---|---|
| Full parity port (resolve → prefetch → offline install) | Largest scope, but restores the acceleration that is the reason these commands exist | **Chosen** |
| Thin delegation to `uv`/`brew` | Small, but discards parallel prefetch; commands become pointless wrappers | Rejected |
| Prefetch into cache only, leave install to user | Avoids process spawning, but loses the one-command workflow | Rejected |

## Design

### 1. New crate `paradl-package`

Add `rust/crates/paradl-package/` to the workspace (`members` is `crates/*`;
also add it to `default-members`). It exposes:

- `CommandSpec { program, args, env, cwd }`, `ProcessOutcome { status, stdout,
  stderr }`.
- `trait ProcessRunner { fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome,
  paradl_core::Error>; }`.
- `SystemProcessRunner` using `std::process::Command` with captured output.
- `async fn brew_install(...)` and `async fn pip_install(...)`, each taking the
  resolved HTTP client config, a runner reference, connection count and an
  optional progress sink, returning `InstallOutcome { success, file_path,
  error, exit_code }`.

`App` gains a `package_runner: Arc<dyn ProcessRunner>` field defaulting to
`SystemProcessRunner`, plus `App::with_package_runner(...)` for tests. Update
all four constructors. `App` calls the async functions through its existing
tokio runtime, as `install_release` already does.

### 2. Brew install flow

1. Reject offline mode with an explicit error.
2. Runner: `brew --cache <formula>` → cache path (also proves `brew` exists;
   missing binary maps to `MissingDependency`). Extract the bottle tag
   (e.g. `arm64_sequoia`) from the cache filename.
3. `BrewResolver::new(...).with_detected_tag(tag).resolve("brew:<formula>", ctx)`
   → asset with URL, filename and SHA-256 digest.
4. Download with `NdEngine` into the cache path: `connections = 16`, checksum
   from the asset digest, headers from the asset. Fail the command if the
   download fails or verification fails.
5. Determine uninstalled dependencies via `brew list --formula` (runner) and
   the formula metadata from the resolver. Add one additive method
   `BrewResolver::formula_dependencies(formula, ctx) -> Vec<String>` backed by
   the existing formulae.brew.sh API call; no change to `ResolvedAsset`.
   For each missing dependency, resolve + download its bottle into its own
   cache path. Best-effort: a failed dependency prefetch is logged, not fatal
   (mirrors the old Python behavior).
6. Runner: `brew install <formula>` with `NONINTERACTIVE=1` and
   `HOMEBREW_NO_AUTO_UPDATE=1`. Status 0 → `result.success = true` with
   `file_path = cache path`; non-zero → `Error::failure` with the captured
   stderr, exit code `Failure`.
7. `brew update` → `brew update` with `NONINTERACTIVE=1`; `brew upgrade` →
   `brew upgrade`. Both map the real status.

### 3. Pip install flow

1. Reject offline mode. Detect the installer: `uv --version` via runner; if
   absent, use `<VIRTUAL_ENV>/bin/python` (when set) or `python3` with `-m pip`.
   Missing both → `MissingDependency`.
2. `PyPIResolver::resolve("pip:<pkg>", ctx)` → wheel URL, filename and digest.
   Download into an RAII temp wheelhouse with 16 channels.
3. Extract `Requires-Dist` from the downloaded wheel with the existing
   `extract_wheel_dependencies` helper in `pypi.rs` (make it public if needed).
   Optionally merge pinned requirements from `uv pip compile` when `uv` is
   present; failure of this optional step is non-fatal.
4. BFS prefetch: for each unique, not-yet-installed requirement, resolve via
   `PyPIResolver` (it already handles PEP 440 specifiers), download the wheel
   into the wheelhouse, extract its dependencies and enqueue them. Track
   visited names to terminate. `uv pip list` / `pip list` via runner seeds the
   already-installed set (best-effort). A dependency that cannot be resolved
   is skipped, not fatal.
5. Runner: `uv pip install --no-index --find-links <wheelhouse> [--upgrade]
   [--python <venv python>] <pkg>` (or `python -m pip install ...`). Status 0 →
   `result.success = true`; non-zero → `Error::failure` with captured stderr.
6. The temp wheelhouse is dropped after the command, so `file_path` is `None`.

### 4. Event and exit-code contract

- Success: `Event::Result` with `subcommand: "pip" | "brew"`, real
  `success`, `exit_code` derived from the runner status, and no leak of
  stdout into the emitter (stdout stays on the process output or is discarded).
- Failure: `Event::Error` with the mapped exit code — `MissingDependency`
  for absent `uv`/`brew`/`pip`, `Failure` for download or installer failure.
- `App::handle` stays the only dispatch point; no new CLI tokens.

### 5. Tests

- New crate: unit tests with the existing fake `SiteHttp` resolvers plus a
  scripted `FakeProcessRunner` asserting the exact command sequence (`brew
  --cache`, `brew list`, `brew install`; `uv --version`, `uv pip list`, `uv pip
  install --no-index --find-links ...`).
- `paradl-app`: dispatch tests using `App::with_package_runner` that assert
  prefetch-then-install ordering and the emitted result/error event. Existing
  offline C23/C24 golden lines (now errors) stay unchanged.
- No test contacts PyPI, formulae.brew.sh or a real package manager.

### 6. Build order and verification

1. Crate skeleton + process runner + brew install; tests; then pip install;
   tests.
2. `cargo test --workspace` and `cargo test -p paradl-app --features media`.
3. Rebuild release and reinstall `~/.local/bin/paradl`.
4. Manual smoke (outside tests, with cleanup): prefetch + install a tiny
   Homebrew formula then `brew uninstall` it; install a tiny package into a
   temp venv then delete the venv. Report real output, or skip with a stated
   reason if the host blocks it.
5. Commit and push to `main`; working tree clean.

## What is left to the builder

- Exact command-line argument order for `uv pip install` and `brew install`.
- Whether to expose `formula_dependencies` on `BrewResolver` or read deps via
  `brew deps` (lead prefers the resolver method; builder may choose `brew deps`
  if it avoids an extra HTTP call, and must record the choice).
- Module/crate naming (`paradl-package` proposed).
