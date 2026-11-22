# paradl-package

Package-manager install engines for the native `paradl` binary. This crate turns
`paradl pip install` and `paradl brew install` into real installs: it prefetches
artifacts with the parallel download engine, verifies them, and then invokes the
real package manager to install from the local cache.

## Pipeline

```
target → site resolver → parallel prefetch → digest verification → installer
```

- **pip**: resolve the root wheel through `PyPIResolver`, collect `Requires-Dist`
  from PyPI JSON, BFS-prefetch every missing wheel into a temporary wheelhouse,
  then run `uv pip install --no-index --find-links <wheelhouse>` (fallback
  `python -m pip install`). `pip upgrade` re-resolves the requested package and
  passes `--upgrade`. An active `VIRTUAL_ENV` is honoured.
- **brew**: resolve the bottle through `BrewResolver` using the tag from
  `brew --cache`, prefetch the bottle and uninstalled dependency bottles into
  Homebrew's download cache, then run `NONINTERACTIVE=1 HOMEBREW_NO_AUTO_UPDATE=1
  brew install`. `brew update` and no-formula `brew upgrade` run directly without
  resolving a bottle.

A dependency that cannot be resolved is skipped; the root package and any digest
failure stay fatal (exit code 3).

## Public surface

- `brew_install(ctx, formula, action)` / `pip_install(ctx, package, action)` —
  the two async engines, returning `InstallOutcome { success, file_path, error,
  exit_code }`.
- `brew_formula_dependencies(http, formula)` — best-effort formula metadata read.
- `PackageContext` — the config plus three injectable seams:
  - `ProcessRunner` / `SystemProcessRunner` — spawns `uv`, `pip`, `brew`;
  - `SiteHttp` (from `paradl-net`) — resolver REST calls;
  - `ArtifactDownloader` / `EngineArtifactDownloader` — prefetch transfers.
- `CommandSpec`, `ProcessOutcome`, `DownloadOutcome` — data types for those seams.
- `BrewAction` / `PipAction` — local action enums; `paradl-app` maps the CLI ones.

## Example

```rust
use std::sync::Arc;
use paradl_package::{brew_install, BrewAction, PackageContext};

let ctx = PackageContext {
    runner: Arc::new(paradl_package::SystemProcessRunner),
    http: Arc::new(paradl_net::HttpSiteHttp::new(
        paradl_net::HttpClient::new(paradl_net::HttpClientConfig::default())?,
    )),
    downloader: Arc::new(paradl_package::EngineArtifactDownloader::default()),
    proxy: None,
    doh_endpoint: None,
    connections: 16,
};
let outcome = brew_install(&ctx, "wget", BrewAction::Install).await?;
assert!(outcome.success);
```

Tests substitute all three seams with scripted implementations, so no test
contacts PyPI/formulae.brew.sh or invokes a real package manager.

## Exit codes

Inherited from `paradl_core::ExitCode`: `0` success, `1` installer/download
failure, `2` usage, `3` digest verification failure, `4` missing `uv`/`brew`/Python.
