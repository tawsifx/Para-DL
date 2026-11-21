//! Ported fault-injection suite from `rapids/src/tests.rs` (21 tests), the
//! byte-exact sidecar and connection-cap contracts from the parity matrix,
//! trait-level `DownloadEngine` exit-code mapping tests, and the DoH TLS-name
//! contract (feature-gated, no live network).
//!
//! Every test here is a behavioural port of the legacy engine suite: loopback
//! servers inject mid-body closes, 5xx responses, stalls and cross-host
//! redirects, and the assertions mirror the Python oracle (`test_rust_engine_parity.py`,
//! `test_rust_pipeline_e2e.py`) for byte-exact files, close-delimited 200 with no
//! false 100%, `.pdl.part`/`.pdl.json` resume and verify-then-rename.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use paradl_core::ExitCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::download::{Download, Outcome, Snapshot, StartOptions, State, set_test_read_timeout};
use crate::engine::{DownloadEngine, NdEngine, NoopProgressSink, ProgressSink, Target};
use crate::limiter::Limiter;
use crate::planner::{Claim, Planner, SavedSegment};
use crate::state::{ResumeState, Validators, state_path};
use crate::verify::{Algo, Hash, matches};

const MB: u64 = 1 << 20;

// ---------------------------------------------------------------------------
// (1) Planner: split / resume / release offset preservation
// ---------------------------------------------------------------------------

fn check_invariants(p: &Planner) {
    let mut expect = 0;
    let mut workers = std::collections::HashSet::new();
    for s in p.segments() {
        assert_eq!(s.start, expect, "segments must stay contiguous");
        assert!(s.start <= s.cursor && s.cursor <= s.reserved && s.reserved <= s.end);
        assert!(s.start < s.end, "no empty segments");
        if let Some(w) = s.worker {
            assert!(workers.insert(w), "a worker may hold only one segment");
        }
        expect = s.end;
    }
    assert_eq!(expect, p.size());
}

#[test]
fn planner_split_resume_release_preserves_offsets() {
    let mut p = Planner::new(100 * MB, MB);
    assert_eq!(
        p.assign(0, None),
        Some(crate::planner::Assignment {
            from: 0,
            to: 100 * MB,
            written: 0,
            preempted: None,
        })
    );
    assert_eq!(
        p.assign(1, None),
        Some(crate::planner::Assignment {
            from: 50 * MB,
            to: 100 * MB,
            written: 0,
            preempted: None,
        })
    );
    assert_eq!(p.segments()[0].end, 50 * MB);
    check_invariants(&p);

    // A worker commits 3 MB, then fails. release() drops the *uncommitted* claim but
    // must keep the committed cursor, so the next assignment resumes at that offset
    // and reports the bytes already on disk.
    assert_eq!(p.claim(0, 0, 8 * MB), Claim::Granted(8 * MB));
    assert_eq!(p.commit(0, 3 * MB), None);
    p.release(0);
    let a = p.assign(0, None).unwrap();
    assert_eq!((a.from, a.written), (3 * MB, 3 * MB));
    check_invariants(&p);

    // Round-trip through the persisted form: offsets must survive verbatim.
    let saved = p.save();
    let restored = Planner::resume(100 * MB, MB, &saved).unwrap();
    assert_eq!(restored.downloaded(), p.downloaded());
    assert_eq!(restored.save(), saved);
    check_invariants(&restored);

    // A corrupt saved table is rejected rather than silently mis-counting bytes.
    let bad = vec![SavedSegment { start: 5, cursor: 5, end: 10 }];
    assert!(Planner::resume(100, 1, &bad).is_err());
}

// ---------------------------------------------------------------------------
// (2) State: save/load round-trip (craft-atomic replaced by std write-temp-rename)
// ---------------------------------------------------------------------------

#[test]
fn state_save_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.bin");
    let part = std::fs::File::create(dir.path().join("a.bin.pdl.part")).unwrap();
    let url: url::Url = "http://x/a.bin".parse().unwrap();
    let validators = Validators::from_response(Some("\"e\""), None, None, Some("x"));
    let mut s = ResumeState::new(url.clone(), url, dest.clone(), 100, validators);
    let segs = vec![
        SavedSegment { start: 0, cursor: 10, end: 50 },
        SavedSegment { start: 50, cursor: 60, end: 100 },
    ];
    s.save(&state_path(&dest), segs.clone(), &part).unwrap();
    let loaded = ResumeState::load(&state_path(&dest)).unwrap();
    assert_eq!(loaded, s);
    assert_eq!(loaded.downloaded(), 20);
    assert_eq!(loaded.segments, segs);
    assert_eq!(
        crate::naming::part_path(&dest),
        dir.path().join("a.bin.pdl.part")
    );
}

// ---------------------------------------------------------------------------
// (3) Verify: SHA-256 parse + match
// ---------------------------------------------------------------------------

#[test]
fn verify_sha256_parse_and_match() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("fox.txt");
    std::fs::write(&p, b"The quick brown fox jumps over the lazy dog").unwrap();

    let sha = Hash::parse("d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592")
        .unwrap();
    assert_eq!(sha.algo, Algo::Sha256);
    assert!(matches(&p, &sha).unwrap());

    let named = Hash::parse("SHA256: d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592  *x.iso").unwrap();
    assert_eq!(named.algo, Algo::Sha256);
    assert!(Hash::parse("md5:abc").is_none());
    assert!(Hash::parse("not a hash").is_none());

    let wrong = Hash::parse("00000000000000000000000000000000").unwrap();
    assert!(!matches(&p, &wrong).unwrap());
}

