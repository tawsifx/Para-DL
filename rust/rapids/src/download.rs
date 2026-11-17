// Modifications: the original engine's 5731-line download.rs is NOT copied. This is a small focused Tokio
// worker pool driven by the vendored planner. The URI pool is created once from the probe's
// post-redirect URL and is never rebuilt; `recycle_connections` cancels workers, releases
// uncommitted claims, and reassigns from committed offsets over the SAME pool. This is the
// in-process replacement for aria2's fragile force_pause/unpause, which lost redirected URIs
// (docs/diagnostics/watchdog-cycle-aria2-bug.md).

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::header::{COOKIE, IF_RANGE, RANGE};
use reqwest::StatusCode;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};
use url::Url;

use crate::error::{Error, Result};
use crate::limiter::Limiter;
use crate::naming::part_path;
use crate::planner::{Assignment, Claim, Planner, SavedSegment, WorkerId};
use crate::probe::{self, ProbeInfo};
use crate::state::{ResumeState, state_path};
use crate::verify::{Hash, matches};

const DEFAULT_MIN_SPLIT: u64 = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Test-only override for the per-chunk read timeout. Production keeps 30 s; tests
/// shorten it to prove a stalled close-delimited stream is an error, not a false EOF.
/// `#[cfg(test)]` keeps this out of the public `StartOptions` API and production builds.
#[cfg(test)]
static TEST_READ_TIMEOUT: AtomicU64 = AtomicU64::new(0);

/// Set the test read timeout. Never zero: zero means "use the production 30 s value".
#[cfg(test)]
pub(crate) fn set_test_read_timeout(timeout: Duration) {
    TEST_READ_TIMEOUT.store(timeout.as_millis() as u64, Ordering::Relaxed);
}

fn read_timeout() -> Duration {
    #[cfg(test)]
    {
        let millis = TEST_READ_TIMEOUT.load(Ordering::Relaxed);
        if millis > 0 {
            return Duration::from_millis(millis);
        }
    }
    READ_TIMEOUT
}

const MAX_RETRIES: u32 = 3;
const COMMANDS: usize = 16;
/// Periodic crash-safe sidecar save interval (the original: "several times per minute").
const SAVE_INTERVAL: Duration = Duration::from_secs(2);
/// Sentinel planner size for a single-stream download with no known Content-Length
/// (the GitHub -> codeload case in docs/diagnostics/watchdog-cycle-aria2-bug.md).
const UNKNOWN_SIZE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Queued,
    Probing,
    Downloading,
    Paused,
    Verifying,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Probing => "probing",
            State::Downloading => "downloading",
            State::Paused => "paused",
            State::Verifying => "verifying",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }
}

/// A point-in-time view for progress displays (Python `poll_snapshot()` -> dict).
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub state: State,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub speed_bps: u64,
    pub connections: usize,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
}

/// Final result of `wait()` (mapped to `TransferResult` by the Python adapter).
#[derive(Clone, Debug)]
pub struct Outcome {
    pub success: bool,
    pub error: Option<String>,
    pub path: Option<PathBuf>,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Options for `start_download` / `resume_download`.
#[derive(Clone, Debug)]
pub struct StartOptions {
    pub url: Url,
    pub mirrors: Vec<Url>,
    pub dest: PathBuf,
    pub connections: u32,
    pub headers: reqwest::header::HeaderMap,
    pub checksum: Option<Hash>,
    pub rate_limit_bps: Option<u64>,
    pub min_split: u64,
    pub resume: bool,
    pub proxy: Option<String>,
    pub ip_family: Option<String>,
    /// Raw Cookie header from the browser.
    pub cookie_header: Option<String>,
    /// Exact hosts the cookie jar was captured for. A cookie is sent ONLY to one of these
    /// hosts — never to a cross-domain redirect target or mirror.
    pub cookie_hosts: Vec<String>,
}

impl Default for StartOptions {
    fn default() -> Self {
        StartOptions {
            url: Url::parse("http://localhost/").unwrap(),
            mirrors: Vec::new(),
            dest: PathBuf::from("download"),
            connections: 8,
            headers: reqwest::header::HeaderMap::new(),
            checksum: None,
            rate_limit_bps: None,
            min_split: DEFAULT_MIN_SPLIT,
            resume: false,
            proxy: None,
            ip_family: None,
            cookie_header: None,
            cookie_hosts: Vec::new(),
        }
    }
}

/// URI pool built once and never dropped by recycle (the aria2-bug fix). The URL list
/// itself is immutable; only the per-URI health flags change when a 5xx is seen, so a
/// failed primary/mirror is skipped on subsequent requests (aria2 mirror failover parity).
#[derive(Debug)]
pub struct UriPool {
    urls: Vec<Url>,
    next: AtomicUsize,
    failed: Mutex<Vec<bool>>,
}

impl UriPool {
    pub fn new(urls: Vec<Url>) -> Result<Self> {
        if urls.is_empty() {
            return Err(Error::NoUri);
        }
        let failed = vec![false; urls.len()];
        Ok(UriPool {
            urls,
            next: AtomicUsize::new(0),
            failed: Mutex::new(failed),
        })
    }

