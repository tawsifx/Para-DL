//! The public [`DownloadEngine`] trait, its supporting types, and the native
//! implementation promoted from `rapids`.
//!
//! The trait is the authoritative interface from the framework charter section 2.
//! Planning performs one probe (with mirror failover); downloading consumes the
//! resulting [`Plan`] without re-probing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use paradl_core::TransferResult;
use reqwest::header::HeaderMap;
use url::Url;

use crate::download::{Download, Snapshot, StartOptions};
use crate::error::Result;
use crate::probe::{self, ProbeInfo};
use crate::state::Validators;
use crate::verify::Hash;

/// Hard cap on concurrent connections (parity-matrix literal L06).
///
/// The engine always uses `min(max(n, 1), 16)`; values above this cap are never
/// honoured, and a zero/empty request still gets one connection.
pub const MAX_CONNECTIONS: u32 = 16;

/// Lower bound on concurrent connections (parity-matrix literal L06).
pub const MIN_CONNECTIONS: u32 = 1;

/// Clamp a requested connection count to the L06 contract `min(max(n, 1), 16)`.
pub fn clamp_connections(n: u32) -> u32 {
    n.clamp(MIN_CONNECTIONS, MAX_CONNECTIONS)
}

/// A download request: what to fetch, where to put it, and how.
///
/// This is the input to [`DownloadEngine::plan`]. Sensitive headers are scoped
/// by the engine to the host they were captured for and never cross to a mirror
/// or a cross-host redirect target.
#[derive(Clone, Debug)]
pub struct Target {
    /// Primary URL to download.
    pub url: Url,
    /// Fallback mirrors, tried in order when the primary fails.
    pub mirrors: Vec<Url>,
    /// Final destination path (the part file is `<dest>.pdl.part`).
    pub dest: PathBuf,
    /// Requested concurrent connections; clamped to [`clamp_connections`].
    pub connections: u32,
    /// Extra HTTP headers; host-sensitive ones are scoped to the original host.
    pub headers: HeaderMap,
    /// Expected checksum supplied by the caller, if any.
    pub checksum: Option<Hash>,
    /// Rate limit in bytes per second (`None` = unlimited).
    pub rate_limit_bps: Option<u64>,
    /// Smallest half a planner split may leave, in bytes.
    pub min_split: u64,
    /// Resume from an existing `.pdl.part` / `.pdl.json` pair.
    pub resume: bool,
    /// Optional SOCKS/HTTP proxy URL.
    pub proxy: Option<String>,
    /// IP-family pin: `ipv4`/`v4`, `ipv6`/`v6`, or `auto`.
    pub ip_family: Option<String>,
    /// Optional DNS-over-HTTPS endpoint (used only with the `doh` feature).
    pub doh_endpoint: Option<String>,
    /// Raw Cookie header, sent only to [`Self::cookie_hosts`].
    pub cookie_header: Option<String>,
    /// Exact hosts the cookie jar was captured for.
    pub cookie_hosts: Vec<String>,
    /// Accepted `--http3` flag. HTTP/3 is logged-dropped, never exposed as a
    /// capability or Cargo feature; transfers use TCP/TLS (HTTP/1.1 or HTTP/2).
    pub http3: bool,
    /// Optional cooperative cancel flag. When a transfer observes it set,
    /// workers are cancelled and [`DownloadEngine::download`] returns a failed
    /// `TransferResult` with `ExitCode::Interrupted` (130); the part file and
    /// `.pdl.json` sidecar are preserved for resume.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for Target {
    fn default() -> Self {
        Target {
            url: Url::parse("http://localhost/").expect("valid default URL"),
            mirrors: Vec::new(),
            dest: PathBuf::from("download"),
            connections: MAX_CONNECTIONS,
            headers: HeaderMap::new(),
            checksum: None,
            rate_limit_bps: None,
            min_split: 64 * 1024,
            resume: false,
            proxy: None,
            ip_family: None,
            doh_endpoint: None,
            cookie_header: None,
            cookie_hosts: Vec::new(),
            http3: false,
            cancel: None,
        }
    }
}

/// The immutable result of [`DownloadEngine::plan`].
///
/// It records the post-redirect URL that every worker and every recycle uses,
/// together with the probe-derived size, validators and checksum. It can be
/// handed straight to [`DownloadEngine::download`].
#[derive(Clone, Debug)]
pub struct Plan {
    /// The target this plan was built for.
    pub target: Target,
    /// URL after redirects; the URI pool is built from this and never rebuilt.
    pub final_url: Url,
    /// `Content-Length` from the probe, when announced.
    pub size: Option<u64>,
    /// Whether the server advertises byte ranges.
    pub resumable: bool,
    /// `Content-Disposition` (or URL-derived) filename.
    pub file_name: String,
    /// Response `Content-Type`, when present.
    pub content_type: Option<String>,
    /// Strong validators for conditional requests.
    pub validators: Validators,
    /// Expected checksum (caller-supplied or server-announced).
    pub checksum: Option<Hash>,
    /// A close-delimited stream with no known length: no false 100% progress.
    pub unknown_size: bool,
    /// Probe candidates that failed (mirrors are skipped on the first request).
    pub(crate) probe_failed: Vec<Url>,
}