// ---------------------------------------------------------------------------
// (4) Limiter: rate is actually bounded (Tokio's paused clock)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn limiter_bounds_average_rate() {
    use tokio::time::Instant;
    let l = Limiter::new(Some(400 * 1024));
    let t = Instant::now();
    for _ in 0..20 {
        l.acquire(10 * 1024).await; // 200 KB at 400 KB/s ~= 0.5 s
    }
    let secs = t.elapsed().as_secs_f64();
    assert!((0.4..0.75).contains(&secs), "took {secs:.2}s");
    l.set(Some(1024));
    assert_eq!(l.get(), Some(1024));
    l.set(None);
    assert_eq!(l.get(), None);
}

// ---------------------------------------------------------------------------
// (5) Fault-injected recycle: offsets preserved AND URI pool intact.
// This is the regression test for the aria2 pause/unpause URI-loss bug described in
// docs/diagnostics/watchdog-cycle-aria2-bug.md: recycling must never drop a URI.
// ---------------------------------------------------------------------------

/// Minimal loopback HTTP Range server with mid-body connection drops (fault injection).
/// Adapted from the labserver *pattern*; written from scratch here, not copied.
async fn spawn_fault_range_server(payload: Arc<Vec<u8>>, faults: Arc<AtomicUsize>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            let faults = faults.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                // Read request headers (bounded).
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let req = String::from_utf8_lossy(&req);
                let range = req.lines().find_map(|l| {
                    let l = l.trim();
                    l.strip_prefix("Range: ")
                        .or_else(|| l.strip_prefix("range: "))
                        .and_then(|v| v.strip_prefix("bytes="))
                });
                let n = payload.len();
                let (status, start, end) = match range {
                    Some(r) => {
                        let mut it = r.splitn(2, '-');
                        let s: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let e: usize = it
                            .next()
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(n - 1)
                            .min(n - 1);
                        if s >= n || s > e {
                            let _ = sock.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                            return;
                        }
                        ("206 Partial Content", s, e)
                    }
                    None => ("200 OK", 0, n - 1),
                };
                let chunk = &payload[start..=end];
                let head = format!(
                    "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    chunk.len()
                );
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                // Probe GET gets the whole body; range GET may be faulted mid-stream.
                if range.is_some() && faults.load(Ordering::SeqCst) > 0 {
                    faults.fetch_sub(1, Ordering::SeqCst);
                    let send = chunk.len().min(4096);
                    let _ = sock.write_all(&chunk[..send]).await;
                    return; // abrupt close == injected network failure
                }
                let _ = sock.write_all(chunk).await;
            });
        }
    });
    (format!("http://{addr}/file.bin"), task)
}

fn offsets_cover(before: &[(u64, u64, u64)], after: &[(u64, u64, u64)]) -> bool {
    before.iter().all(|(start, cursor, _)| {
        after
            .iter()
            .any(|(s, c, e)| *s <= *start && *e >= *cursor && *c >= *cursor)
    })
}

#[tokio::test]
async fn fault_injected_recycle_preserves_offsets_and_uri_pool() {
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let faults = Arc::new(AtomicUsize::new(8));
    let (url, server) = spawn_fault_range_server(Arc::new(payload.clone()), faults.clone()).await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let primary: url::Url = url.parse().unwrap();
    // F5: distinct URLs — identical ones would be de-duplicated by UriPool::new, so the
    // URI-pool-preservation assertion would be meaningless (and asserted len 2 vs actual 1).
    let mirror: url::Url = format!("{url}/mirror.bin").parse().unwrap();
    let opts = StartOptions {
        url: primary,
        mirrors: vec![mirror],
        dest: dest.clone(),
        connections: 4,
        headers: Default::default(),
        checksum: None,
        rate_limit_bps: None,
        min_split: 1024,
        resume: false,
        proxy: None,
        ip_family: None,
        doh_endpoint: None,
        cookie_header: None,
        cookie_hosts: Vec::new(),
    };
    let dl = Download::start(opts).await.unwrap();

    // Wait until committed progress exists (fault server sends 4 KB before dropping).
    for _ in 0..400 {
        if dl.poll_snapshot().downloaded > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = dl.poll_snapshot();
    assert!(before.downloaded > 0, "expected committed bytes before recycle");
    let before_offsets = dl.committed_offsets();
    let before_uris = dl.uri_pool();
    assert_eq!(before_uris.len(), 2, "URI pool holds final URL plus mirror");

    dl.recycle_connections().await.unwrap();

    // The aria2 bug: pause/unpause lost the redirected URI. Our recycle must keep
    // the exact same URI pool, with the same count and order.
    assert_eq!(dl.uri_pool(), before_uris, "URI pool must survive recycle");
    let after_offsets = dl.committed_offsets();
    assert!(
        offsets_cover(&before_offsets, &after_offsets),
        "committed offsets must never move backwards: before={before_offsets:?} after={after_offsets:?}"
    );
    assert!(
        dl.poll_snapshot().downloaded >= before.downloaded,
        "downloaded bytes must be monotonic across recycle"
    );
    // The supervisor processes the recycle command asynchronously; wait for re-assignment.
    for _ in 0..200 {
        if dl.poll_snapshot().connections == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(dl.poll_snapshot().connections, 4, "all workers re-assigned");

    // Stop injecting faults and let the re-assigned workers finish from committed offsets.
    faults.store(0, Ordering::SeqCst);
    let outcome: Outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("download timed out after recycle")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload, "file must be byte-exact");
    server.abort();
}

// ---------------------------------------------------------------------------
// Large transfer with many connections: catches completion-bookkeeping races that
// a 1 MiB payload cannot expose (the 32 MiB benchmark failure).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn large_transfer_16_connections_completes() {
    let payload: Vec<u8> = (0..32 * MB).map(|i| (i % 251) as u8).collect();
    let faults = Arc::new(AtomicUsize::new(0));
    let (url, server) = spawn_fault_range_server(Arc::new(payload.clone()), faults).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("large.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 16,
        min_split: 64 * 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(60), dl.wait()).await;
    match result {
        Ok(Ok(outcome)) => {
            assert!(outcome.success, "outcome: {outcome:?}");
            assert_eq!(std::fs::read(&dest).unwrap(), payload, "file must be byte-exact");
        }
        Ok(Err(e)) => {
            let snap = dl.poll_snapshot();
            let offsets = dl.committed_offsets();
            panic!(
                "download failed: {e:?}; snapshot={snap:?}; committed={}; last={:?}",
                offsets.len(),
                offsets.last()
            );
        }
        Err(_) => panic!("large transfer timed out"),
    }
    server.abort();
}