    /// Round-robin pick that skips URIs already marked failed. If every URI has failed,
    /// the next one is returned anyway so workers fail fast and the supervisor can end
    /// the download with an error (generic failure parity) instead of hanging.
    pub fn next(&self) -> Url {
        let failed = self.failed.lock().unwrap();
        let n = self.urls.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % n;
        for i in 0..n {
            let idx = (start + i) % n;
            if !failed[idx] {
                return self.urls[idx].clone();
            }
        }
        self.urls[start].clone()
    }

    /// Mark a URI unhealthy after a 5xx so all workers fail over to a healthy mirror.
    pub fn mark_failed(&self, url: &Url) {
        let mut failed = self.failed.lock().unwrap();
        if let Some(i) = self.urls.iter().position(|u| u == url) {
            failed[i] = true;
        }
    }

    pub fn all_failed(&self) -> bool {
        self.failed.lock().unwrap().iter().all(|f| *f)
    }

    /// A stable snapshot (strings) for tests and introspection — includes failed URIs.
    pub fn urls(&self) -> Vec<String> {
        self.urls.iter().map(|u| u.to_string()).collect()
    }

    pub fn len(&self) -> usize {
        self.urls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }
}

struct StateInfo {
    state: State,
    error: Option<String>,
}

#[derive(Debug)]
struct SpeedMeter {
    last_bytes: u64,
    last_at: Instant,
    bps: u64,
}

impl SpeedMeter {
    fn new() -> Self {
        SpeedMeter {
            last_bytes: 0,
            last_at: Instant::now(),
            bps: 0,
        }
    }
    fn sample(&mut self, bytes: u64) -> u64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).as_secs_f64();
        if dt >= 0.1 && bytes >= self.last_bytes {
            self.bps = ((bytes - self.last_bytes) as f64 / dt) as u64;
            self.last_bytes = bytes;
            self.last_at = now;
        }
        self.bps
    }
}

struct Shared {
    planner: Mutex<Planner>,
    progress: AtomicU64,
    info: Mutex<StateInfo>,
    speed: Mutex<SpeedMeter>,
    limiter: Limiter,
    uris: Arc<UriPool>,
    client: reqwest::Client,
    headers: reqwest::header::HeaderMap,
    /// Host the caller's sensitive headers were captured for. They are sent only
    /// to this host — never to a cross-host redirect target or a mirror.
    header_host: Option<String>,
    /// Cookie header value and the exact hosts it was captured for.
    cookie_header: Option<String>,
    cookie_hosts: Vec<String>,
    /// Once an unknown-size stream has reached EOF, the final size is known from progress.
    finished: AtomicBool,
    /// Last committed byte, used by the supervisor's stall detector (not a blind timeout).
    last_progress: Mutex<Instant>,
    part: std::fs::File,
    part_path: PathBuf,
    dest: PathBuf,
    resume_state: Mutex<Option<ResumeState>>,
    min_split: u64,
    connections: u32,
    /// Unknown total size: single plain GET, no Range, total reported as None (no false 100%).
    unknown_size: bool,
    last_save: Mutex<Instant>,
    paused: watch::Sender<bool>,
    /// Cancellation flag (watch channel instead of an extra tokio-util dependency).
    cancel_tx: watch::Sender<bool>,
}

impl Shared {
    fn set_state(&self, state: State) {
        self.info.lock().unwrap().state = state;
    }
    fn set_error(&self, err: impl Into<String>) {
        self.info.lock().unwrap().error = Some(err.into());
    }
    fn snapshot(&self) -> Snapshot {
        let info = self.info.lock().unwrap();
        let planner = self.planner.lock().unwrap();
        let downloaded = self.progress.load(Ordering::Relaxed);
        let total = if self.unknown_size {
            // No fake total while streaming. At EOF the real size is known, so report it
            // (the parity tests assert total == len(payload) after completion).
            if self.finished.load(Ordering::Relaxed) {
                Some(downloaded)
            } else {
                None
            }
        } else if planner.size() > 0 {
            Some(planner.size())
        } else {
            None
        };
        let speed_bps = self.speed.lock().unwrap().sample(downloaded);
        Snapshot {
            state: info.state,
            downloaded,
            total,
            speed_bps,
            connections: planner.active_workers(),
            error: info.error.clone(),
            path: Some(self.dest.clone()),
        }
    }

