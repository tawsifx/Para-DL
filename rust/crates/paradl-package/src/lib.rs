//! Package-manager install orchestration for the native binary.
//!
//! Resolves artifacts through `paradl-net`, prefetches them with the parallel
//! engine, then invokes the real installer (`uv`/`brew`) through an injectable
//! process runner so the whole pipeline is testable without side effects.

use std::path::PathBuf;
use std::sync::Arc;

use paradl_core::ExitCode;
use paradl_net::SiteHttp;

pub mod brew;
pub mod download;
pub mod process;

pub use brew::{brew_formula_dependencies, brew_install};
pub use download::{
    ArtifactDownloader, DownloadOutcome, EngineArtifactDownloader,
};
pub use process::{CommandSpec, ProcessOutcome, ProcessRunner, SystemProcessRunner};

/// Everything the install engines need, with both side-effect seams injectable.
#[derive(Clone)]
pub struct PackageContext {
    /// Process boundary (`uv`, `pip`, `brew`).
    pub runner: Arc<dyn ProcessRunner>,
    /// HTTP boundary for site-resolver calls.
    pub http: Arc<dyn SiteHttp>,
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Optional DNS-over-HTTPS endpoint.
    pub doh_endpoint: Option<String>,
    /// Parallel connections used for prefetch.
    pub connections: u32,
    /// Injectable artifact download boundary.
    pub downloader: Arc<dyn ArtifactDownloader>,
}

/// Terminal result of an install operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// Whether the installer exited successfully.
    pub success: bool,
    /// Installed artifact path, when meaningful.
    pub file_path: Option<PathBuf>,
    /// Installer stderr on failure.
    pub error: Option<String>,
    /// Mapped process exit code.
    pub exit_code: ExitCode,
}

/// Homebrew install action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrewAction {
    /// Install a formula.
    Install,
    /// Upgrade a formula.
    Upgrade,
    /// Update the formulae catalog.
    Update,
}

/// Pip install action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipAction {
    /// Install a package.
    Install,
    /// Upgrade a package.
    Upgrade,
}
