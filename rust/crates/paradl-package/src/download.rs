//! Download seam for prefetched artifacts.
//!
//! The real implementation delegates to `paradl-engine`, which already owns
//! byte-exact parallel transfer and checksum verification. Tests substitute a
//! scripted downloader so no network is touched.

use std::path::Path;

use async_trait::async_trait;
use paradl_core::{Error, ExitCode};
use paradl_engine::{DownloadEngine, Hash, NdEngine, NoopProgressSink, Target as EngineTarget};
use url::Url;

/// Result of one artifact download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadOutcome {
    /// Whether the transfer completed successfully.
    pub success: bool,
    /// Bytes committed to disk.
    pub bytes: u64,
    /// Engine error text on failure.
    pub error: Option<String>,
    /// Engine exit code (including `VerificationFailed`).
    pub exit_code: ExitCode,
}

/// Injectable artifact download boundary.
#[async_trait]
pub trait ArtifactDownloader: Send + Sync {
    /// Download `url` into `dest`, verifying `expected` (`sha256:<hex>`) when given.
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        expected: Option<&str>,
        connections: u32,
    ) -> Result<DownloadOutcome, Error>;
}

/// Production downloader backed by the parallel engine.
#[derive(Debug, Clone, Default)]
pub struct EngineArtifactDownloader {
    /// Optional proxy URL.
    pub proxy: Option<String>,
    /// Optional DNS-over-HTTPS endpoint.
    pub doh_endpoint: Option<String>,
}

#[async_trait]
impl ArtifactDownloader for EngineArtifactDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        expected: Option<&str>,
        connections: u32,
    ) -> Result<DownloadOutcome, Error> {
        let parsed = Url::parse(url)
            .map_err(|error| Error::failure(format!("invalid artifact URL {url:?}: {error}")))?;
        let checksum = match expected {
            Some(value) => Some(Hash::parse(value).ok_or_else(|| {
                Error::verification_failed(format!("invalid artifact digest: {value}"))
            })?),
            None => None,
        };
        let target = EngineTarget {
            url: parsed,
            dest: dest.to_path_buf(),
            connections,
            checksum,
            proxy: self.proxy.clone(),
            doh_endpoint: self.doh_endpoint.clone(),
            ..EngineTarget::default()
        };
        let engine = NdEngine::default();
        let plan = engine.plan(&target).await?;
        let result = engine.download(plan, &mut NoopProgressSink).await?;
        Ok(DownloadOutcome {
            success: result.success,
            bytes: result.bytes_downloaded,
            error: result.error,
            exit_code: match result.exit_code {
                0 => ExitCode::Ok,
                2 => ExitCode::Usage,
                3 => ExitCode::VerificationFailed,
                4 => ExitCode::MissingDependency,
                130 => ExitCode::Interrupted,
                _ => ExitCode::Failure,
            },
        })
    }
}