    /// Crash-safe periodic sidecar write: `ResumeState::save` fsyncs the part first, then
    /// atomically renames the sidecar, so a saved cursor never claims bytes not on disk.
    fn maybe_save(&self) {
        let now = Instant::now();
        {
            let mut last = self.last_save.lock().unwrap();
            if now.duration_since(*last) < SAVE_INTERVAL {
                return;
            }
            *last = now;
        }
        let saved = self.planner.lock().unwrap().save();
        if let Some(rs) = self.resume_state.lock().unwrap().as_mut() {
            let p = state_path(&self.dest);
            let _ = rs.save(&p, saved, &self.part);
        }
    }
}

#[allow(dead_code)] // payloads document the end condition; supervisor only needs the variant
#[derive(Debug)]
enum WorkerEnd {
    SegmentDone(u64),
    NeedWork,
    /// The plain (no-Range) stream ended: an unknown-size download is complete.
    StreamEnded(u64),
    Failed(String),
}

/// Write `buf` at `offset` in the (sparse) part file. `pwrite` on Unix avoids seeks racing
/// between workers; the portable fallback clones the descriptor (independent file cursor).
fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(buf, offset)
    }
    #[cfg(not(unix))]
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = file.try_clone()?;
        f.seek(SeekFrom::Start(offset))?;
        f.write_all(buf)
    }
}

fn planner_downloaded(shared: &Shared) -> u64 {
    shared.planner.lock().unwrap().downloaded()
}

async fn wait_until_running(shared: &Shared, rx: &mut watch::Receiver<bool>) -> Result<()> {
    loop {
        if *shared.cancel_tx.borrow() {
            return Err(Error::Cancelled);
        }
        if !*rx.borrow() {
            return Ok(());
        }
        if rx.changed().await.is_err() {
            return Err(Error::Cancelled);
        }
    }
}