// ---------------------------------------------------------------------------
// F4 regression: GitHub -> codeload style server with NO Content-Length and NO
// Accept-Ranges must stream as one stream with total=None (never a false 100%).
// ---------------------------------------------------------------------------

async fn spawn_stream_only_server(payload: Arc<Vec<u8>>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                // Deliberately no Content-Length and no Accept-Ranges.
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                if sock.write_all(head).await.is_err() {
                    return;
                }
                let _ = sock.write_all(&payload).await;
            });
        }
    });
    (format!("http://{addr}/main.zip"), task)
}

#[tokio::test]
async fn unknown_size_stream_has_no_false_total() {
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_stream_only_server(Arc::new(payload.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("main.zip");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 16,
        min_split: 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    assert_eq!(dl.poll_snapshot().total, None, "unknown size must not report a total");
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("unknown-size stream timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    server.abort();
}

// ---------------------------------------------------------------------------
// F1 regression: a checksum mismatch must fail AFTER verifying the part file and
// BEFORE renaming it into place (ExitCode.VERIFICATION_FAILED parity).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn checksum_mismatch_fails_before_rename() {
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let faults = Arc::new(AtomicUsize::new(0)); // no faults
    let (url, server) = spawn_fault_range_server(Arc::new(payload), faults).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        min_split: 1024,
        checksum: Hash::parse("00000000000000000000000000000000"), // wrong SHA-256
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let err = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("checksum test timed out")
        .unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::VerificationFailed, "got {err:?}");
    assert!(!dest.exists(), "a failed verification must not rename the part into place");
    server.abort();
}

// ---------------------------------------------------------------------------
// L1 root cause: a close-delimited (no Content-Length / no Accept-Ranges) 200 is
// re-GET from byte 0 for every 64 KiB window. A no-Range server can only answer
// with the full body, so the engine re-downloads the whole committed prefix on
// every reconnect: O(N^2) bytes for an N-window payload. This test asserts the
// *structural* invariant (request count / bytes served), not wall-clock time,
// so it is deterministic. The Python symptom is 25 GETs / 13 MB for a 1 MB body.
// ---------------------------------------------------------------------------

async fn spawn_close_delimited_counting_server(
    payload: Arc<Vec<u8>>,
    requests: Arc<AtomicUsize>,
    served: Arc<AtomicU64>,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            requests.fetch_add(1, Ordering::SeqCst);
            let payload = payload.clone();
            let served = served.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                // Deliberately NO Content-Length / Accept-Ranges: close-delimited,
                // exactly like codeload's final response and /final in the parity suite.
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                if sock.write_all(head).await.is_err() {
                    return;
                }
                if sock.write_all(&payload).await.is_ok() {
                    served.fetch_add(payload.len() as u64, Ordering::SeqCst);
                }
            });
        }
    });
    (format!("http://{addr}/main.zip"), task)
}

#[tokio::test]
async fn close_delimited_stream_is_not_refetched_per_window() {
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let requests = Arc::new(AtomicUsize::new(0));
    let served = Arc::new(AtomicU64::new(0));
    let (url, server) = spawn_close_delimited_counting_server(
        Arc::new(payload.clone()), requests.clone(), served.clone(),
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("main.zip");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 8, // must be ignored: unknown-size is a single plain GET
        min_split: 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("close-delimited stream timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);

    // Exactly two GETs: one probe + one body stream. Before the fix the 64 KiB
    // windowing re-GET the prefix 4 times (+ a final all-skip GET): 5 total.
    let got = requests.load(Ordering::SeqCst);
    let bytes_served = served.load(Ordering::SeqCst);
    assert!(
        got <= 2,
        "close-delimited body was re-GET {got} times (probe + {} body GETs); \
         bytes served = {bytes_served} for a {} byte payload",
        got.saturating_sub(1),
        payload.len(),
    );
    assert!(
        bytes_served <= 2 * payload.len() as u64,
        "server re-sent {bytes_served} bytes for a {} byte payload (quadratic re-skip)",
        payload.len(),
    );
    server.abort();
}

/// Close-delimited server with a per-chunk delay so a recycle can be injected mid-stream.
async fn spawn_slow_close_delimited_server(
    payload: Arc<Vec<u8>>,
    requests: Arc<AtomicUsize>,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            requests.fetch_add(1, Ordering::SeqCst);
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                if sock.write_all(head).await.is_err() {
                    return;
                }
                for part in payload.chunks(4096) {
                    if sock.write_all(part).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
        }
    });
    (format!("http://{addr}/stream.bin"), task)
}

