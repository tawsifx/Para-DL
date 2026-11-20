//! One-shot worker lifecycle: spawn, send one request, parse frames, reap.
//!
//! A fresh `python -m paradl_media` process is created for every operation.
//! Its stdout is protocol-only; its human logs go to our stderr via inherited
//! stderr. Every exit path kills and reaps the child (no orphan processes),
//! forwards SIGINT/SIGTERM, and applies an inactivity timeout.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use paradl_core::{Error, MEDIA_DOMAINS};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::discovery::WorkerLaunch;
use crate::progress::{NoopProgressSink, ProgressSink, reemit_progress};
use crate::protocol::{
    MediaData, MediaOptions, MediaRequest, MediaResult, ProtocolOp, ProtocolRequest,
    WorkerFrame, WorkerVersions, error_kind_to_error,
};
use crate::router::MediaRoute;
use crate::MediaResolver;

/// Default inactivity timeout: reset on every worker output line.
pub const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(300);

/// Grace period given to a worker to exit after its terminal frame.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// Concrete `MediaResolver` backed by one-shot Python worker processes.
///
/// ```
/// use paradl_media::{WorkerLaunch, WorkerMediaResolver};
/// let resolver = WorkerMediaResolver::with_launch(
///     WorkerLaunch::new("sh", ["-c", "cat >/dev/null"]),
/// );
/// assert_eq!(resolver.timeout().as_secs(), 300);
/// ```
#[derive(Debug, Clone)]
pub struct WorkerMediaResolver {
    launch: Option<WorkerLaunch>,
    timeout: Duration,
}

impl Default for WorkerMediaResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerMediaResolver {
    /// Create a resolver that discovers the worker Python on each operation.
    pub fn new() -> Self {
        Self {
            launch: None,
            timeout: DEFAULT_WORKER_TIMEOUT,
        }
    }

    /// Create a resolver with an explicit launch command (also used by tests).
    pub fn with_launch(launch: WorkerLaunch) -> Self {
        Self {
            launch: Some(launch),
            timeout: DEFAULT_WORKER_TIMEOUT,
        }
    }

    /// Set the per-line inactivity timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Return the configured inactivity timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Run one operation and return the terminal frame's versions/caps/data.
    async fn send(
        &self,
        op: ProtocolOp,
        target: &str,
        out_dir: &Path,
        options: &MediaOptions,
        sink: &mut dyn ProgressSink,
    ) -> Result<(WorkerVersions, Vec<String>, Value), Error> {
        let request_id = Uuid::new_v4().to_string();
        let request = ProtocolRequest::new(
            op,
            &MediaRequest {
                target: target.to_owned(),
                out_dir: out_dir.to_path_buf(),
                options: options.clone(),
            },
            &request_id,
        );
        let line = serde_json::to_string(&request)
            .map_err(|error| Error::failure(format!("cannot serialize media request: {error}")))?;

        let launch = match &self.launch {
            Some(launch) => launch.clone(),
            None => WorkerLaunch::discovered()?,
        };
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // Worker human logs belong on stderr; stdout is protocol-only.
            .stderr(std::process::Stdio::inherit());
        let child = command.spawn().map_err(|error| {
            Error::missing_dependency(format!(
                "cannot start media worker {}: {error}",
                launch.program.display()
            ))
        })?;
        let mut child = KillOnDrop(child);
        let pid = child.0.id();
        let interrupted = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let signals = SignalForwarder::start(pid, interrupted.clone(), shutdown_tx, shutdown_rx);

        let mut stdin = child
            .0
            .stdin
            .take()
            .ok_or_else(|| Error::failure("media worker stdin was not piped"))?;
        let write_result = stdin.write_all(format!("{line}\n").as_bytes()).await;
        drop(stdin); // EOF tells the one-shot worker the request is complete.
        if let Err(error) = write_result {
            return Err(Error::failure(format!(
                "cannot send request to media worker: {error}"
            )));
        }

        let stdout = child
            .0
            .stdout
            .take()
            .ok_or_else(|| Error::failure("media worker stdout was not piped"))?;
        let mut lines = BufReader::new(stdout).lines();

        let terminal = loop {
            let next = match tokio::time::timeout(self.timeout, lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => {
                    if interrupted.load(Ordering::Relaxed) {
                        return Err(Error::interrupted("media worker interrupted"));
                    }
                    return Err(Error::failure(
                        "media worker exited without a terminal frame",
                    ));
                }
                Ok(Err(error)) => {
                    return Err(Error::failure(format!(
                        "cannot read media worker stdout: {error}"
                    )));
                }
                Err(_) => {
                    return Err(Error::failure(format!(
                        "media worker timed out after {:?} without output",
                        self.timeout
                    )));
                }
            };
            if next.trim().is_empty() {
                continue;
            }
            match WorkerFrame::parse(&next, &request_id)? {
                WorkerFrame::Progress(progress) => reemit_progress(&progress, target, sink),
                WorkerFrame::Result {
                    versions,
                    caps,
                    data,
                } => break (versions, caps, data),
                WorkerFrame::Error { kind, message, .. } => {
                    return Err(error_kind_to_error(kind.as_str(), message));
                }
            }
        };

        drop(signals);
        if tokio::time::timeout(EXIT_GRACE, child.0.wait()).await.is_err() {
            // `KillOnDrop` reaps on every path; this only bounds the grace wait.
            let _ = child.0.try_wait();
        }
        Ok(terminal)
    }