/// Fetch one segment, committing bytes at their exact offsets. Returns when the segment
/// is done, when the assignment changed under it (split/recycle), or on a network error.
async fn worker(shared: Arc<Shared>, w: WorkerId, assignment: Assignment) -> WorkerEnd {
    let mut pos = assignment.from;
    let mut retries = 0u32;
    // Read timeouts are retried from the committed offset, but must not be reset by a
    // successful reconnect (unlike `retries`, which counts failed GETs). Otherwise a
    // permanently stalled stream would reconnect forever and never become a failure.
    let mut read_timeouts = 0u32;
    let mut paused_rx = shared.paused.subscribe();
    // Unknown-size downloads use a single plain GET with no Range at all.
    let mut stream_plain = shared.unknown_size;

    loop {
        if let Err(e) = wait_until_running(&shared, &mut paused_rx).await {
            return WorkerEnd::Failed(e.to_string());
        }

        // How far may we fetch from `pos`? The segment end moves down when another worker
        // splits it, so always ask the planner rather than trusting `assignment.to`.
        let (room, end) = {
            let p = shared.planner.lock().unwrap();
            match p.room(w, pos) {
                Some(r) => (r, p.segments().iter().find(|s| s.worker == Some(w)).map(|s| s.end).unwrap_or(pos + r)),
                None => return WorkerEnd::NeedWork,
            }
        };
        if !stream_plain && (room == 0 || pos >= end) {
            return WorkerEnd::SegmentDone(end);
        }
        let want_end = (pos + room.saturating_sub(1)).min(end.saturating_sub(1));

        let uri = shared.uris.next(); // rotate across mirrors / the pool on each request
        // These are DIRECT requests (no redirect for reqwest to strip on): scope
        // the caller's sensitive headers to the original host on every request.
        let headers = probe::scope_headers(&shared.headers, shared.header_host.as_deref(), uri.host_str());
        let mut req = shared.client.get(uri.clone()).headers(headers);
        if !stream_plain {
            req = req.header(RANGE, format!("bytes={pos}-{want_end}"));
        }
        // Host-scoped cookies: a cross-domain mirror or post-redirect host must never
        // receive the browser's cookie for the original host.
        if let (Some(cookie), Some(host)) = (&shared.cookie_header, uri.host_str())
            && shared.cookie_hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
        {
            req = req.header(COOKIE, cookie.as_str());
        }
        if assignment.written > 0 && !shared.unknown_size {
            let if_range = {
                let rs = shared.resume_state.lock().unwrap();
                rs.as_ref()
                    .and_then(|s| s.validators.if_range(s.final_url.host_str()))
                    .map(str::to_owned)
            };
            if let Some(v) = if_range {
                req = req.header(IF_RANGE, v);
            }
        }

        let resp = match tokio::time::timeout(CONNECT_TIMEOUT, req.send()).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                retries += 1;
                if retries > MAX_RETRIES {
                    return WorkerEnd::Failed(e.to_string());
                }
                continue;
            }
            Err(_) => {
                retries += 1;
                if retries > MAX_RETRIES {
                    return WorkerEnd::Failed("connect timeout".into());
                }
                continue;
            }
        };

        if resp.status().is_server_error() {
            // Mirror failover: a failing primary/mirror is removed from the rotation and
            // every worker immediately re-requests from a healthy URI at its committed
            // offset. If every URI is down, fail the download (generic-failure parity).
            shared.uris.mark_failed(&uri);
            if shared.uris.all_failed() {
                return WorkerEnd::Failed(format!("HTTP {} from all URIs", resp.status().as_u16()));
            }
            return WorkerEnd::NeedWork;
        }
        if resp.status() == StatusCode::OK && !stream_plain {
            // Server ignored Range (e.g. a 302 to a host without Accept-Ranges). Hand the
            // whole file to the connection streaming from byte 0 and stop splitting.
            let took_over = {
                let mut p = shared.planner.lock().unwrap();
                pos == 0 && p.make_single(w)
            };
            if !took_over {
                return WorkerEnd::Failed("server ignores ranges on a non-zero segment".into());
            }
            pos = 0; // re-stream from byte 0 of the file the server actually returned
            stream_plain = true;
        } else if resp.status() != StatusCode::PARTIAL_CONTENT && resp.status() != StatusCode::OK {
            return WorkerEnd::Failed(format!("unexpected HTTP {}", resp.status().as_u16()));
        }
        retries = 0;

        let mut stream = resp.bytes_stream();
        let mut short = false;
        // A plain GET always returns the whole body from byte 0. Discard the prefix that is
        // already committed so a reconnect (after a recycle or a bounded window) resumes at
        // the right offset instead of overwriting earlier bytes with the wrong data.
        let mut skip = if stream_plain { pos } else { 0 };
        loop {
            // A read timeout (Err(Elapsed)) is NOT the same as EOF (Ok(None)). Collapsing
            // both into `None` used to end a stalled close-delimited stream with
            // StreamEnded, which the supervisor mistook for a completed unknown-size
            // download and renamed a truncated file into place.
            let chunk = match tokio::time::timeout(read_timeout(), stream.next()).await {
                Ok(Some(Ok(c))) => c,
                Ok(Some(Err(e))) => {
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return WorkerEnd::Failed(e.to_string());
                    }
                    short = true;
                    break;
                }
                Ok(None) => break, // genuine EOF: the only clean end of a stream
                Err(_) => {
                    // Bounded retry from the committed offset: a timeout is a short read,
                    // never a finished stream. Exhausting retries fails the worker so the
                    // supervisor records the error instead of declaring false success.
                    read_timeouts += 1;
                    if read_timeouts > MAX_RETRIES {
                        return WorkerEnd::Failed(format!("read timeout after {pos} bytes"));
                    }
                    short = true;
                    break;
                }
            };
            if chunk.is_empty() {
                continue;
            }
            if skip > 0 {
                let drop = skip.min(chunk.len() as u64) as usize;
                skip -= drop as u64;
                if drop == chunk.len() {
                    continue;
                }
                // the rest of this chunk begins exactly at `pos`
                let rest = &chunk[drop..];
                let granted = shared.planner.lock().unwrap().claim(w, pos, rest.len() as u64);
                let n = match granted {
                    Claim::Granted(n) => n as usize,
                    Claim::Lost => return WorkerEnd::NeedWork,
                };
                if n == 0 {
                    short = true;
                    break;
                }
                shared.limiter.acquire(n as u64).await;
                if let Err(e) = write_at(&shared.part, &rest[..n], pos) {
                    return WorkerEnd::Failed(e.to_string());
                }
                let end = pos + n as u64;
                let done = shared.planner.lock().unwrap().commit(w, end);
                shared.progress.fetch_add(n as u64, Ordering::Relaxed);
                *shared.last_progress.lock().unwrap() = Instant::now();
                pos = end;
                shared.maybe_save();
                if let Some(end) = done {
                    return WorkerEnd::SegmentDone(end);
                }
                continue;
            }
            let granted = shared.planner.lock().unwrap().claim(w, pos, chunk.len() as u64);
            let n = match granted {
                Claim::Granted(n) => n as usize,
                Claim::Lost => return WorkerEnd::NeedWork,
            };
            if n == 0 {
                short = true;
                break;
            }
            shared.limiter.acquire(n as u64).await;
            let end = pos + n as u64;
            if let Err(e) = write_at(&shared.part, &chunk[..n], pos) {
                return WorkerEnd::Failed(e.to_string());
            }
            let done = shared.planner.lock().unwrap().commit(w, end);
            shared.progress.fetch_add(n as u64, Ordering::Relaxed);
                *shared.last_progress.lock().unwrap() = Instant::now();
            pos = end;
            shared.maybe_save(); // crash-safe progress, throttled to SAVE_INTERVAL
            if n < chunk.len() {
                // A split cut this chunk short; the remainder is re-requested from `pos`.
                short = true;
                break;
            }
            if let Some(end) = done {
                return WorkerEnd::SegmentDone(end);
            }
        }
        if short {
            // Fault injection / server close: go around and re-request from the committed
            // offset. `release` is not needed: claims beyond the cursor are reset by assign().
            continue;
        }
        if shared.unknown_size && stream_plain {
            // The plain stream reached EOF: an unknown-size download is complete.
            return WorkerEnd::StreamEnded(pos);
        }
        // Stream exhausted without filling the assignment; re-request from current pos.
    }
}