/// A sink that receives periodic [`Snapshot`] updates during a download.
///
/// Implementors must be `Send` so the download future itself can drive updates
/// without spawning a `'static` task.
pub trait ProgressSink: Send {
    /// Called with a point-in-time progress view.
    fn on_progress(&mut self, snapshot: &Snapshot);
}

/// A [`ProgressSink`] that discards every update.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn on_progress(&mut self, _snapshot: &Snapshot) {}
}

impl<F> ProgressSink for F
where
    F: FnMut(&Snapshot) + Send,
{
    fn on_progress(&mut self, snapshot: &Snapshot) {
        self(snapshot);
    }
}

/// What a native engine can do. Used by callers to decide whether to fall back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineCapabilities {
    /// Effective hard cap on concurrent connections.
    pub max_connections: u32,
    /// Segmented ranged requests are supported.
    pub supports_ranges: bool,
    /// `.pdl.part` / `.pdl.json` resume is supported.
    pub supports_resume: bool,
    /// Whole-file checksum verification is supported.
    pub supports_checksum: bool,
    /// Proxy transfers are supported.
    pub supports_proxy: bool,
    /// IPv6 transfers are supported.
    pub supports_ipv6: bool,
    /// DNS-over-HTTPS is compiled in (the `doh` feature).
    pub supports_doh: bool,
    /// Always `false`: HTTP/3 is accepted and logged-dropped with a reason.
    pub http3: bool,
}

impl Default for EngineCapabilities {
    fn default() -> Self {
        EngineCapabilities {
            max_connections: MAX_CONNECTIONS,
            supports_ranges: true,
            supports_resume: true,
            supports_checksum: true,
            supports_proxy: true,
            supports_ipv6: true,
            supports_doh: cfg!(feature = "doh"),
            http3: false,
        }
    }
}

/// Reusable download engine (framework charter section 2).
///
/// `plan` and `download` are separate so callers can inspect (persist, display,
/// or memoise) the plan before committing to a transfer. Both are `Send + Sync`.
#[async_trait]
pub trait DownloadEngine: Send + Sync {
    /// Probe `target` and produce a transfer plan.
    async fn plan(&self, target: &Target) -> Result<Plan>;

    /// Execute `plan`, reporting progress to `sink`.
    ///
    /// Transfer outcomes (including checksum mismatch and cancellation) are
    /// returned as an `Ok(TransferResult)` with `success = false` and the correct
    /// `exit_code`, matching the Python oracle's `execute()` contract. Errors
    /// that prevent the transfer from starting are returned as `Err`.
    async fn download(&self, plan: Plan, sink: &mut dyn ProgressSink) -> Result<TransferResult>;

    /// Report what this engine supports.
    fn capabilities(&self) -> EngineCapabilities;
}

/// The native download engine promoted from `rapids`.
///
/// It is stateless and cheap to clone: all per-transfer state lives in the
/// [`Download`] handle created by [`DownloadEngine::download`].
#[derive(Clone, Copy, Debug, Default)]
pub struct NdEngine {
    /// Timeout applied to each connection attempt.
    pub connect_timeout: Duration,
    /// How often progress is pushed to the sink.
    pub progress_interval: Duration,
}

impl NdEngine {
    /// Build an engine with production defaults (15 s connect timeout, 100 ms ticks).
    pub fn new() -> Self {
        NdEngine {
            connect_timeout: Duration::from_secs(15),
            progress_interval: Duration::from_millis(100),
        }
    }
}

#[async_trait]
impl DownloadEngine for NdEngine {
    async fn plan(&self, target: &Target) -> Result<Plan> {
        if target.http3 {
            // Accepted-and-logged dropped flag (global constraint): there is no
            // HTTP/3 capability and deliberately no Cargo feature to enable it.
            tracing::warn!(
                "--http3 was accepted but is not implemented: transferring over TCP/TLS \
                 (HTTP/1.1 or HTTP/2); no HTTP/3 feature is exposed"
            );
        }
        let client = probe::build_client(
            target.proxy.as_deref(),
            target.ip_family.as_deref(),
            self.effective_connect_timeout(),
            target.doh_endpoint.as_deref(),
        )?;
        let base = StartOptions {
            url: target.url.clone(),
            mirrors: target.mirrors.clone(),
            dest: target.dest.clone(),
            connections: clamp_connections(target.connections),
            headers: target.headers.clone(),
            checksum: target.checksum.clone(),
            rate_limit_bps: target.rate_limit_bps,
            min_split: target.min_split,
            resume: target.resume,
            proxy: target.proxy.clone(),
            ip_family: target.ip_family.clone(),
            doh_endpoint: target.doh_endpoint.clone(),
            cookie_header: target.cookie_header.clone(),
            cookie_hosts: target.cookie_hosts.clone(),
        };
        let (info, probe_failed) = Download::probe_candidates(&client, &base).await?;
        let unknown_size = info.size.is_none() || !info.resumable;
        Ok(Plan {
            target: target.clone(),
            final_url: info.final_url,
            size: info.size,
            resumable: info.resumable,
            file_name: info.file_name,
            content_type: info.content_type,
            validators: info.validators,
            checksum: base.checksum.or(info.checksum),
            unknown_size,
            probe_failed,
        })
    }

