//! Completion / failure hooks with strict timeout isolation.
//!
//! Port of `src/paradl/resilience/hooks.py`. Executables under
//! `$PARADL_HOME/hooks/on_complete` or `on_fail` receive the result JSON on
//! **stdin** with a 30-second timeout; a hanging hook is killed with SIGKILL
//! (`Child::kill` on Unix). Hooks never block or crash the caller and every
//! outcome is a typed `hook_*` event.
//!
//! ```
//! use std::time::Duration;
//! use paradl_resilience::hooks::{HookRunner, HookType};
//! let dir = tempfile::tempdir()?;
//! let runner = HookRunner::new(dir.path().to_path_buf(), Duration::from_secs(30));
//! assert!(runner.find_executables(HookType::OnComplete)?.is_empty());
//! # Ok::<(), paradl_core::Error>(())
//! ```

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use paradl_core::get_paradl_home;
use paradl_events::{
    Event, HookErrorEvent, HookExecutedEvent, HookFailedEvent, HookTimeoutEvent, to_json_line,
};
use serde_json::Value;

use crate::{Emitted, Result};

/// Default hook timeout: 30 seconds (`DEFAULT_HOOK_TIMEOUT_SEC`).
pub const DEFAULT_HOOK_TIMEOUT_SEC: f64 = 30.0;

/// Which hook directory to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    /// Executed after a successful transfer.
    OnComplete,
    /// Executed after a failed transfer.
    OnFail,
}

impl HookType {
    /// Directory name under `$PARADL_HOME/hooks`.
    pub fn as_dir(self) -> &'static str {
        match self {
            Self::OnComplete => "on_complete",
            Self::OnFail => "on_fail",
        }
    }
}

/// Discovers and safely executes user hooks with a hard timeout.
pub struct HookRunner {
    hooks_dir: PathBuf,
    timeout: Duration,
}

impl HookRunner {
    /// Construct a runner over an explicit hooks directory.
    pub fn new(hooks_dir: PathBuf, timeout: Duration) -> Self {
        Self { hooks_dir, timeout }
    }

    /// Construct a runner over `$PARADL_HOME/hooks` with the 30 s default.
    pub fn with_defaults() -> Self {
        Self::new(
            get_paradl_home().join("hooks"),
            Duration::from_secs_f64(DEFAULT_HOOK_TIMEOUT_SEC),
        )
    }

    /// List executable hooks for a hook type, sorted by name.
    pub fn find_executables(&self, hook_type: HookType) -> Result<Vec<PathBuf>> {
        let target = self.hooks_dir.join(hook_type.as_dir());
        if !target.exists() {
            return Ok(Vec::new());
        }
        if target.is_file() && is_executable(&target) {
            return Ok(vec![target]);
        }
        let mut executables = Vec::new();
        if target.is_dir() {
            for entry in std::fs::read_dir(&target).map_err(paradl_core::Error::from)? {
                let path = entry.map_err(paradl_core::Error::from)?.path();
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
                if path.is_file() && !name.starts_with('.') && is_executable(&path) {
                    executables.push(path);
                }
            }
        }
        executables.sort();
        Ok(executables)
    }

    /// Execute one hook with JSON on stdin; never panics or propagates a hook failure.
    pub fn execute_hook(&self, hook: &Path, payload_json: &str) -> Emitted {
        let name = hook.to_string_lossy().into_owned();
        let mut child = match Command::new(hook)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                return Emitted::new(Event::HookError(HookErrorEvent::new(
                    crate::now(),
                    name,
                    error.to_string(),
                )));
            }
        };

        if let Some(mut stdin) = child.stdin.take()
            && let Err(error) = stdin.write_all(payload_json.as_bytes())
        {
            return Emitted::new(Event::HookError(HookErrorEvent::new(
                crate::now(),
                name,
                error.to_string(),
            )));
        }

        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    let stderr = stderr.trim().to_string();
                    if status.success() {
                        return Emitted::new(Event::HookExecuted(HookExecutedEvent::new(
                            crate::now(),
                            name,
                            true,
                        )));
                    }
                    return Emitted::new(Event::HookFailed(HookFailedEvent::new(
                        crate::now(),
                        name,
                        status.code().unwrap_or(-1),
                        if stderr.is_empty() { None } else { Some(stderr) },
                    )));
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        // `Child::kill` is SIGKILL on Unix.
                        let _ = child.kill();
                        let _ = child.wait();
                        return Emitted::new(Event::HookTimeout(HookTimeoutEvent::new(
                            crate::now(),
                            name,
                            self.timeout.as_secs_f64(),
                        )));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Emitted::new(Event::HookError(HookErrorEvent::new(
                        crate::now(),
                        name,
                        error.to_string(),
                    )));
                }
            }
        }
    }

    /// Serialize a result payload through the single emitter and run all hooks.
    ///
    /// The JSON handed to hook stdin uses `to_json_line` (Python `json.dumps`
    /// separators) with redaction — never a raw `json.dumps`-equivalent.
    pub fn run_result_actions(&self, payload: &Value, is_success: bool) -> Result<Vec<Emitted>> {
        let hook_type = if is_success {
            HookType::OnComplete
        } else {
            HookType::OnFail
        };
        let hooks = self.find_executables(hook_type)?;
        if hooks.is_empty() {
            return Ok(Vec::new());
        }
        let payload_json = to_json_line(payload);
        Ok(hooks
            .iter()
            .map(|hook| self.execute_hook(hook, &payload_json))
            .collect())
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("exe")
}
