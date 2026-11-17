// SPDX-License-Identifier: GPL-3.0-or-later
// Ported from Mister Download Manager (c) Ahmed Nefzaoui — https://github.com/anefzaoui/misterdownloadmanager
// Adapted for paradl by Tawsif Rahman, 2026-11-17. Tests written first (TDD).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::download::{Download, Outcome, StartOptions};
use crate::error::Error;
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
    let part = std::fs::File::create(dir.path().join("a.bin.mdm.part")).unwrap();
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
        dir.path().join("a.bin.mdm.part")
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
/// Adapted from MDM's labserver *pattern*; written from scratch here, not copied.
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
    assert!(matches!(err, Error::ChecksumMismatch), "got {err:?}");
    assert!(!dest.exists(), "a failed verification must not rename the part into place");
    server.abort();
}