#[tokio::test]
async fn recycle_is_noop_for_unknown_size_stream() {
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let requests = Arc::new(AtomicUsize::new(0));
    let (url, server) = spawn_slow_close_delimited_server(
        Arc::new(payload.clone()), requests.clone(),
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("stream.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await; // mid-stream
    let before = requests.load(Ordering::SeqCst);
    assert!(before >= 2, "probe + body GET must have happened, got {before}");
    dl.recycle_connections().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let after = requests.load(Ordering::SeqCst);
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("stream timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    assert_eq!(
        after, before,
        "recycle must be a no-op for a no-Range single stream: \
         it re-GET the whole committed prefix ({before} -> {after} requests)"
    );
    server.abort();
}

#[tokio::test]
async fn recycle_on_terminal_handle_is_noop() {
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let faults = Arc::new(AtomicUsize::new(0));
    let (url, server) = spawn_fault_range_server(Arc::new(payload), faults).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        min_split: 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("download timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(dl.poll_snapshot().state, State::Done);

    // A late watchdog tick must not turn a completed transfer into exit 130:
    // recycle on a terminal handle returns Ok promptly and leaves state intact.
    tokio::time::timeout(Duration::from_millis(500), dl.recycle_connections())
        .await
        .expect("recycle on terminal handle hung")
        .expect("recycle on terminal handle returned an error");
    assert_eq!(dl.poll_snapshot().state, State::Done);
    assert_eq!(dl.poll_snapshot().downloaded, outcome.downloaded);
    server.abort();
}

// ---------------------------------------------------------------------------
// Fault injection: a 5xx on the primary during a *range* GET (the probe still
// succeeds) must mark that URI unhealthy and fail the worker over to the mirror
// at its committed offset, keeping the URI pool intact.
// ---------------------------------------------------------------------------

async fn spawn_probe_ok_range_500_server(payload: Arc<Vec<u8>>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let req = String::from_utf8_lossy(&req);
                if req.lines().any(|l| l.to_ascii_lowercase().starts_with("range:")) {
                    let _ = sock.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.write_all(&payload).await;
            });
        }
    });
    (format!("http://{addr}/primary.bin"), task)
}

#[tokio::test]
async fn mirror_failover_on_5xx_completes_byte_exact() {
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let (primary, primary_server) = spawn_probe_ok_range_500_server(Arc::new(payload.clone())).await;
    let (mirror, mirror_server) = spawn_fault_range_server(Arc::new(payload.clone()), Arc::new(AtomicUsize::new(0))).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let opts = StartOptions {
        url: primary.parse().unwrap(),
        mirrors: vec![mirror.parse().unwrap()],
        dest: dest.clone(),
        connections: 4,
        min_split: 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    assert_eq!(dl.uri_pool_len(), 2, "URI pool holds primary + mirror");
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("mirror failover timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload, "mirror bytes must be byte-exact");
    assert_eq!(dl.uri_pool_len(), 2, "failover must not rebuild/drop the URI pool");
    primary_server.abort();
    mirror_server.abort();
}

// ---------------------------------------------------------------------------
// Crash resume: a part file plus its .pdl.json sidecar (committed cursor at the
// halfway point) must resume at that offset and produce a byte-identical file,
// then remove the sidecar.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_from_part_and_sidecar_is_byte_exact() {
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let half = (payload.len() / 2) as u64;
    let (url, server) = spawn_fault_range_server(Arc::new(payload.clone()), Arc::new(AtomicUsize::new(0))).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("resumed.bin");
    let part = crate::naming::part_path(&dest);
    std::fs::write(&part, &payload[..half as usize]).unwrap();
    let pfile = std::fs::OpenOptions::new().write(true).open(&part).unwrap();
    let parsed: url::Url = url.parse().unwrap();
    let mut rs = ResumeState::new(parsed.clone(), parsed, dest.clone(), payload.len() as u64, Validators::default());
    rs.save(
        &state_path(&dest),
        vec![SavedSegment { start: 0, cursor: half, end: payload.len() as u64 }],
        &pfile,
    )
    .unwrap();
    drop(pfile);
    assert!(state_path(&dest).exists());

    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        min_split: 1024,
        resume: true,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    assert_eq!(dl.committed_offsets()[0].1, half, "must resume at the committed cursor");
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("resume timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload, "resumed file must be byte-identical");
    assert!(!state_path(&dest).exists(), "sidecar must be removed on completion");
    server.abort();
}

// ---------------------------------------------------------------------------
// Liveness: cancel while workers are retiring, and caller-side wait(timeout=)
// on a stalled stream, must both return promptly (no JoinSet/channel hang).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_while_workers_retire_does_not_hang() {
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let requests = Arc::new(AtomicUsize::new(0));
    let (url, server) = spawn_slow_close_delimited_server(Arc::new(payload), requests).await;
    let dir = tempfile::tempdir().unwrap();
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dir.path().join("cancelled.bin"),
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    dl.cancel().unwrap();
    let res = tokio::time::timeout(Duration::from_secs(5), dl.wait())
        .await
        .expect("wait() hung after cancel — all workers retired");
    assert!(
        matches!(&res, Err(e) if e.exit_code() == ExitCode::Interrupted),
        "cancel must map to exit 130: {res:?}"
    );
    server.abort();
}

async fn spawn_stalling_range_server(payload: Arc<Vec<u8>>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes 0-{}/{}\r\nConnection: close\r\n\r\n",
                    payload.len(), payload.len() - 1, payload.len()
                );
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.write_all(&payload[..4096]).await;
                std::future::pending::<()>().await; // stall mid-body
            });
        }
    });
    (format!("http://{addr}/stall.bin"), task)
}

