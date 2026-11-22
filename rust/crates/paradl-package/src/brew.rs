//! Homebrew install engine: prefetch bottles into the cache, then pour them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use paradl_core::{Error, ExitCode};
use paradl_net::routing::{ResolveContext, ResolvedAsset, TargetResolver};
use paradl_net::sites::brew::BrewResolver;

use crate::{BrewAction, CommandSpec, InstallOutcome, PackageContext, ProcessOutcome};

const BREW: &str = "brew";
const API_TEMPLATE: &str = "https://formulae.brew.sh/api/formula/{formula}.json";

fn run(ctx: &PackageContext, args: &[&str]) -> Result<ProcessOutcome, Error> {
    ctx.runner.run(&CommandSpec {
        program: BREW.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![],
        cwd: None,
    })
}

fn cache_path_and_tag(ctx: &PackageContext, formula: &str) -> Result<(PathBuf, String), Error> {
    let outcome = ctx.runner.run(&CommandSpec {
        program: BREW.to_owned(),
        args: vec!["--cache".to_owned(), formula.to_owned()],
        env: vec![],
        cwd: None,
    })?;
    if outcome.status != 0 {
        return Err(Error::missing_dependency(format!(
            "brew --cache {formula} failed: {}",
            outcome.stderr.trim()
        )));
    }
    let path = PathBuf::from(outcome.stdout.trim());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim_end_matches(".bottle.tar.gz");
    let tag = name.rsplit('.').next().unwrap_or_default().to_owned();
    Ok((path, tag))
}

fn resolve_context(ctx: &PackageContext) -> ResolveContext {
    ResolveContext {
        proxy: ctx.proxy.clone(),
        doh_endpoint: ctx.doh_endpoint.clone(),
        timeout: Duration::from_secs(15),
        ..ResolveContext::default()
    }
}

fn resolver(ctx: &PackageContext, tag: &str) -> BrewResolver {
    BrewResolver::from_shared(Arc::clone(&ctx.http)).with_detected_tag(tag)
}

async fn download_asset(
    ctx: &PackageContext,
    asset: &ResolvedAsset,
    dest: &Path,
) -> Result<(), Error> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let outcome = ctx
        .downloader
        .download(
            &asset.url,
            dest,
            asset.digest.as_deref(),
            ctx.connections,
        )
        .await?;
    if outcome.exit_code == ExitCode::VerificationFailed {
        return Err(Error::verification_failed(format!(
            "bottle digest verification failed for {}",
            dest.display()
        )));
    }
    if !outcome.success {
        return Err(Error::failure(
            outcome
                .error
                .unwrap_or_else(|| format!("bottle download failed for {}", dest.display())),
        ));
    }
    Ok(())
}

