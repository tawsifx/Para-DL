//! Runnable demonstration of the `DownloadEngine` trait.
//!
//! Starts a loopback HTTP Range server so the example needs no external
//! network, then plans and executes a transfer while printing progress.

use std::sync::Arc;
use std::time::Duration;

use paradl_engine::{DownloadEngine, NdEngine, ProgressSink, Snapshot, Target};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Minimal loopback server that answers probes with 200 and ranged GETs with 206.
async fn spawn_range_server(payload: Arc<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
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
                let _ = sock.write_all(chunk).await;
            });
        }
    });
    format!("http://{addr}/payload.bin")
}

struct Printer;

impl ProgressSink for Printer {
    fn on_progress(&mut self, snapshot: &Snapshot) {
        println!(
            "  progress: {} bytes, {} connection(s), state {:?}",
            snapshot.downloaded, snapshot.connections, snapshot.state
        );
    }
}

#[tokio::main]
async fn main() {
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let url = spawn_range_server(Arc::new(payload.clone())).await;

    let dir = std::env::temp_dir().join(format!("paradl-engine-example-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let dest = dir.join("payload.bin");

    let engine = NdEngine::new();
    let target = Target {
        url: url.parse().unwrap(),
        dest: dest.clone(),
        connections: 4,
        ..Target::default()
    };

    let plan = engine.plan(&target).await.expect("planning failed");
    println!(
        "planned {} bytes from {} (resumable: {})",
        plan.size.unwrap_or(0),
        plan.final_url,
        plan.resumable
    );

    // Keep a short tick so the example finishes quickly.
    let engine = NdEngine {
        progress_interval: Duration::from_millis(20),
        ..engine
    };
    let result = engine
        .download(plan, &mut Printer)
        .await
        .expect("download failed");

    println!(
        "result: success={} exit_code={} bytes={}",
        result.success, result.exit_code, result.bytes_downloaded
    );
    assert!(result.success);
    assert_eq!(std::fs::read(&dest).unwrap(), payload, "file must be byte-exact");
    println!("verified byte-exact");

    let _ = std::fs::remove_file(&dest);
}
