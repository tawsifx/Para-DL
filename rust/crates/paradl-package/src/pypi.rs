//! Pip install engine: prefetch wheels into a wheelhouse, then install offline.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use paradl_core::{Error, ExitCode};
use paradl_net::routing::{ResolveContext, TargetResolver};
use paradl_net::sites::pypi::{PyPIResolver, WheelTarget, parse_pypi_target};
use paradl_net::EnvSecretProvider;

use crate::{
    CommandSpec, InstallOutcome, PackageContext, PipAction, ProcessOutcome,
};

const API_TEMPLATE: &str = "https://pypi.org/pypi/{package}/json";

fn run(ctx: &PackageContext, program: &str, args: &[&str]) -> Result<ProcessOutcome, Error> {
    ctx.runner.run(&CommandSpec {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: vec![],
        cwd: None,
    })
}

fn requirement_name(spec: &str) -> String {
    let name: String = spec
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    name.replace(['_', '.'], "-").to_ascii_lowercase()
}

fn installed_packages(ctx: &PackageContext, installer: &str, python: &str) -> HashSet<String> {
    let args: Vec<&str> = if installer == "uv" {
        vec!["pip", "list"]
    } else {
        vec!["-m", "pip", "list"]
    };
    let Ok(outcome) = run(ctx, python, &args) else {
        return HashSet::new();
    };
    outcome
        .stdout
        .lines()
        .skip(2)
        .filter_map(|line| line.split_whitespace().next())
        .map(requirement_name)
        .collect()
}