#[derive(Debug)]
enum Command {
    Recycle,
    Pause,
    Resume,
    Cancel,
    SetLimit(Option<u64>),
}

fn spawn_worker(set: &mut JoinSet<(WorkerId, WorkerEnd)>, shared: &Arc<Shared>, w: WorkerId) {
    let assignment = shared.planner.lock().unwrap().assign(w, None);
    if let Some(a) = assignment {
        let s = shared.clone();
        set.spawn(async move {
            let end = worker(s, w, a).await;
            (w, end)
        });
    }
}

async fn supervise(
    shared: Arc<Shared>,
    mut cmds: mpsc::Receiver<Command>,
) -> Result<Outcome> {
    let n = shared.connections as WorkerId;
    let mut set: JoinSet<(WorkerId, WorkerEnd)> = JoinSet::new();
    let mut failures: HashMap<WorkerId, u32> = HashMap::new();
    for w in 0..n {
        spawn_worker(&mut set, &shared, w);
    }
    // Liveness watchdog: if every worker retires while the planner is still incomplete,
    // the JoinSet empties and `cmds.recv()` would otherwise block forever (the caller
    // still holds a Sender). A periodic tick detects that state and fails the download.
    let mut tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            Some(cmd) = cmds.recv() => {
                match cmd {
                    Command::Recycle => {
                        // 1. Cancel in-flight worker HTTP requests.
                        set.abort_all();
                        while set.join_next().await.is_some() {}
                        // 2. Drop uncommitted claims. Committed cursors stay on disk.
                        {
                            let mut p = shared.planner.lock().unwrap();
                            for w in 0..n { p.release(w); }
                        }
                        failures.clear();
                        // 3. Reassign the remainder over the SAME URI pool with fresh TCP
                        //    connections. No URI list is rebuilt or re-derived from a
                        //    daemon-held GID — this is the aria2-bug fix.
                        for w in 0..n {
                            spawn_worker(&mut set, &shared, w);
                        }
                    }
                    Command::Pause => {
                        shared.set_state(State::Paused);
                        let _ = shared.paused.send(true);
                    }
                    Command::Resume => {
                        let _ = shared.paused.send(false);
                        shared.set_state(State::Downloading);
                    }
                    Command::Cancel => {
                        let _ = shared.cancel_tx.send(true);
                        set.abort_all();
                        shared.set_state(State::Cancelled);
                        return Err(Error::Cancelled);
                    }
                    Command::SetLimit(rate) => shared.limiter.set(rate),
                }
            }
            Some(res) = set.join_next(), if !set.is_empty() => {
                // A task can also finish with a JoinError (panic/abort); ignore it and let
                // the liveness tick detect an empty set if nothing else can make progress.
                let Ok((w, end)) = res else { continue };
                if matches!(end, WorkerEnd::StreamEnded(_)) {
                    break; // plain stream at EOF: unknown-size download finished
                }
                if let WorkerEnd::Failed(msg) = &end
                    && shared.unknown_size
                {
                    // A single plain stream has no peer to take over. Any terminal worker
                    // failure (read timeout, exhausted retries, unexpected status) is fatal:
                    // record it so the final guard fails instead of renaming truncated bytes.
                    shared.set_error(msg);
                    break;
                }
                if !shared.unknown_size && shared.planner.lock().unwrap().is_complete() {
                    break;
                }
                let failed = matches!(end, WorkerEnd::Failed(_));
                if failed {
                    let count = failures.entry(w).or_default();
                    *count += 1;
                    if *count > MAX_RETRIES * 2 {
                        // THE 32 MiB BUG: a retired worker used to leave its planner segment
                        // assigned, so its uncommitted bytes were orphaned forever while
                        // `progress` still looked like 100%. Release the segment and start a
                        // replacement rather than losing those bytes.
                        failures.remove(&w);
                        shared.planner.lock().unwrap().release(w);
                        if shared.uris.all_failed() {
                            shared.set_error("all URIs failed");
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        spawn_worker(&mut set, &shared, w);
                        continue;
                    }
                } else {
                    failures.remove(&w);
                }
                spawn_worker(&mut set, &shared, w);
            }
            _ = tick.tick() => {
                if !shared.unknown_size && shared.planner.lock().unwrap().is_complete() {
                    break;
                }
                // A genuine stall (no committed byte for 30 s) is the only reason to fail
                // here — an empty JoinSet is not, because workers may have just retired
                // with segments that still need to be handed to replacements.
                if shared.last_progress.lock().unwrap().elapsed() > Duration::from_secs(30) {
                    shared.set_error("transfer stalled: no committed bytes for 30s");
                    break;
                }
                if set.is_empty() {
                    // Release any orphaned assignments and restart the pool.
                    {
                        let mut p = shared.planner.lock().unwrap();
                        for w in 0..n {
                            p.release(w);
                        }
                    }
                    for w in 0..n {
                        spawn_worker(&mut set, &shared, w);
                    }
                }
            }
            else => break,
        }
    }
    set.abort_all();

    let downloaded = if shared.unknown_size {
        shared.progress.load(Ordering::Relaxed)
    } else {
        planner_downloaded(&shared)
    };
    let total = if shared.unknown_size {
        downloaded
    } else {
        shared.planner.lock().unwrap().size()
    };

    if !shared.unknown_size && !shared.planner.lock().unwrap().is_complete() {
        let err = shared
            .info
            .lock()
            .unwrap()
            .error
            .clone()
            .unwrap_or_else(|| "all workers stopped before completion".into());
        shared.set_error(&err);
        shared.set_state(State::Failed);
        return Err(Error::Network(err));
    }
    if !shared.unknown_size && total > 0 && downloaded != total {
        let err = format!("size mismatch: {downloaded} of {total} bytes");
        shared.set_error(&err);
        shared.set_state(State::Failed);
        return Err(Error::Verification(err));
    }
    // An unknown-size supervisor can leave the loop for reasons other than genuine EOF:
    // "all URIs failed", "transfer stalled", or a terminal worker failure. Those paths
    // already called set_error(); without this guard they were ignored and a truncated
    // part file was renamed into place and reported as success. Genuine EOF leaves no
    // recorded error, so the normal unknown-size completion path is unchanged.
    if shared.unknown_size {
        let recorded = shared.info.lock().unwrap().error.clone();
        if let Some(err) = recorded {
            shared.set_state(State::Failed);
            return Err(Error::Network(err));
        }
    }

    shared.set_state(State::Verifying);
    // Persist the final segment table before renaming the part file into place.
    {
        let saved: Vec<SavedSegment> = shared.planner.lock().unwrap().save();
        if let Some(rs) = shared.resume_state.lock().unwrap().as_mut() {
            let p = state_path(&shared.dest);
            let _ = rs.save(&p, saved, &shared.part);
        }
    }

    // F1: verify the PART file (where the bytes actually are), THEN rename. Verifying
    // `dest` before the rename either fails on a fresh download or hashes a stale file.
    if let Some(hash) = shared_resume_checksum(&shared) {
        match verify_file(shared.part_path.clone(), hash.clone()).await {
            Ok(true) => {}
            Ok(false) => {
                let err = format!("checksum mismatch: expected {}", hash.display());
                shared.set_error(&err);
                shared.set_state(State::Failed);
                return Err(Error::ChecksumMismatch);
            }
            Err(e) => {
                shared.set_error(e.to_string());
                shared.set_state(State::Failed);
                return Err(e);
            }
        }
    }

    std::fs::rename(&shared.part_path, &shared.dest).map_err(Error::Io)?;
    // The download is complete: remove the sidecar so a later run does not "resume" it.
    let sp = state_path(&shared.dest);
    let _ = std::fs::remove_file(sp);
    shared.progress.store(total, Ordering::Relaxed);
    shared.finished.store(true, Ordering::Relaxed);
    shared.set_state(State::Done);
    Ok(Outcome {
        success: true,
        error: None,
        path: Some(shared.dest.clone()),
        downloaded,
        total: Some(total),
    })
}

