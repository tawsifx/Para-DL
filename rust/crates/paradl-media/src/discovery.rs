//! Discover the Python interpreter that hosts the one-shot media worker.
//!
//! Precedence (final master prompt §0 ruling 10): `PARADL_MEDIA_PY` >
//! a path relative to [`std::env::current_exe`] > per-user data directories.
//! The discovered program is validated before use; an invalid explicit
//! override is an error rather than a silent fallback.

use std::path::{Path, PathBuf};

use paradl_core::{Error, get_paradl_home};

/// Environment variable that overrides media Python discovery.
pub const MEDIA_PY_ENV: &str = "PARADL_MEDIA_PY";

/// Python module invoked as `python -m paradl_media`.
pub const WORKER_MODULE: &str = "paradl_media";

/// Exact command used to launch the worker.
///
/// [`WorkerLaunch::discovered`] uses `-m paradl_media`; embeddings and tests
/// may pass an explicit program and argument vector instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLaunch {
    /// Python executable (or a test stub).
    pub program: PathBuf,
    /// Arguments passed to the program.
    pub args: Vec<String>,
}

impl WorkerLaunch {
    /// Discover Python and use the standard `-m paradl_media` invocation.
    pub fn discovered() -> Result<Self, Error> {
        Ok(Self::python(discover_python(None)?))
    }

    /// Launch `program -m paradl_media` with an explicit Python path.
    pub fn python(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec!["-m".to_owned(), WORKER_MODULE.to_owned()],
        }
    }

    /// Launch an explicit program with explicit arguments.
    ///
    /// This is the test/embedding seam: e.g. `WorkerLaunch::new("sh",
    /// ["-c", script])` starts a stub worker that emits canned frames.
    pub fn new(program: impl Into<PathBuf>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// Resolve the worker Python using the mandated precedence.
///
/// An explicit `override_path` (used by embeddings) takes priority over
/// [`MEDIA_PY_ENV`]. A missing interpreter is a missing dependency (exit `4`)
/// with install guidance; non-media paths never call this function.
pub fn discover_python(override_path: Option<&Path>) -> Result<PathBuf, Error> {
    if let Some(path) = override_path {
        return resolve_python_path(path).ok_or_else(|| {
            Error::missing_dependency(format!(
                "media worker Python not found at explicit path: {}",
                path.display()
            ))
        });
    }

    if let Some(value) = std::env::var_os(MEDIA_PY_ENV) {
        let path = PathBuf::from(value);
        return resolve_python_path(&path).ok_or_else(|| {
            Error::missing_dependency(format!(
                "{MEDIA_PY_ENV} does not point to a Python executable or venv: {}",
                path.display()
            ))
        });
    }

    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        for candidate in exe_relative_candidates(directory) {
            if let Some(python) = resolve_python_path(&candidate) {
                return Ok(python);
            }
        }
    }

    for candidate in data_dir_candidates() {
        if let Some(python) = resolve_python_path(&candidate) {
            return Ok(python);
        }
    }

    Err(Error::missing_dependency(format!(
        "media worker Python not found; set {MEDIA_PY_ENV} to a Python 3.11+ interpreter with `paradl_media` installed"
    )))
}

fn resolve_python_path(path: &Path) -> Option<PathBuf> {
    if path.is_file() && is_executable(path) {
        return Some(path.to_path_buf());
    }
    if path.is_dir() {
        for child in ["bin/python3", "bin/python", "Scripts/python.exe"] {
            let candidate = path.join(child);
            if candidate.is_file() && is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn exe_relative_candidates(exe_dir: &Path) -> Vec<PathBuf> {
    let parent = exe_dir.parent().unwrap_or(exe_dir);
    let mut candidates = vec![
        parent.join("libexec/venv"),
        parent.join("libexec/media-venv"),
        parent.join("share/paradl/media/venv"),
        parent.join("venv"),
        exe_dir.join("venv"),
    ];
    if cfg!(windows) {
        candidates.push(parent.join("libexec/venv/Scripts"));
    }
    candidates
}

fn data_dir_candidates() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg_data_home = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);

    let mut roots = vec![get_paradl_home()];
    if let Some(appdata) = appdata {
        roots.push(appdata.join("paradl"));
    }
    if let Some(home) = home {
        roots.push(home.join("Library/Application Support/paradl"));
        roots.push(home.join(".local/share/paradl"));
    }
    if let Some(xdg) = xdg_data_home {
        roots.push(xdg.join("paradl"));
    }

    let mut candidates = Vec::new();
    for root in roots {
        candidates.push(root.join("media/venv"));
        candidates.push(root.join("venv"));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_launch_defaults_to_module_invocation() {
        let launch = WorkerLaunch::python("/usr/bin/python3");
        assert_eq!(launch.program, Path::new("/usr/bin/python3"));
        assert_eq!(launch.args, ["-m", WORKER_MODULE]);
    }

    #[test]
    fn explicit_bad_python_path_fails_closed() {
        let error = discover_python(Some(Path::new("/definitely/not/a/python"))).unwrap_err();
        assert_eq!(error.exit_code().as_i32(), 4);
    }
}