    /// Ask the worker for claims plus the metadata from its terminal frame.
    async fn claims_with_meta(
        &self,
        url: &str,
    ) -> Result<(bool, WorkerVersions, Vec<String>), Error> {
        let (versions, caps, data) = self
            .send(
                ProtocolOp::Claims,
                url,
                Path::new(""),
                &MediaOptions::default(),
                &mut NoopProgressSink,
            )
            .await?;
        let claimed = data
            .get("claims")
            .and_then(Value::as_bool)
            .or_else(|| data.as_bool())
            .ok_or_else(|| Error::failure("claims result missing boolean `claims`"))?;
        Ok((claimed, versions, caps))
    }

    /// Finalize worker-owned tags/parts after Rust-owned checksum verification.
    ///
    /// Checksum and archive security remain in `paradl-postproc`; this only
    /// forwards the `finalize` operation of the worker protocol.
    pub async fn finalize(&self, request: MediaRequest) -> Result<MediaResult, Error> {
        let (versions, caps, data) = self
            .send(
                ProtocolOp::Finalize,
                &request.target,
                &request.out_dir,
                &request.options,
                &mut NoopProgressSink,
            )
            .await?;
        Ok(MediaResult {
            route: MediaRoute::Managed,
            data: Some(MediaData::from_value(&data)?),
            versions,
            caps,
        })
    }
}

#[async_trait]
impl MediaResolver for WorkerMediaResolver {
    fn domains(&self) -> &'static [&'static str] {
        MEDIA_DOMAINS
    }

    async fn resolve(&self, request: MediaRequest) -> Result<MediaResult, Error> {
        let (claimed, claim_versions, claim_caps) = self.claims_with_meta(&request.target).await?;
        if !claimed {
            return Ok(MediaResult {
                route: MediaRoute::Direct,
                data: None,
                versions: claim_versions,
                caps: claim_caps,
            });
        }
        let (versions, caps, data) = self
            .send(
                ProtocolOp::Resolve,
                &request.target,
                &request.out_dir,
                &request.options,
                &mut NoopProgressSink,
            )
            .await?;
        Ok(MediaResult {
            route: MediaRoute::Managed,
            data: Some(MediaData::from_value(&data)?),
            versions,
            caps,
        })
    }

    async fn download_managed(
        &self,
        request: MediaRequest,
        sink: &mut dyn ProgressSink,
    ) -> Result<MediaResult, Error> {
        let (claimed, claim_versions, claim_caps) = self.claims_with_meta(&request.target).await?;
        if !claimed {
            return Ok(MediaResult {
                route: MediaRoute::Direct,
                data: None,
                versions: claim_versions,
                caps: claim_caps,
            });
        }
        let (versions, caps, data) = self
            .send(
                ProtocolOp::DownloadManaged,
                &request.target,
                &request.out_dir,
                &request.options,
                sink,
            )
            .await?;
        Ok(MediaResult {
            route: MediaRoute::Managed,
            data: Some(MediaData::from_value(&data)?),
            versions,
            caps,
        })
    }

    async fn claims(&self, url: &str) -> Result<bool, Error> {
        Ok(self.claims_with_meta(url).await?.0)
    }
}

/// Kill and reap the child on every exit path, including `?` early returns.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
        let _ = self.0.try_wait();
    }
}

/// Stops the signal task when an operation finishes (success or failure).
struct SignalForwarder(Option<oneshot::Sender<()>>, JoinHandle<()>);

impl SignalForwarder {
    fn start(
        pid: Option<u32>,
        interrupted: Arc<AtomicBool>,
        shutdown_tx: oneshot::Sender<()>,
        shutdown_rx: oneshot::Receiver<()>,
    ) -> Self {
        Self(
            Some(shutdown_tx),
            spawn_signal_forwarder(pid, interrupted, shutdown_rx),
        )
    }
}

impl Drop for SignalForwarder {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
        self.1.abort();
    }
}

#[cfg(unix)]
fn spawn_signal_forwarder(
    pid: Option<u32>,
    interrupted: Arc<AtomicBool>,
    mut shutdown: oneshot::Receiver<()>,
) -> JoinHandle<()> {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async move {
        let Some(pid) = pid else { return };
        let (mut sigint, mut sigterm) = match (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) {
            (Ok(int), Ok(term)) => (int, term),
            _ => return,
        };
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = sigint.recv() => {
                    interrupted.store(true, Ordering::Relaxed);
                    // SAFETY: `kill` only signals the child pid we just spawned.
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT); }
                }
                _ = sigterm.recv() => {
                    interrupted.store(true, Ordering::Relaxed);
                    // SAFETY: `kill` only signals the child pid we just spawned.
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
                }
            }
        }
    })
}

#[cfg(not(unix))]
fn spawn_signal_forwarder(
    _pid: Option<u32>,
    _interrupted: Arc<AtomicBool>,
    mut shutdown: oneshot::Receiver<()>,
) -> JoinHandle<()> {
    // Windows has no POSIX signals; the worker receives the console control
    // event directly and reports `error.kind=interrupted` (exit 130).
    tokio::spawn(async move {
        let _ = shutdown.await;
    })
}
