//! One-shot worker lifecycle: spawn, send one request, parse frames, reap.
//!
//! A fresh `python -m paradl_media` process is created for every operation.
//! Its stdout is protocol-only; its human logs go to our stderr via inherited
//! stderr. Every exit path kills and reaps the child (no orphan processes),
//! forwards SIGINT/SIGTERM, and applies an inactivity timeout.

use std::path::{Path, PathBuf};
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
            // Tokio reaps the child in the background if this future is
            // dropped before `wait()` (error frames, malformed frames, timeouts).
            .kill_on_drop(true)
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
    ///
    /// The worker validates a non-empty absolute `out_dir` on **every** op,
    /// including the read-only `claims` op (charter §4), so callers must pass a
    /// real directory here rather than an empty placeholder.
    async fn claims_with_meta(
        &self,
        url: &str,
        out_dir: &Path,
    ) -> Result<(bool, WorkerVersions, Vec<String>), Error> {
        let (versions, caps, data) = self
            .send(
                ProtocolOp::Claims,
                url,
                out_dir,
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

/// Absolute scratch directory used by the read-only [`MediaResolver::claims`]
/// API.
///
/// `claims` never writes files, but the worker validates a non-empty absolute
/// `out_dir` on every op (charter §4). We hand it the process temp directory
/// as a neutral placeholder. `std::env::temp_dir()` is absolute in normal
/// configurations; if a relative `TMPDIR` is supplied we fall back to the
/// current working directory, which is absolute by contract.
fn claims_scratch_dir() -> PathBuf {
    let temp = std::env::temp_dir();
    if temp.is_absolute() {
        temp
    } else {
        std::env::current_dir().unwrap_or(temp)
    }
}

#[async_trait]
impl MediaResolver for WorkerMediaResolver {
    fn domains(&self) -> &'static [&'static str] {
        MEDIA_DOMAINS
    }

    async fn resolve(&self, request: MediaRequest) -> Result<MediaResult, Error> {
        let (claimed, claim_versions, claim_caps) = self
            .claims_with_meta(&request.target, &request.out_dir)
            .await?;
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
        let (claimed, claim_versions, claim_caps) = self
            .claims_with_meta(&request.target, &request.out_dir)
            .await?;
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
        Ok(self.claims_with_meta(url, &claims_scratch_dir()).await?.0)
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

/// POSIX signal delivered to a worker child.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerSignal {
    /// Ctrl-C (`SIGINT`).
    Interrupt,
    /// Termination request (`SIGTERM`).
    Terminate,
}

/// Send a signal to a recorded worker pid.
///
/// This is the seam exercised by the Unix signal test: the forwarder and the
/// test call the exact same helper, so no test needs to signal its own runner.
#[cfg(unix)]
fn send_worker_signal(pid: u32, signal: WorkerSignal) -> bool {
    let number = match signal {
        WorkerSignal::Interrupt => libc::SIGINT,
        WorkerSignal::Terminate => libc::SIGTERM,
    };
    // SAFETY: `pid` is the child just spawned by `send`, and `kill` with a
    // positive pid signals only that process (never the test runner/group).
    unsafe { libc::kill(pid as libc::pid_t, number) == 0 }
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
                    send_worker_signal(pid, WorkerSignal::Interrupt);
                }
                _ = sigterm.recv() => {
                    interrupted.store(true, Ordering::Relaxed);
                    send_worker_signal(pid, WorkerSignal::Terminate);
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

#[cfg(all(test, unix))]
mod signal_tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use paradl_core::ExitCode;
    use tokio::io::AsyncBufReadExt;
    use tokio::process::Command;

    use crate::{ErrorKind, MediaResolver};

    use super::{
        KillOnDrop, WorkerFrame, WorkerLaunch, WorkerMediaResolver, WorkerSignal,
        error_kind_to_error, send_worker_signal,
    };

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paradl-media-signal-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn wait_for_marker(path: &Path) -> String {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(path).await
                    && !text.is_empty()
                {
                    return text;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child trapped the forwarded signal and wrote its marker")
    }

    async fn wait_for_reap(pid: i32) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                // SAFETY: `kill(pid, 0)` only probes the recorded child pid.
                if unsafe { libc::kill(pid, 0) } != 0 {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("child was killed and reaped (no zombie/orphan left behind)");
    }

    const INTERRUPTED_FRAME: &str = r#"{"protocol":"paradl-media/1","request_id":"signal-test","frame":"error","ok":false,"error":{"kind":"interrupted","message":"interrupted by signal"},"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"}}"#;

    #[tokio::test]
    async fn unix_forwarder_seam_signals_recorded_child_and_maps_130() {
        let dir = test_dir();
        let marker = dir.join("signal.marker");
        let part = dir.join("clip.m4a.pdl.part");
        let sidecar = dir.join("clip.m4a.pdl.json");
        std::fs::write(&part, b"partial").unwrap();
        std::fs::write(&sidecar, b"{}").unwrap();

        let script = format!(
            "trap 'printf \"INT\" > \"{}\"; exit 130' INT\nprintf '%s\\n' '{}'\nwhile :; do sleep 0.05; done\n",
            marker.display(),
            INTERRUPTED_FRAME,
        );
        let mut command = Command::new("sh");
        command
            .args(["-c", &script])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = KillOnDrop(command.spawn().unwrap());
        let pid = child.0.id().unwrap();
        let stdout = child.0.stdout.take().unwrap();
        let reader = tokio::io::BufReader::new(stdout);
        let line = tokio::time::timeout(Duration::from_secs(2), reader.lines().next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("stub emitted its terminal error frame");

        let WorkerFrame::Error { kind, message, .. } = WorkerFrame::parse(&line, "signal-test").unwrap()
        else {
            panic!("expected an interrupted terminal frame");
        };
        assert_eq!(kind, ErrorKind::Interrupted);
        assert_eq!(
            error_kind_to_error(kind.as_str(), message).exit_code(),
            ExitCode::Interrupted,
        );

        // The same helper used by the real SIGINT/SIGTERM forwarder, but with
        // the recorded child pid: the test runner is never signalled.
        assert!(send_worker_signal(pid, WorkerSignal::Interrupt));
        assert_eq!(wait_for_marker(&marker).await, "INT");
        let status = tokio::time::timeout(Duration::from_secs(2), child.0.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(130));
        assert!(part.exists(), ".pdl.part must survive SIGINT");
        assert!(sidecar.exists(), ".pdl.json must survive SIGINT");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unix_forwarder_seam_also_forwards_sigterm() {
        let dir = test_dir();
        let marker = dir.join("term.marker");
        let ready = dir.join("term.ready");
        let script = format!(
            "trap 'printf \"TERM\" > \"{}\"; exit 0' TERM\nprintf 'ready' > \"{}\"\nwhile :; do sleep 0.05; done\n",
            marker.display(),
            ready.display(),
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]).kill_on_drop(true);
        let mut child = KillOnDrop(command.spawn().unwrap());
        let pid = child.0.id().unwrap();
        // Wait until the child has installed its trap before signalling it.
        assert_eq!(wait_for_marker(&ready).await, "ready");
        assert!(send_worker_signal(pid, WorkerSignal::Terminate));
        assert_eq!(wait_for_marker(&marker).await, "TERM");
        let _ = tokio::time::timeout(Duration::from_secs(2), child.0.wait())
            .await
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn malformed_frame_return_reaps_child_without_zombie() {
        let dir = test_dir();
        let pidfile = dir.join("worker.pid");
        let script = format!(
            "printf '%s\\n' \"$$\" > \"{}\"\nprintf 'this-is-not-json\\n'\nwhile :; do sleep 0.05; done\n",
            pidfile.display(),
        );
        let resolver = WorkerMediaResolver::with_launch(WorkerLaunch::new(
            "sh",
            ["-c".to_owned(), script],
        ));
        let error = resolver.claims("https://example.com/x").await.unwrap_err();
        assert_eq!(error.exit_code().as_i32(), 1, "malformed frame fails closed");

        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for_reap(pid).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod claims_scratch_tests {
    use super::claims_scratch_dir;

    #[test]
    fn claims_scratch_dir_is_absolute_and_nonempty() {
        let dir = claims_scratch_dir();
        assert!(
            dir.is_absolute(),
            "claims out_dir must be absolute: {}",
            dir.display()
        );
        assert!(
            !dir.as_os_str().is_empty(),
            "claims out_dir must be non-empty"
        );
    }
}