fn shared_resume_checksum(shared: &Shared) -> Option<Hash> {
    shared
        .resume_state
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.checksum.clone())
}

/// Hash a completed file off the async threads: `spawn_blocking` keeps a Tokio worker free.
async fn verify_file(path: PathBuf, hash: Hash) -> Result<bool> {
    tokio::task::spawn_blocking(move || matches(&path, &hash))
        .await
        .map_err(|e| Error::Verification(format!("verification task failed: {e}")))?
        .map_err(Error::Io)
}

/// A running download. `wait` consumes the JoinHandle exactly once.
pub struct Download {
    shared: Arc<Shared>,
    cmds: mpsc::Sender<Command>,
    task: Mutex<Option<JoinHandle<Result<Outcome>>>>,
}

impl Download {
    /// Probe (or resume), build the URI pool once, create the sparse part file, and start
    /// the supervisor. Must be called from within a Tokio runtime.
    pub async fn start(opts: StartOptions) -> Result<Self> {
        let client = probe::build_client(
            opts.proxy.as_deref(),
            opts.ip_family.as_deref(),
            CONNECT_TIMEOUT,
        )?;

        // Mirror failover starts at the probe: if the primary answers 5xx (or is
        // unreachable), try each mirror in order. aria2 parity requires the first
        // mirror's 500 to fall through to the next URI rather than aborting the transfer.
        let mut candidates = Vec::with_capacity(1 + opts.mirrors.len());
        candidates.push(opts.url.clone());
        candidates.extend(opts.mirrors.iter().cloned());
        let mut probe_failed: Vec<Url> = Vec::new();
        let mut info: Option<ProbeInfo> = None;
        let mut last_err: Option<Error> = None;
        for candidate in &candidates {
            let cookie = opts.cookie_header.as_deref().filter(|_| {
                candidate
                    .host_str()
                    .is_some_and(|h| opts.cookie_hosts.iter().any(|c| c.eq_ignore_ascii_case(h)))
            });
            // A mirror candidate is a different host: never send the caller's
            // credentials to it during the probe either (reqwest only strips on
            // redirects, not on a direct GET to an alternate host).
            let scoped = probe::scope_headers(&opts.headers, opts.url.host_str(), candidate.host_str());
            match probe::probe(&client, candidate.clone(), scoped, cookie).await {
                Ok(i) => {
                    info = Some(i);
                    break;
                }
                Err(e) => {
                    probe_failed.push(candidate.clone());
                    last_err = Some(e);
                }
            }
        }
        let info: ProbeInfo = info.ok_or_else(|| {
            last_err.unwrap_or_else(|| Error::Network("no URI available".into()))
        })?;

        // F4: a server without Content-Length/Accept-Ranges is NOT an error. Use a single
        // plain GET (no Range), report total=None, and never claim a fictitious 100%.
        let unknown_size = info.size.is_none() || !info.resumable;
        let size = if unknown_size {
            UNKNOWN_SIZE
        } else {
            info.size.unwrap()
        };

        // F10: use the probe's server-announced checksum when the caller did not supply one,
        // and retain the Content-Disposition/URL filename in the resume state.
        let checksum = opts.checksum.clone().or_else(|| info.checksum.clone());
        let suggested_name = info.file_name.clone();

        let final_url = info.final_url.clone();
        let validators = info.validators.clone();

        // The jar is scoped to the EXACT hosts it was captured for (e.g. 127.0.0.1 only).
        // A 302 to localhost must never receive it, even though localhost is the final host.
        let cookie_hosts = opts.cookie_hosts.clone();

        let mut urls = vec![final_url.clone()];
        // Mirrors supplied by the caller; de-duplicated but never reordered or dropped.
        for m in &opts.mirrors {
            if !urls.contains(m) {
                urls.push(m.clone());
            }
        }
        let uris = Arc::new(UriPool::new(urls)?);
        for failed in &probe_failed {
            uris.mark_failed(failed);
        }

        let min_split = if opts.min_split > 0 {
            opts.min_split
        } else {
            DEFAULT_MIN_SPLIT
        };

        // Resume handling: a previously unknown-size download is restarted from byte 0
        // (its server does not support ranges), so no saved segment offsets are trusted.
        let mut resumed_state: Option<ResumeState> = None;
        if opts.resume {
            let sp = state_path(&opts.dest);
            let rs = ResumeState::load(&sp).map_err(Error::InvalidState)?;
            if !part_path(&opts.dest).exists() {
                return Err(Error::InvalidState("part file is missing".into()));
            }
            resumed_state = Some(rs);
        }
        let planner = if let Some(rs) = &resumed_state
            && !rs.unknown_size
        {
            Planner::resume(size, min_split, &rs.segments)
                .map_err(|_| Error::InvalidState("saved segments do not tile the file".into()))?
        } else {
            Planner::new(size, min_split)
        };

        let part = part_path(&opts.dest);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&part)?;
        if unknown_size {
            if let Some(rs) = &resumed_state
                && rs.unknown_size
            {
                file.set_len(0)?; // restart a no-range download from byte 0
            }
        } else if file.metadata()?.len() < size {
            file.set_len(size)?; // sparse, known size only
        }

