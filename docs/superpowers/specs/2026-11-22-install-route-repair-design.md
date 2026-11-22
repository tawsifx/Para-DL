# Repair PATH wrapper and make `install` real

**Date:** 2026-11-22
**Status:** Implemented. Superseded for pip/brew by
`2026-11-23-package-install-engines-design.md` — the "no pip/brew install
implementation" non-goal below no longer holds.

## Intent and why

Two user-visible defects: `paradl` on PATH crashes because `~/.local/bin/paradl`
is a stale bash shim into the deleted `paradl.cli:main`, and `paradl install
<owner/repo>` emits `success` while downloading nothing. The workaround (manual
direct download of a release zip) proves the engine works.

Goals: (1) repair PATH so `paradl --version` resolves to the native binary;
(2) make `install cli/cli` resolve the latest GitHub release, download through
the existing engine with checksum verification, extract and install `gh` into
the configured bin dir; (3) classify bare `owner/repo` as GitHub, not search;
(4) never report success without a real install.

## Non-goals

- No pip/brew install implementation (those routes stay as-is).
- No changes to `GithubResolver` selection/scoring logic — reuse it as-is.
- No change to the media/aria2 paths.
- No new CLI flags.

## Constraints

- Reuse existing crates: `paradl-net` site resolvers, `paradl-engine` download,
  `paradl-postproc::archive::extract`, `paradl-events` single emitter.
- Byte-exact event shapes unchanged; failures emit `Event::Error` and the
  mapped exit code, never `success`.
- `install` must work offline-safe: in `offline` mode return explicit failure.
- Temp files cleaned up on every path (RAII temp dir).
- TDD for every behavioral change; full suite green before push.

## Design

### 1. PATH repair (local environment, outside repo)

Replace `~/.local/bin/paradl` (bash shim) with the native binary built from
`main`; point `~/.local/bin/parallel-dl` at it. Verify `paradl --version` from
a fresh shell and `which -a paradl`. Leave the stale `.venv/bin/paradl` scripts
in place but unreferenced; note them in the report.

### 2. Classifier: bare `owner/repo`

- `paradl-net/src/routing.rs::shortcut_type`: recognise bare shorthand
  `^[A-Za-z0-9_]+/[A-Za-z0-9_]+$` (no dots, no scheme, no traversal) *after*
  the existing `github.com/` checks, as `TargetType::Github`. Dotted repo names
  remain available via the explicit `gh:` / `github:` prefixes.
- `paradl-net/src/sites/github.rs::GithubTarget::parse`: the same bare-repo
  shape maps to `GithubTarget::Repo`; this branch goes near the end, after
  `gh:`/`github:` prefixes, gist, asset, blob, tree and URL cases, so a plain
  `github.com/owner` URL is still handled by the existing URL patterns.
- Tests pin `classify_target("cli/cli")`, `paradl info cli/cli` behavior, and
  the pure parser branch.

### 3. Real install route

`paradl-app/src/app.rs`:

- New pure helpers: `normalize_install_target(&str) -> String` (bare
  `owner/repo` → `gh:owner/repo`), `find_install_binary(&[PathBuf]) ->
  Option<&Path>` (prefer exact `gh`/`gh.exe`, then basename match), and
  `expand_bin_dir(&str) -> PathBuf` (`~` handling).
- `fn install(&mut self, command, json)`:
  1. If `self.offline`, emit error `"install requires network"` and return
     `MissingDependency`/`Failure`.
  2. Build `HttpClient` + `HttpSiteHttp::new`, `GithubResolver::new(http,
     EnvSecretProvider::from_process())`, `ResolveContext` with proxy/DoH from
     config and a reasonable timeout.
  3. `runtime().block_on(resolver.resolve(&target, &ctx))`; reject empty URL
     lists; take the first asset with a non-empty URL (resolver already scores
     platform assets).
  4. Create a RAII temp dir; build `EngineTarget` with the asset URL, headers
     from `asset.headers`, `checksum: asset.digest.and_then(Hash::parse)`,
     `watchdog: Some(watchdog_config_for(...))`, `connections: 16`.
  5. `NdEngine::plan` + `download` with a sink that forwards watchdog events
     through `AppProgressSink` (reuse existing JSON/plain renderer). Require
     `result.success`.
  6. `paradl_postproc::archive::extract(&archive, temp, false)`; locate the
     binary: prefer an exact `gh`/`gh.exe`, then a file whose basename equals
     the repo name (or `repo.exe`); otherwise fail explicitly. The installed
     name is the discovered file's own basename (so `uv` stays `uv`, `gh` stays
     `gh`). Copy to `bin_dir/<basename>` with mode `0755` on Unix; emit `Result`
     success with the installed path. Any failure emits `Event::Error` and the
     mapped exit code.
  7. Temp files removed by RAII drop on all paths.

### 4. Verification

- TDD for helpers/classifier/parser; `cargo test --all`, clippy, docs.
- Real run: `paradl install cli/cli -y -c 16` → `gh` in `~/.local/bin`,
  `gh --version` works; `paradl info cli/cli` → `github`; PATH `paradl --version`.
- Report any residual mismatch (e.g. asset naming) explicitly.