#[tokio::test]
async fn wait_timeout_is_honored_on_stalled_download() {
    let payload: Vec<u8> = (0..512 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_stalling_range_server(Arc::new(payload)).await;
    let dir = tempfile::tempdir().unwrap();
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dir.path().join("stall.bin"),
        connections: 2,
        min_split: 1024,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let res = tokio::time::timeout(Duration::from_millis(300), dl.wait()).await;
    assert!(res.is_err(), "wait() must honor the caller's timeout on a stalled stream");
    dl.cancel().unwrap(); // release the supervisor; no leaked task
    server.abort();
}

// Global constraint: all waits have timeouts. The probe's reqwest send() had no
// bound, so a server that accepts but never sends headers hung Download::start
// forever. Virtual time lets this be proven in milliseconds.
#[tokio::test(start_paused = true)]
async fn probe_send_has_bounded_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_sock, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await; // accepts, never replies
    });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let dir = tempfile::tempdir().unwrap();
    let opts = StartOptions {
        url: format!("http://{addr}/never.bin").parse().unwrap(),
        dest: dir.path().join("never.bin"),
        ..StartOptions::default()
    };
    let start = tokio::spawn(async move { Download::start(opts).await });
    // Advance in small steps so the spawned start task is actually polled into
    // probe::probe before its 15 s timer fires.
    for _ in 0..80 {
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
    }
    let res = tokio::time::timeout(Duration::from_secs(2), start)
        .await
        .expect("Download::start hung on a non-responsive server — probe has no timeout")
        .unwrap();
    assert!(res.is_err(), "probe must time out on a non-responsive server");
    server.abort();
}

// ---------------------------------------------------------------------------
// Security: host-sensitive headers must not survive a cross-host 302. The probe
// follows redirects with reqwest (which strips Authorization/Cookie/
// Proxy-Authorization across origins), but workers issue DIRECT requests to the
// redirect-derived final URI, so the native engine must do the stripping itself.
// Headers are asserted as observed by the loopback server, not echoed from input.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SeenHeaders {
    host: String,
    path: String,
    authorization: Option<String>,
    proxy_authorization: Option<String>,
    cookie: Option<String>,
    trace: Option<String>,
}

fn header_value(req: &str, name: &str) -> Option<String> {
    req.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name)
            .then(|| v.trim().to_owned())
    })
}

/// One listener; /redirect (Host: 127.0.0.1) 302s to /file (Host: localhost),
/// the same socket address but a different host string — exactly the S3 scenario.
async fn spawn_header_observing_redirect_server(
    payload: Arc<Vec<u8>>,
    seen: Arc<std::sync::Mutex<Vec<SeenHeaders>>>,
    redirect_host: &str,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let redirect_host = redirect_host.to_owned();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            let seen = seen.clone();
            let redirect_host = redirect_host.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let req = String::from_utf8_lossy(&req);
                let get = req.lines().next().unwrap_or("");
                let path = get.split_whitespace().nth(1).unwrap_or("").to_owned();
                let host = header_value(&req, "Host").unwrap_or_default();
                seen.lock().unwrap().push(SeenHeaders {
                    host: host.clone(),
                    path: path.clone(),
                    authorization: header_value(&req, "Authorization"),
                    proxy_authorization: header_value(&req, "Proxy-Authorization"),
                    cookie: header_value(&req, "Cookie"),
                    trace: header_value(&req, "X-Trace-Id"),
                });
                let (head, body): (String, &[u8]) = if path.starts_with("/redirect") {
                    (
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://{redirect_host}:{port}/file\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        ),
                        &[],
                    )
                } else {
                    let (start, end) = match req.lines().find_map(|l| {
                        l.strip_prefix("Range: bytes=")
                            .or_else(|| l.strip_prefix("range: bytes="))
                    }) {
                        Some(r) => {
                            let mut it = r.splitn(2, '-');
                            let s: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                            let e: usize = it.next().and_then(|v| v.parse().ok()).unwrap_or(payload.len() - 1);
                            (s, e.min(payload.len() - 1))
                        }
                        None => (0, payload.len() - 1),
                    };
                    let chunk = &payload[start..=end];
                    (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            chunk.len(), payload.len()
                        ),
                        chunk,
                    )
                };
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let _ = sock.write_all(body).await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/redirect"), task)
}