fn probe_python_version(ctx: &PackageContext, python: &str) -> Option<(u32, u32)> {
    let outcome = run(
        ctx,
        python,
        &["-c", "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')"],
    )
    .ok()?;
    if outcome.status != 0 {
        return None;
    }
    let text = outcome.stdout.trim();
    let (major, minor) = text.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

async fn requires_dist(ctx: &PackageContext, requirement: &str) -> Vec<String> {
    let (package, _version) = parse_pypi_target(requirement);
    let url = API_TEMPLATE.replace("{package}", &requirement_name(&package));
    let Ok(response) = ctx
        .http
        .fetch_json(&url, &std::collections::HashMap::new(), Duration::from_secs(15))
        .await
    else {
        return Vec::new();
    };
    response
        .body
        .get("info")
        .and_then(|info| info.get("requires_dist"))
        .and_then(serde_json::Value::as_array)
        .map(|deps| {
            deps.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn prefetch_wheel(
    ctx: &PackageContext,
    resolver: &PyPIResolver,
    context: &ResolveContext,
    requirement: &str,
    wheelhouse: &Path,
) -> Result<(), Error> {
    let assets = resolver.resolve(&format!("pip:{requirement}"), context).await?;
    let asset = assets
        .first()
        .ok_or_else(|| Error::failure(format!("no wheel for {requirement}")))?;
    let filename = asset
        .filename
        .clone()
        .unwrap_or_else(|| format!("{requirement}.whl"));
    let dest = wheelhouse.join(filename);
    let outcome = ctx
        .downloader
        .download(&asset.url, &dest, asset.digest.as_deref(), ctx.connections)
        .await?;
    if outcome.exit_code == ExitCode::VerificationFailed {
        return Err(Error::verification_failed(format!(
            "wheel digest verification failed for {requirement}"
        )));
    }
    if !outcome.success {
        return Err(Error::failure(
            outcome
                .error
                .unwrap_or_else(|| format!("wheel download failed for {requirement}")),
        ));
    }
    Ok(())
}

/// Prefetch the root wheel and its transitive dependencies into a temporary
/// wheelhouse, then install offline with `uv pip` (or `python -m pip`).
pub async fn pip_install(
    ctx: &PackageContext,
    package: &str,
    action: PipAction,
) -> Result<InstallOutcome, Error> {
    let uv_available = run(ctx, "uv", &["--version"])
        .map(|outcome| outcome.status == 0)
        .unwrap_or(false);
    let python = std::env::var("VIRTUAL_ENV")
        .ok()
        .map(|venv| format!("{venv}/bin/python"))
        .unwrap_or_else(|| "python3".to_owned());
    if !uv_available && run(ctx, &python, &["--version"]).map(|o| o.status != 0).unwrap_or(true) {
        return Err(Error::missing_dependency(
            "neither uv nor a Python interpreter is available",
        ));
    }

    let py_version = probe_python_version(ctx, &python);
    let resolve_context = ResolveContext {
        py_version,
        proxy: ctx.proxy.clone(),
        doh_endpoint: ctx.doh_endpoint.clone(),
        timeout: Duration::from_secs(15),
        ..ResolveContext::default()
    };
    let resolver = PyPIResolver::from_shared(
        std::sync::Arc::clone(&ctx.http),
        std::sync::Arc::new(EnvSecretProvider::from_process()),
    )
    .with_target(WheelTarget::from_context(&resolve_context));

    let wheelhouse = tempfile::tempdir()
        .map_err(|error| Error::failure(format!("cannot create wheelhouse: {error}")))?;
    let installed = installed_packages(ctx, if uv_available { "uv" } else { "pip" }, &python);
    let mut queue = vec![package.to_owned()];
    let mut visited = HashSet::new();
    while let Some(requirement) = queue.pop() {
        let name = requirement_name(&requirement);
        if name.is_empty() || !visited.insert(name.clone()) {
            continue;
        }
        // Dependencies already installed are skipped; the requested package is
        // always re-resolved so install/upgrade can prefetch its wheel.
        if name != requirement_name(package) && installed.contains(&name) {
            continue;
        }
        prefetch_wheel(ctx, &resolver, &resolve_context, &requirement, wheelhouse.path()).await?;
        for dep in requires_dist(ctx, &requirement).await {
            if !visited.contains(&requirement_name(&dep)) {
                queue.push(dep);
            }
        }
    }

    let mut args: Vec<String> = vec!["install".to_owned(), "--no-index".to_owned()];
    args.push("--find-links".to_owned());
    args.push(wheelhouse.path().to_string_lossy().into_owned());
    if action == PipAction::Upgrade {
        args.push("--upgrade".to_owned());
    }
    if uv_available && std::env::var("VIRTUAL_ENV").is_ok() {
        args.push("--python".to_owned());
        args.push(python.clone());
    }
    args.push(package.to_owned());
    let program = if uv_available { "uv" } else { &python };
    let cmd_args: Vec<&str> = if uv_available {
        let mut prefix = vec!["pip"];
        prefix.extend(args.iter().map(String::as_str));
        prefix
    } else {
        let mut prefix = vec!["-m", "pip"];
        prefix.extend(args.iter().map(String::as_str));
        prefix
    };
    let outcome = run(ctx, program, &cmd_args)?;
    if outcome.status != 0 {
        return Err(Error::failure(
            if outcome.stderr.trim().is_empty() {
                format!("pip install failed with exit code {}", outcome.status)
            } else {
                outcome.stderr.trim().to_owned()
            },
        ));
    }
    Ok(InstallOutcome {
        success: true,
        file_path: None,
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
    use paradl_core::Error;
    use paradl_net::sites::{JsonResponse, RedirectInfo, SiteHttp};
    use paradl_net::Result as NetResult;

    use crate::{
        ArtifactDownloader, CommandSpec, DownloadOutcome, PackageContext, PipAction, ProcessOutcome,
        ProcessRunner, pip_install,
    };

    const SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[derive(Default)]
    struct ScriptedRunner {
        calls: Mutex<Vec<CommandSpec>>,
        python_version: String,
        installed: String,
        fail_install: bool,
    }
    impl ScriptedRunner {
        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|spec| format!("{} {}", spec.program, spec.args.join(" ")))
                .collect()
        }
        fn install_key(&self) -> Option<String> {
            self.calls().into_iter().find(|call| call.contains(" install "))
        }
    }
    impl ProcessRunner for ScriptedRunner {
        fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, Error> {
            self.calls.lock().unwrap().push(spec.clone());
            let stdout = if spec.args.first().map(String::as_str) == Some("-c") {
                self.python_version.clone()
            } else if spec.args.iter().any(|arg| arg == "list") {
                self.installed.clone()
            } else {
                String::new()
            };
            let status = if self.fail_install && spec.args.iter().any(|arg| arg == "install") {
                1
            } else {
                0
            };
            Ok(ProcessOutcome {
                status,
                stdout,
                stderr: String::new(),
            })
        }
    }

    struct ScriptedHttp {
        requested: Arc<Mutex<Vec<String>>>,
        cp311: bool,
    }
    #[async_trait]
    impl SiteHttp for ScriptedHttp {
        async fn fetch_json(
            &self,
            url: &str,
            _headers: &HashMap<String, String>,
            _timeout: std::time::Duration,
        ) -> NetResult<JsonResponse> {
            self.requested.lock().unwrap().push(url.to_owned());
            let body = if url.contains("/requests/") {
                let wheels = if self.cp311 {
                    vec![
                        serde_json::json!({
                            "filename": "requests-2.31.0-cp311-cp311-macosx_11_0_arm64.whl",
                            "url": "https://files.example/requests-cp311.whl",
                            "digests": { "sha256": SHA },
                            "packagetype": "bdist_wheel"
                        }),
                        serde_json::json!({
                            "filename": "requests-2.31.0-cp312-cp312-macosx_11_0_arm64.whl",
                            "url": "https://files.example/requests-cp312.whl",
                            "digests": { "sha256": SHA },
                            "packagetype": "bdist_wheel"
                        }),
                    ]
                } else {
                    vec![serde_json::json!({
                        "filename": "requests-2.31.0-py3-none-any.whl",
                        "url": "https://files.example/requests.whl",
                        "digests": { "sha256": SHA },
                        "packagetype": "bdist_wheel"
                    })]
                };
                serde_json::json!({ "urls": wheels, "info": { "requires_dist": ["urllib3>=1.21.1"] } })
            } else if url.contains("/urllib3/") {
                serde_json::json!({
                    "urls": [serde_json::json!({
                        "filename": "urllib3-2.0.0-py3-none-any.whl",
                        "url": "https://files.example/urllib3.whl",
                        "digests": { "sha256": SHA },
                        "packagetype": "bdist_wheel"
                    })],
                    "info": { "requires_dist": [] }
                })
            } else {
                serde_json::Value::Null
            };
            Ok(JsonResponse {
                status: if body.is_null() { 404 } else { 200 },
                headers: HashMap::new(),
                body,
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
        calls: Mutex<Vec<String>>,
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
            self.calls.lock().unwrap().push(url.to_owned());
            if self.fail_verification {
                return Ok(DownloadOutcome {
                    success: false,
                    bytes: 0,
                    error: Some("checksum mismatch".to_owned()),
                    exit_code: paradl_core::ExitCode::VerificationFailed,
                });
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(dest, b"wheel")?;
            Ok(DownloadOutcome {
                success: true,
                bytes: 5,
                error: None,
                exit_code: paradl_core::ExitCode::Ok,
            })
        }
    }

    fn context(
        runner: Arc<dyn ProcessRunner>,
        http: Arc<dyn SiteHttp>,
        downloader: Arc<dyn ArtifactDownloader>,
    ) -> PackageContext {
        PackageContext {
            runner,
            http,
            proxy: None,
            doh_endpoint: None,
            connections: 16,
            downloader,
        }
    }

    #[tokio::test]
    async fn pip_install_prefetches_root_and_dependency_wheels() {
        let runner = Arc::new(ScriptedRunner {
            python_version: "3.12".to_owned(),
            ..Default::default()
        });
        let downloader = Arc::new(ScriptedDownloader::default());
        let outcome = pip_install(
            &context(runner.clone(), Arc::new(ScriptedHttp { requested: Arc::new(Mutex::new(vec![])), cp311: false }), downloader.clone()),
            "requests",
            PipAction::Install,
        )
        .await
        .expect("pip install succeeds");
        assert!(outcome.success);
        assert_eq!(downloader.calls.lock().unwrap().len(), 2);
        let install = runner.install_key().expect("install command");
        assert!(install.contains("--no-index --find-links"), "{install}");
    }

    #[tokio::test]
    async fn pip_upgrade_re_resolves_installed_package() {
        let runner = Arc::new(ScriptedRunner {
            python_version: "3.12".to_owned(),
            installed: "Package Version\n------- -------\nrequests 2.31.0\n".to_owned(),
            ..Default::default()
        });
        let downloader = Arc::new(ScriptedDownloader::default());
        let outcome = pip_install(
            &context(runner.clone(), Arc::new(ScriptedHttp { requested: Arc::new(Mutex::new(vec![])), cp311: false }), downloader.clone()),
            "requests",
            PipAction::Upgrade,
        )
        .await
        .expect("pip upgrade succeeds");
        assert!(outcome.success);
        assert_eq!(downloader.calls.lock().unwrap().len(), 2);
        assert!(runner.install_key().unwrap().contains("--upgrade"));
    }

    #[tokio::test]
    async fn pip_install_uses_detected_python_version() {
        let runner = Arc::new(ScriptedRunner {
            python_version: "3.11".to_owned(),
            ..Default::default()
        });
        let downloader = Arc::new(ScriptedDownloader::default());
        pip_install(
            &context(runner, Arc::new(ScriptedHttp { requested: Arc::new(Mutex::new(vec![])), cp311: true }), downloader.clone()),
            "requests",
            PipAction::Install,
        )
        .await
        .expect("pip install succeeds");
        let calls = downloader.calls.lock().unwrap();
        assert_eq!(calls.first().map(String::as_str), Some("https://files.example/requests-cp311.whl"), "{calls:?}");
    }

    #[tokio::test]
    async fn pip_install_digest_mismatch_is_verification_failed() {
        let runner = Arc::new(ScriptedRunner {
            python_version: "3.12".to_owned(),
            ..Default::default()
        });
        let downloader = Arc::new(ScriptedDownloader { fail_verification: true, ..Default::default() });
        let result = pip_install(
            &context(runner.clone(), Arc::new(ScriptedHttp { requested: Arc::new(Mutex::new(vec![])), cp311: false }), downloader),
            "requests",
            PipAction::Install,
        )
        .await;
        assert!(matches!(result, Err(Error::VerificationFailed(_))), "{result:?}");
        assert!(runner.install_key().is_none());
    }
}