/// Formula names a formula depends on, read from the formulae.brew.sh API.
/// Best-effort: any failure means no dependency prefetch, not a failed install.
pub async fn brew_formula_dependencies(http: Arc<dyn paradl_net::SiteHttp>, formula: &str) -> Vec<String> {
    let url = API_TEMPLATE.replace("{formula}", formula);
    let response = match http
        .fetch_json(&url, &HashMap::new(), Duration::from_secs(15))
        .await
    {
        Ok(response) if response.status == 200 => response,
        _ => return Vec::new(),
    };
    response
        .body
        .get("dependencies")
        .and_then(serde_json::Value::as_array)
        .map(|deps| {
            deps.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Prefetch the bottle (and missing dependency bottles) into Homebrew's cache,
/// then pour the formula non-interactively.
pub async fn brew_install(
    ctx: &PackageContext,
    formula: &str,
    action: BrewAction,
) -> Result<InstallOutcome, Error> {
    let (cache_path, tag) = cache_path_and_tag(ctx, formula)?;

    let assets = resolver(ctx, &tag)
        .resolve(&format!("brew:{formula}"), &resolve_context(ctx))
        .await?;
    let asset = assets
        .first()
        .ok_or_else(|| Error::failure(format!("no bottle available for {formula}")))?;
    download_asset(ctx, asset, &cache_path).await?;

    let installed = run(ctx, &["list", "--formula"])
        .map(|outcome| outcome.stdout)
        .unwrap_or_default();
    let deps = brew_formula_dependencies(Arc::clone(&ctx.http), formula).await;
    for dep in deps
        .into_iter()
        .filter(|dep| !installed.lines().any(|line| line.trim() == dep))
    {
        if let Ok((dep_cache, dep_tag)) = cache_path_and_tag(ctx, &dep)
            && let Ok(mut dep_assets) = resolver(ctx, &dep_tag)
                .resolve(&format!("brew:{dep}"), &resolve_context(ctx))
                .await
            && let Some(dep_asset) = dep_assets.drain(..).next()
            && download_asset(ctx, &dep_asset, &dep_cache).await.is_ok()
        {
            // best effort: a failing dependency prefetch never blocks the pour
        }
    }

    let args: Vec<&str> = match action {
        BrewAction::Install => vec!["install", formula],
        BrewAction::Upgrade => vec!["upgrade", formula],
        BrewAction::Update => vec!["update"],
    };
    let outcome = ctx.runner.run(&CommandSpec {
        program: BREW.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![
            ("NONINTERACTIVE".to_owned(), "1".to_owned()),
            ("HOMEBREW_NO_AUTO_UPDATE".to_owned(), "1".to_owned()),
        ],
        cwd: None,
    })?;
    if outcome.status != 0 {
        return Err(Error::failure(
            if outcome.stderr.trim().is_empty() {
                format!("brew {} failed with exit code {}", args.join(" "), outcome.status)
            } else {
                outcome.stderr.trim().to_owned()
            },
        ));
    }
    Ok(InstallOutcome {
        success: true,
        file_path: Some(cache_path),
        error: None,
        exit_code: ExitCode::Ok,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use paradl_core::{Error, ExitCode};
    use paradl_net::sites::{JsonResponse, RedirectInfo, SiteHttp};
    use paradl_net::Result as NetResult;

    use crate::{
        ArtifactDownloader, BrewAction, CommandSpec, DownloadOutcome, PackageContext, ProcessOutcome,
        ProcessRunner, brew_install,
    };

    fn cache() -> PathBuf {
        std::env::temp_dir()
            .join("pdl-brew-test")
            .join("wget-1.21.4.arm64_sequoia.bottle.tar.gz")
    }
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const API: &str = "https://formulae.brew.sh/api/formula/wget.json";

    #[derive(Default)]
    struct ScriptedRunner {
        calls: Mutex<Vec<CommandSpec>>,
        responses: HashMap<String, ProcessOutcome>,
    }

    impl ScriptedRunner {
        fn respond(mut self, key: &str, status: i32, stdout: &str) -> Self {
            self.responses.insert(
                key.to_owned(),
                ProcessOutcome {
                    status,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                },
            );
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|spec| format!("{} {}", spec.program, spec.args.join(" ")))
                .collect()
        }
        fn specs(&self) -> Vec<CommandSpec> {
            self.calls.lock().unwrap().clone()
        }
        fn install_calls(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|call| call.contains(" install ") || call.contains(" upgrade "))
                .collect()
        }
    }

    impl ProcessRunner for ScriptedRunner {
        fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, Error> {
            self.calls.lock().unwrap().push(spec.clone());
            let key = format!("{} {}", spec.program, spec.args.join(" "));
            self.responses
                .get(&key)
                .cloned()
                .ok_or_else(|| Error::missing_dependency(format!("no such tool response for: {key}")))
        }
    }

    struct ScriptedHttp;
    #[async_trait]
    impl SiteHttp for ScriptedHttp {
        async fn fetch_json(
            &self,
            url: &str,
            _headers: &HashMap<String, String>,
            _timeout: std::time::Duration,
        ) -> NetResult<JsonResponse> {
            if url == API {
                let body = serde_json::json!({
                    "versions": { "stable": "1.21.4" },
                    "dependencies": [],
                    "bottle": { "stable": { "files": {
                        "arm64_sequoia": {
                            "sha256": SHA,
                            "url": "https://example.com/wget.bottle.tar.gz"
                        }
                    }}}
                });
                return Ok(JsonResponse {
                    status: 200,
                    headers: HashMap::new(),
                    body,
                });
            }
            Ok(JsonResponse {
                status: 404,
                headers: HashMap::new(),
                body: serde_json::Value::Null,
            })
        }
        async fn redirect(
            &self,
            _url: &str,
            _headers: &HashMap<String, String>,
            _timeout: std::time::Duration,
        ) -> NetResult<Option<RedirectInfo>> {
            Ok(None)
        }
    }

    #[derive(Default)]
    struct ScriptedDownloader {
        calls: Mutex<Vec<(String, PathBuf)>>,
        fail_verification: bool,
    }
    #[async_trait]
    impl ArtifactDownloader for ScriptedDownloader {
        async fn download(
            &self,
            url: &str,
            dest: &Path,
            _expected: Option<&str>,
            _connections: u32,
        ) -> Result<DownloadOutcome, Error> {
            self.calls
                .lock()
                .unwrap()
                .push((url.to_owned(), dest.to_path_buf()));
            if self.fail_verification {
                return Ok(DownloadOutcome {
                    success: false,
                    bytes: 0,
                    error: Some("checksum mismatch".to_owned()),
                    exit_code: ExitCode::VerificationFailed,
                });
            }
            if let Some(parent) = dest.parent() {
                assert!(parent.exists(), "caller must create cache parent: {parent:?}");
            }
            std::fs::write(dest, b"bottle")?;
            Ok(DownloadOutcome {
                success: true,
                bytes: 6,
                error: None,
                exit_code: ExitCode::Ok,
            })
        }
    }

    fn context(
        runner: Arc<dyn ProcessRunner>,
        downloader: Arc<dyn ArtifactDownloader>,
    ) -> PackageContext {
        PackageContext {
            runner,
            http: Arc::new(ScriptedHttp),
            proxy: None,
            doh_endpoint: None,
            connections: 16,
            downloader,
        }
    }

    fn happy_runner() -> Arc<ScriptedRunner> {
        let path = cache().to_string_lossy().into_owned();
        Arc::new(
            ScriptedRunner::default()
                .respond("brew --cache wget", 0, &path)
                .respond("brew list --formula", 0, "")
                .respond("brew install wget", 0, ""),
        )
    }

    #[tokio::test]
    async fn brew_install_prefetches_bottle_then_pours() {
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("pdl-brew-test"));
        let runner = happy_runner();
        let downloader = Arc::new(ScriptedDownloader::default());
        let outcome = brew_install(&context(runner.clone(), downloader.clone()), "wget", BrewAction::Install)
            .await
            .expect("brew install succeeds");
        assert!(outcome.success);
        assert_eq!(outcome.file_path.as_deref(), Some(cache().as_path()));
        let calls = runner.calls();
        assert_eq!(
            calls,
            vec!["brew --cache wget", "brew list --formula", "brew install wget"]
        );
        let install = runner
            .specs()
            .into_iter()
            .find(|spec| spec.args.first().map(String::as_str) == Some("install"))
            .expect("install command captured");
        assert!(
            install.env.contains(&("NONINTERACTIVE".to_owned(), "1".to_owned()))
        );
        assert!(install
            .env
            .contains(&("HOMEBREW_NO_AUTO_UPDATE".to_owned(), "1".to_owned())));
        assert_eq!(downloader.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn brew_install_digest_mismatch_fails_with_verification() {
        let runner = happy_runner();
        let downloader = Arc::new(ScriptedDownloader {
            fail_verification: true,
            ..Default::default()
        });
        let result = brew_install(&context(runner.clone(), downloader), "wget", BrewAction::Install).await;
        assert!(matches!(result, Err(Error::VerificationFailed(_))), "{result:?}");
        assert!(runner.install_calls().is_empty());
    }

    #[tokio::test]
    async fn brew_upgrade_forwards_formula() {
        let path = cache().to_string_lossy().into_owned();
        let runner = Arc::new(
            ScriptedRunner::default()
                .respond("brew --cache wget", 0, &path)
                .respond("brew list --formula", 0, "")
                .respond("brew upgrade wget", 0, ""),
        );
        let result = brew_install(
            &context(runner.clone(), Arc::new(ScriptedDownloader::default())),
            "wget",
            BrewAction::Upgrade,
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert!(runner.calls().contains(&"brew upgrade wget".to_owned()));
    }

    #[tokio::test]
    async fn brew_install_missing_brew_is_missing_dependency() {
        let runner = Arc::new(ScriptedRunner::default());
        let result = brew_install(
            &context(runner, Arc::new(ScriptedDownloader::default())),
            "wget",
            BrewAction::Install,
        )
        .await;
        assert!(matches!(result, Err(Error::MissingDependency(_))), "{result:?}");
    }
}