        let resume_state = if let Some(mut rs) = resumed_state {
            rs.checksum = checksum.clone().or(rs.checksum);
            if unknown_size {
                rs.unknown_size = true;
                rs.size = 0;
                rs.segments.clear();
            }
            Some(rs)
        } else {
            let mut rs = ResumeState::new(opts.url.clone(), final_url, opts.dest.clone(), size, validators);
            rs.checksum = checksum.clone();
            rs.unknown_size = unknown_size;
            rs.suggested_name = Some(suggested_name);
            Some(rs)
        };
        let initial_cursors: Vec<SavedSegment> = planner.save();
        let downloaded = if unknown_size { 0 } else { planner.downloaded() };

        let (paused, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            planner: Mutex::new(planner),
            progress: AtomicU64::new(downloaded),
            info: Mutex::new(StateInfo {
                state: State::Downloading,
                error: None,
            }),
            speed: Mutex::new(SpeedMeter::new()),
            limiter: Limiter::new(opts.rate_limit_bps),
            uris,
            client,
            headers: opts.headers.clone(),
            header_host: opts.url.host_str().map(str::to_owned),
            cookie_header: opts.cookie_header.clone(),
            cookie_hosts,
            finished: AtomicBool::new(false),
            last_progress: Mutex::new(Instant::now()),
            part: file,
            part_path: part,
            dest: opts.dest.clone(),
            resume_state: Mutex::new(resume_state),
            min_split,
            connections: if unknown_size { 1 } else { opts.connections.clamp(1, 16) },
            unknown_size,
            last_save: Mutex::new(Instant::now()),
            paused,
            cancel_tx: watch::channel(false).0,
        });
        // Touch the sidecar immediately so a crash during the first segments is resumable.
        if let Some(rs) = shared.resume_state.lock().unwrap().as_mut() {
            let p = state_path(&opts.dest);
            let _ = rs.save(&p, initial_cursors, &shared.part);
        }

        let (tx, rx) = mpsc::channel(COMMANDS);
        let s = shared.clone();
        let task = tokio::spawn(async move { supervise(s, rx).await });
        Ok(Download {
            shared,
            cmds: tx,
            task: Mutex::new(Some(task)),
        })
    }

    /// Progress view. Called frequently by Python; holds locks only for a few reads.
    pub fn poll_snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    /// Recycle all sockets in process. Committed offsets and the URI pool are preserved.
    /// A no-Range single stream has nothing to resegment: aborting it would force a full
    /// re-GET of the committed prefix (the O(N^2) L1 defect), so it is a no-op here too.
    /// Terminal handles are a no-op so a late watchdog tick cannot turn a completed
    /// transfer into exit 130.
    pub async fn recycle_connections(&self) -> Result<()> {
        let terminal = matches!(
            self.shared.info.lock().unwrap().state,
            State::Done | State::Failed | State::Cancelled
        );
        if self.shared.unknown_size || terminal {
            return Ok(());
        }
        self.cmds
            .send(Command::Recycle)
            .await
            .map_err(|_| Error::Cancelled)
    }

    pub fn pause(&self) -> Result<()> {
        self.cmds.try_send(Command::Pause).map_err(|_| Error::Cancelled)
    }

    pub fn resume(&self) -> Result<()> {
        self.cmds.try_send(Command::Resume).map_err(|_| Error::Cancelled)
    }

    pub fn cancel(&self) -> Result<()> {
        self.cmds.try_send(Command::Cancel).map_err(|_| Error::Cancelled)
    }

    pub fn set_limit(&self, rate_bps: Option<u64>) -> Result<()> {
        self.cmds
            .try_send(Command::SetLimit(rate_bps))
            .map_err(|_| Error::Cancelled)
    }

    /// Stable URI pool snapshot (final URL first), for tests and introspection.
    pub fn uri_pool(&self) -> Vec<String> {
        self.shared.uris.urls()
    }

    /// Committed `(start, cursor, end)` per segment, for offset-preservation assertions.
    pub fn committed_offsets(&self) -> Vec<(u64, u64, u64)> {
        self.shared
            .planner
            .lock()
            .unwrap()
            .save()
            .into_iter()
            .map(|s| (s.start, s.cursor, s.end))
            .collect()
    }

    /// Wait for completion. Consumes the supervisor handle exactly once.
    pub async fn wait(&self) -> Result<Outcome> {
        let handle = self
            .task
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::InvalidState("wait() called more than once".into()))?;
        handle.await.map_err(|e| Error::Network(e.to_string()))?
    }

    pub fn min_split(&self) -> u64 {
        self.shared.min_split
    }

    pub fn uri_pool_len(&self) -> usize {
        self.shared.uris.len()
    }
}