#[tokio::test]
async fn host_sensitive_headers_do_not_leak_across_cross_host_redirect() {
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, server) = spawn_header_observing_redirect_server(Arc::new(payload.clone()), seen.clone(), "localhost").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer scoped-token".parse().unwrap());
    headers.insert("Proxy-Authorization", "Basic cHJveHk6c2VjcmV0".parse().unwrap());
    headers.insert("Cookie", "session=from-headers".parse().unwrap());
    headers.insert("X-Trace-Id", "trace-123".parse().unwrap()); // benign: must survive
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        min_split: 1024,
        headers,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await
        .expect("download timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);

    let requests = seen.lock().unwrap().clone();
    assert!(requests.iter().any(|r| r.path.starts_with("/redirect")), "no redirect request seen");
    assert!(requests.iter().any(|r| r.path.starts_with("/file")), "no final-host request seen");
    // Sanity: the server really observed distinct host strings.
    assert!(
        requests.iter().any(|r| r.host.starts_with("127.0.0.1"))
            && requests.iter().any(|r| r.host.starts_with("localhost")),
        "expected both original and cross-host requests: {requests:?}"
    );
    // Sensitive headers must reach the ORIGINAL host...
    let original = requests.iter().find(|r| r.host.starts_with("127.0.0.1")).unwrap();
    assert_eq!(original.authorization.as_deref(), Some("Bearer scoped-token"));
    assert_eq!(original.proxy_authorization.as_deref(), Some("Basic cHJveHk6c2VjcmV0"));
    assert_eq!(original.cookie.as_deref(), Some("session=from-headers"));
    // ...and must NEVER reach the cross-host redirect target (worker direct GETs).
    let leaked: Vec<&SeenHeaders> = requests
        .iter()
        .filter(|r| r.host.starts_with("localhost"))
        .filter(|r| r.authorization.is_some() || r.proxy_authorization.is_some() || r.cookie.is_some())
        .collect();
    assert!(
        leaked.is_empty(),
        "host-sensitive headers leaked to the cross-host target: {leaked:?}"
    );
    // Benign headers are still sent to both hosts.
    assert!(requests.iter().all(|r| r.trace.as_deref() == Some("trace-123")));
    server.abort();
}

#[tokio::test]
async fn same_host_redirect_keeps_sensitive_headers() {
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    // Same host string but a different path after the 302: scoping must not strip.
    let (url, server) = spawn_header_observing_redirect_server(Arc::new(payload.clone()), seen.clone(), "127.0.0.1").await;
    let dir = tempfile::tempdir().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer scoped-token".parse().unwrap());
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dir.path().join("same.bin"),
        connections: 2,
        min_split: 1024,
        headers,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await.expect("same-host download timed out").unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(dir.path().join("same.bin")).unwrap(), payload);
    let requests = seen.lock().unwrap().clone();
    assert!(requests.iter().any(|r| r.path.starts_with("/file")));
    assert!(
        requests.iter().all(|r| r.authorization.as_deref() == Some("Bearer scoped-token")),
        "same-host redirect must keep Authorization: {requests:?}"
    );
    server.abort();
}

#[tokio::test]
async fn mirror_host_does_not_receive_sensitive_headers() {
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (url, server) = spawn_header_observing_redirect_server(Arc::new(payload.clone()), seen.clone(), "localhost").await;
    let port = url::Url::parse(&url).unwrap().port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer scoped-token".parse().unwrap());
    let opts = StartOptions {
        url: format!("http://127.0.0.1:{port}/file").parse().unwrap(),
        // A different host string for the same socket: must NOT get the credential.
        mirrors: vec![format!("http://localhost:{port}/file").parse().unwrap()],
        dest: dir.path().join("mirror.bin"),
        connections: 4,
        min_split: 1024,
        headers,
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(15), dl.wait())
        .await.expect("mirror download timed out").unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(dir.path().join("mirror.bin")).unwrap(), payload);
    let requests = seen.lock().unwrap().clone();
    assert!(requests.iter().any(|r| r.host.starts_with("127.0.0.1")), "primary never hit");
    assert!(requests.iter().any(|r| r.host.starts_with("localhost")), "mirror never hit");
    assert!(
        requests.iter().filter(|r| r.host.starts_with("127.0.0.1"))
            .all(|r| r.authorization.as_deref() == Some("Bearer scoped-token")),
        "original host must keep Authorization: {requests:?}"
    );
    assert!(
        requests.iter().filter(|r| r.host.starts_with("localhost"))
            .all(|r| r.authorization.is_none()),
        "mirror host must not receive Authorization: {requests:?}"
    );
    server.abort();
}

// ---------------------------------------------------------------------------
// FINAL-REVIEW MUST-FIX: a close-delimited (unknown-size) stream that stalls after a
// partial body must be an ERROR, not a false success with a truncated file.
// `timeout(...).ok().flatten()` used to collapse a read timeout into the same `None`
// as genuine EOF, so the worker reported StreamEnded and the supervisor skipped all
// completion/error guards for unknown_size and renamed the truncated part into place.
// ---------------------------------------------------------------------------

/// Close-delimited server: writes a prefix, then either completes (real EOF) or stalls
/// forever. No Content-Length / Accept-Ranges, so only socket close signals end-of-body.
async fn spawn_close_delimited_stall_server(
    payload: Arc<Vec<u8>>,
    stall: bool,
) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let (mut sock, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut byte = [0u8; 1];
                for _ in 0..8192 {
                    match sock.read(&mut byte).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            req.push(byte[0]);
                            if req.ends_with(b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                if sock.write_all(head).await.is_err() {
                    return;
                }
                let prefix = 4096.min(payload.len());
                if sock.write_all(&payload[..prefix]).await.is_err() {
                    return;
                }
                if stall {
                    std::future::pending::<()>().await; // no EOF: only a read timeout ends this
                } else {
                    let _ = sock.write_all(&payload[prefix..]).await; // then close == real EOF
                }
            });
        }
    });
    (format!("http://{addr}/stall.bin"), task)
}

