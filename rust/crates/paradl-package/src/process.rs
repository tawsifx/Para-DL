//! Process runner seam for invoking `uv`, `pip` and `brew` in tests.

use std::path::PathBuf;

use paradl_core::Error;

/// One external command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Program to execute.
    pub program: String,
    /// Arguments, in order.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// Optional working directory.
    pub cwd: Option<PathBuf>,
}

/// Captured result of a finished process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutcome {
    /// Process exit status (`-1` when terminated by a signal).
    pub status: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Injectable process boundary — tests substitute a scripted runner.
pub trait ProcessRunner: Send + Sync {
    /// Run one command and capture its output.
    fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, Error>;
}

/// Production runner backed by `std::process::Command`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<ProcessOutcome, Error> {
        let mut command = std::process::Command::new(&spec.program);
        command.args(&spec.args).envs(spec.env.iter().cloned());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        let output = command.output().map_err(|error| {
            Error::missing_dependency(format!("cannot start {}: {error}", spec.program))
        })?;
        Ok(ProcessOutcome {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{CommandSpec, ProcessRunner, SystemProcessRunner};

    #[test]
    fn system_runner_captures_stdout_and_status() {
        let spec = CommandSpec {
            program: "/bin/echo".to_owned(),
            args: vec!["ok".to_owned()],
            env: vec![],
            cwd: None,
        };
        let outcome = SystemProcessRunner.run(&spec).expect("echo runs");
        assert_eq!(outcome.status, 0);
        assert_eq!(outcome.stdout.trim(), "ok");
    }

    #[test]
    fn system_runner_missing_program_is_error() {
        let spec = CommandSpec {
            program: "/definitely/not/a/real/package-manager-binary".to_owned(),
            args: vec![],
            env: vec![],
            cwd: None,
        };
        let result = SystemProcessRunner.run(&spec);
        assert!(
            matches!(result, Err(paradl_core::Error::MissingDependency(_))),
            "{result:?}"
        );
    }
}