    async fn download(&self, plan: Plan, sink: &mut dyn ProgressSink) -> Result<TransferResult> {
        let started = Instant::now();
        let t = plan.target;
        let opts = StartOptions {
            url: t.url.clone(),
            mirrors: t.mirrors.clone(),
            dest: t.dest.clone(),
            connections: clamp_connections(t.connections),
            headers: t.headers.clone(),
            checksum: plan.checksum.clone(),
            rate_limit_bps: t.rate_limit_bps,
            min_split: t.min_split,
            resume: t.resume,
            proxy: t.proxy.clone(),
            ip_family: t.ip_family.clone(),
            doh_endpoint: t.doh_endpoint.clone(),
            cookie_header: t.cookie_header.clone(),
            cookie_hosts: t.cookie_hosts.clone(),
        };
        let client = probe::build_client(
            t.proxy.as_deref(),
            t.ip_family.as_deref(),
            self.effective_connect_timeout(),
            t.doh_endpoint.as_deref(),
        )?;
        let info = ProbeInfo {
            url: t.url.clone(),
            final_url: plan.final_url,
            size: plan.size,
            resumable: plan.resumable,
            file_name: plan.file_name,
            content_type: plan.content_type,
            validators: plan.validators,
            checksum: plan.checksum.clone(),
        };
        let download = Download::start_with_probe(opts, client, info, plan.probe_failed).await?;
        let cancel = t.cancel.clone();

        let wait = download.wait();
        tokio::pin!(wait);
        let mut tick = tokio::time::interval(self.effective_progress_interval());
        let outcome = loop {
            tokio::select! {
                result = &mut wait => break result,
                _ = tick.tick() => {
                    sink.on_progress(&download.poll_snapshot());
                    if cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                        // Cancel workers so the part file and sidecar stay on disk, then
                        // surface the interrupt as a failed TransferResult (exit 130).
                        let _ = download.cancel();
                        break Err(crate::error::cancelled());
                    }
                }
            }
        };

        let duration = started.elapsed().as_secs_f64();
        let checksum_fields = plan.checksum.as_ref().map(|hash| {
            (
                Some(true),
                Some(hash.algo.name().to_ascii_lowercase().replace('-', "")),
                Some(hex::encode(&hash.value)),
            )
        });
        match outcome {
            Ok(o) => {
                let bytes = o.downloaded;
                let (verified, algo, digest) = if o.success {
                    checksum_fields.unwrap_or((None, None, None))
                } else {
                    (None, None, None)
                };
                Ok(TransferResult {
                    success: o.success,
                    file_path: o.path.clone(),
                    files: o.path.into_iter().collect(),
                    bytes_downloaded: bytes,
                    duration,
                    avg_speed_bps: if duration > 0.0 {
                        bytes as f64 / duration
                    } else {
                        0.0
                    },
                    checksum_verified: verified,
                    checksum_algo: algo,
                    checksum_digest: digest,
                    exit_code: 0,
                    ..TransferResult::default()
                })
            }
            Err(e) => {
                let code = e.exit_code();
                let (verified, algo, digest) = if code.as_i32() == 3 {
                    checksum_fields
                        .map(|(_, algo, digest)| (Some(false), algo, digest))
                        .unwrap_or_default()
                } else {
                    (None, None, None)
                };
                Ok(TransferResult {
                    success: false,
                    bytes_downloaded: download.poll_snapshot().downloaded,
                    duration,
                    checksum_verified: verified,
                    checksum_algo: algo,
                    checksum_digest: digest,
                    error: Some(e.to_string()),
                    exit_code: code.as_i32(),
                    ..TransferResult::default()
                })
            }
        }
    }

    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities::default()
    }
}

impl NdEngine {
    fn effective_connect_timeout(&self) -> Duration {
        if self.connect_timeout.is_zero() {
            probe::PROBE_TIMEOUT
        } else {
            self.connect_timeout
        }
    }

    fn effective_progress_interval(&self) -> Duration {
        if self.progress_interval.is_zero() {
            Duration::from_millis(100)
        } else {
            self.progress_interval
        }
    }
}