#[tokio::test]
async fn stalled_unknown_size_stream_is_an_error_not_a_false_success() {
    // Shorten the per-chunk read timeout so a stalled stream is provable in milliseconds.
    // cfg(test) only; production keeps the 30 s READ_TIMEOUT.
    set_test_read_timeout(Duration::from_millis(500));
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_close_delimited_stall_server(Arc::new(payload.clone()), true).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("stall.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let res = tokio::time::timeout(Duration::from_secs(10), dl.wait())
        .await
        .expect("stalled unknown-size stream did not terminate");
    assert!(
        res.is_err(),
        "a read timeout must NOT be reported as EOF/success: {res:?}"
    );
    assert_eq!(dl.poll_snapshot().state, State::Failed);
    assert!(!dest.exists(), "a truncated file must not be renamed into place");
    server.abort();
}

/// Companion guard: the shortened read timeout must NOT break a normally-completing
/// close-delimited stream — genuine EOF still means success.
#[tokio::test]
async fn completing_unknown_size_stream_still_succeeds_with_short_read_timeout() {
    set_test_read_timeout(Duration::from_millis(500));
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_close_delimited_stall_server(Arc::new(payload.clone()), false).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("main.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), dl.wait())
        .await
        .expect("completing unknown-size stream timed out")
        .unwrap();
    assert!(outcome.success, "outcome: {outcome:?}");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    assert_eq!(dl.poll_snapshot().state, State::Done);
    server.abort();
}

// ---------------------------------------------------------------------------
// Parity-matrix contract literals L06 and L14: connection cap and sidecar bytes.
// ---------------------------------------------------------------------------

#[test]
fn connections_are_clamped_to_one_through_sixteen() {
    // L06: min(max(n, 1), 16) — never 0, never above the MAX_CONNECTIONS cap.
    assert_eq!(crate::clamp_connections(0), 1);
    assert_eq!(crate::clamp_connections(1), 1);
    assert_eq!(crate::clamp_connections(8), 8);
    assert_eq!(crate::clamp_connections(16), 16);
    assert_eq!(crate::clamp_connections(17), 16);
    assert_eq!(crate::clamp_connections(u32::MAX), 16);
    assert_eq!(crate::MAX_CONNECTIONS, 16);
}

#[test]
fn sidecar_bytes_match_oracle_golden() {
    // L14: the `.pdl.json` sidecar must be byte-identical to the W0-2 golden
    // fixture (2-space pretty JSON, declared field order, skipped defaults).
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("a.bin");
    let part = std::fs::File::create(crate::naming::part_path(&dest)).unwrap();
    let url: url::Url = "http://x/a.bin".parse().unwrap();
    let validators = Validators::from_response(Some("\"e\""), None, None, Some("x"));
    let mut s = ResumeState::new(url.clone(), url, dest.clone(), 100, validators);
    let segs = vec![
        SavedSegment { start: 0, cursor: 10, end: 50 },
        SavedSegment { start: 50, cursor: 60, end: 100 },
    ];
    s.save(&state_path(&dest), segs, &part).unwrap();
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"url\": \"http://x/a.bin\",\n  \"final_url\": \"http://x/a.bin\",\n  \"dest\": \"{}\",\n  \"size\": 100,\n  \"validators\": {{\n    \"etag\": \"\\\"e\\\"\",\n    \"etag_host\": \"x\",\n    \"last_modified\": null\n  }},\n  \"segments\": [\n    {{\n      \"start\": 0,\n      \"cursor\": 10,\n      \"end\": 50\n    }},\n    {{\n      \"start\": 50,\n      \"cursor\": 60,\n      \"end\": 100\n    }}\n  ],\n  \"content_type\": null,\n  \"checksum\": null\n}}",
        dest.display()
    );
    assert_eq!(std::fs::read_to_string(state_path(&dest)).unwrap(), expected);
}

// ---------------------------------------------------------------------------
// L14 second half: an interrupt (exit 130) must leave the part file and the
// `.pdl.json` sidecar on disk so the next run can resume byte-exactly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_persists_sidecars_for_resume() {
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let requests = Arc::new(AtomicUsize::new(0));
    let (url, server) = spawn_slow_close_delimited_server(
        Arc::new(payload.clone()), requests,
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("interrupted.bin");
    let opts = StartOptions {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        ..StartOptions::default()
    };
    let dl = Download::start(opts).await.unwrap();
    // The sidecar touched at start is the zero snapshot (no committed bytes yet).
    let initial = ResumeState::load(&state_path(&dest)).unwrap().downloaded();
    assert_eq!(initial, 0, "a fresh download must start from a zero sidecar");
    // Throttle the transfer so it is still in flight when the periodic 2 s sidecar
    // save fires; a 60 ms run would never refresh the sidecar at all.
    dl.set_limit(Some(4096)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut refreshed = initial;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        refreshed = ResumeState::load(&state_path(&dest))
            .map(|s| s.downloaded())
            .unwrap_or(0);
        if refreshed > initial {
            break;
        }
    }
    assert!(
        refreshed > initial,
        "the real transfer path must refresh the sidecar with committed progress \
         (initial={initial}, got={refreshed})"
    );
    dl.cancel().unwrap();
    let res = tokio::time::timeout(Duration::from_secs(5), dl.wait())
        .await
        .expect("wait() hung after cancel");
    assert!(
        matches!(&res, Err(e) if e.exit_code() == ExitCode::Interrupted),
        "interrupt must map to exit 130: {res:?}"
    );
    assert!(
        crate::naming::part_path(&dest).exists(),
        "the part file must survive an interrupt"
    );
    assert!(
        state_path(&dest).exists(),
        "the .pdl.json sidecar must survive an interrupt"
    );
    server.abort();
}

// ---------------------------------------------------------------------------
// Public DownloadEngine trait adapter: exercise plan/download/capabilities and
// assert every exit-code path of the `Ok(TransferResult{...})` mapping.
// Expected codes are derived from `paradl_core::ExitCode`, never literals.
// ---------------------------------------------------------------------------

/// Sink that flips a cooperative cancel flag on its first progress tick.
struct CancelOnFirstTick(Arc<AtomicBool>);

impl ProgressSink for CancelOnFirstTick {
    fn on_progress(&mut self, _snapshot: &Snapshot) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn engine_trait_success_maps_to_ok_exit_code() {
    use sha2::Digest as _;

    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_fault_range_server(
        Arc::new(payload.clone()), Arc::new(AtomicUsize::new(0)),
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("trait-ok.bin");
    let digest_hex = hex::encode(sha2::Sha256::digest(&payload));
    let checksum = Hash::parse(&format!("sha256:{digest_hex}")).unwrap();
    let engine = NdEngine::new();

    let caps = engine.capabilities();
    assert_eq!(caps.max_connections, crate::MAX_CONNECTIONS);
    assert!(caps.supports_ranges && caps.supports_resume && caps.supports_checksum);
    assert!(!caps.http3, "HTTP/3 must never be advertised");

    let target = Target {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        checksum: Some(checksum),
        ..Target::default()
    };
    let plan = engine.plan(&target).await.unwrap();
    assert_eq!(plan.size, Some(payload.len() as u64));
    assert!(plan.resumable && !plan.unknown_size);
    assert_eq!(plan.final_url.host_str(), Some("127.0.0.1"));

    let mut sink = NoopProgressSink;
    let result = engine.download(plan, &mut sink).await.unwrap();
    assert!(result.success, "error: {:?}", result.error);
    assert_eq!(result.exit_code, ExitCode::Ok.as_i32());
    assert_eq!(result.checksum_verified, Some(true));
    assert_eq!(result.checksum_algo.as_deref(), Some("sha256"));
    assert_eq!(result.bytes_downloaded, payload.len() as u64);
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    server.abort();
}

#[tokio::test]
async fn engine_trait_checksum_mismatch_maps_to_verification_exit_code() {
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_fault_range_server(
        Arc::new(payload.clone()), Arc::new(AtomicUsize::new(0)),
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("trait-badsum.bin");
    let engine = NdEngine::new();
    let target = Target {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        checksum: Hash::parse(&format!("sha256:{}", "0".repeat(64))),
        ..Target::default()
    };

    let plan = engine.plan(&target).await.unwrap();
    let result = engine.download(plan, &mut NoopProgressSink).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.exit_code, ExitCode::VerificationFailed.as_i32());
    assert_eq!(result.checksum_verified, Some(false));
    assert!(!dest.exists(), "failed verification must not rename the part file");
    server.abort();
}

#[tokio::test]
async fn engine_trait_cancel_maps_to_interrupted_exit_code() {
    let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let (url, server) = spawn_slow_close_delimited_server(
        Arc::new(payload.clone()), Arc::new(AtomicUsize::new(0)),
    ).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("trait-cancel.bin");
    let flag = Arc::new(AtomicBool::new(false));
    let engine = NdEngine::new();
    let target = Target {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        cancel: Some(flag.clone()),
        ..Target::default()
    };

    let plan = engine.plan(&target).await.unwrap();
    let mut sink = CancelOnFirstTick(flag);
    let result = engine.download(plan, &mut sink).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.exit_code, ExitCode::Interrupted.as_i32());
    assert!(result.error.as_deref().is_some_and(|e| e.contains("cancel")));
    assert!(crate::naming::part_path(&dest).exists(), "part persists on interrupt");
    assert!(state_path(&dest).exists(), "sidecar persists on interrupt");
    server.abort();
}

#[tokio::test]
async fn engine_trait_transport_failure_maps_to_failure_exit_code() {
    let payload: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    // Probe succeeds but every ranged GET returns 500, with no mirror to fail over to.
    let (url, server) = spawn_probe_ok_range_500_server(Arc::new(payload)).await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("trait-fail.bin");
    let engine = NdEngine::new();
    let target = Target {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 2,
        ..Target::default()
    };

    let plan = engine.plan(&target).await.unwrap();
    let result = engine.download(plan, &mut NoopProgressSink).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.exit_code, ExitCode::Failure.as_i32());
    assert!(!dest.exists(), "a failed transfer must not produce the final file");
    server.abort();
}

// ---------------------------------------------------------------------------
// DoH contract (feature-gated): the endpoint HOST must be used as the TLS
// server name so SNI and certificate validation work for hostname endpoints.
// Pure config construction — no live network is touched.
// ---------------------------------------------------------------------------

#[cfg(feature = "doh")]
#[test]
fn doh_config_uses_endpoint_host_as_tls_server_name() {
    let (ns, host, port) = crate::probe::doh_nameserver_config(
        "https://cloudflare-dns.com/dns-query",
    ).unwrap();
    assert_eq!(host, "cloudflare-dns.com");
    assert_eq!(port, 443);
    assert_eq!(ns.tls_dns_name.as_deref(), Some("cloudflare-dns.com"));
    assert_eq!(ns.http_endpoint.as_deref(), Some("/dns-query"));

    let (ns, host, port) = crate::probe::doh_nameserver_config(
        "https://my-dns.example:8443/custom-path",
    ).unwrap();
    assert_eq!(host, "my-dns.example");
    assert_eq!(port, 8443);
    assert_eq!(ns.tls_dns_name.as_deref(), Some("my-dns.example"));
    assert_eq!(ns.http_endpoint.as_deref(), Some("/custom-path"));
}
