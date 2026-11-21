//! Centralized state-path management honoring `PARADL_HOME`.
//!
//! Byte-exact port of `src/paradl/core/paths.py`. All paths live under the
//! resolved `PARADL_HOME` directory, which defaults to `~/.config/paradl`
//! (or `%LOCALAPPDATA%/paradl` on Windows).

use std::env;
use std::path::{Component, Path, PathBuf};

/// Return `true` when running on Windows (`sys.platform == "win32"` in Python).
pub fn is_windows() -> bool {
    cfg!(windows)
}

fn home_dir() -> PathBuf {
    let key = if is_windows() { "USERPROFILE" } else { "HOME" };
    env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        home_dir()
    } else if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        home_dir().join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let base = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut result = PathBuf::new();
    for component in base.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn resolve(path: PathBuf) -> PathBuf {
    let normalized = lexical_normalize(path);
    std::fs::canonicalize(&normalized).unwrap_or(normalized)
}

/// Return the paraDL state directory, honoring `PARADL_HOME`.
///
/// A non-empty, whitespace-trimmed `PARADL_HOME` value is expanded and resolved
/// first; otherwise Windows uses `%LOCALAPPDATA%/paradl` and other platforms use
/// `~/.config/paradl`.
pub fn get_paradl_home() -> PathBuf {
    if let Some(value) = env::var_os("PARADL_HOME") {
        let trimmed = value.to_string_lossy().trim().to_owned();
        if !trimmed.is_empty() {
            return resolve(expand_user(&trimmed));
        }
    }
    if is_windows() {
        for key in ["LOCALAPPDATA", "APPDATA"] {
            if let Some(value) = env::var_os(key) {
                let trimmed = value.to_string_lossy().trim().to_owned();
                if !trimmed.is_empty() {
                    return resolve(PathBuf::from(trimmed).join("paradl"));
                }
            }
        }
    }
    resolve(home_dir().join(".config").join("paradl"))
}

/// Return the path to `config.json` inside [`get_paradl_home`].
pub fn config_file() -> PathBuf {
    get_paradl_home().join("config.json")
}

/// Return the path to the legacy `history.json` file.
pub fn legacy_history_file() -> PathBuf {
    get_paradl_home().join("history.json")
}

/// Return the path to the SQLite `history.db` file.
pub fn history_db_file() -> PathBuf {
    get_paradl_home().join("history.db")
}

/// Return the path to `mirrors.json`.
pub fn mirrors_file() -> PathBuf {
    get_paradl_home().join("mirrors.json")
}

/// Return the path to the logs directory.
pub fn logs_dir() -> PathBuf {
    get_paradl_home().join("logs")
}

/// Return the path to the throttle diagnostics directory.
pub fn throttle_dir() -> PathBuf {
    get_paradl_home().join("throttle")
}

/// Return the path to custom `targets.toml`.
pub fn targets_file() -> PathBuf {
    get_paradl_home().join("targets.toml")
}

/// Return the default helper-binary directory (Python `default_bin_dir`).
pub fn default_bin_dir() -> String {
    if is_windows()
        && let Some(local) = env::var_os("LOCALAPPDATA")
        && !local.to_string_lossy().trim().is_empty()
    {
        let local = local.to_string_lossy();
        return Path::new(local.as_ref())
            .join("Programs")
            .join("pdl")
            .join("bin")
            .to_string_lossy()
            .into_owned();
    }
    home_dir()
        .join(".local")
        .join("bin")
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_helpers_are_children_of_home() {
        let home = get_paradl_home();
        for path in [
            config_file(),
            legacy_history_file(),
            history_db_file(),
            mirrors_file(),
            logs_dir(),
            throttle_dir(),
            targets_file(),
        ] {
            assert_eq!(path.parent(), Some(home.as_path()));
        }
    }

    #[test]
    fn lexical_normalize_handles_dot_segments() {
        assert_eq!(
            lexical_normalize(PathBuf::from("/a/b/../c")),
            PathBuf::from("/a/c")
        );
    }
}
