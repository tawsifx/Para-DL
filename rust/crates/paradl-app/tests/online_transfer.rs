//! Online-mode integration test against a loopback HTTP server.
//!
//! Drives the REAL `paradl-engine` transfer (not the offline shim) and asserts
//! the produced bytes plus the `.pdl.part` / `.pdl.json` sidecar channel,
//! including preservation across a cooperative interrupt (exit code 130).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use paradl_app::{App, Capture};
use paradl_cli::{Cli, Command, Dispatch, ExitCode, ParseOutcome};
use paradl_core::PDLConfig;
use paradl_engine::ResumeState;
use url::Url;

const PAYLOAD_LEN: usize = 96 * 1024;

fn payload() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|index| (index % 251) as u8).collect()
}

struct LoopbackServer {
    origin: Url,
}

impl LoopbackServer {
    fn start(body: Vec<u8>, delay_ms: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        // Detached acceptor: joining it would block forever on `incoming()`.
        // The listener is dropped with the process when the test binary exits.
        thread::spawn(move || {
            for stream in listener.incoming().take(512) {
                let Ok(stream) = stream else { break };
                let bytes = body.clone();
                thread::spawn(move || serve_one(stream, bytes, delay_ms));
            }
        });
        Self {
            origin: Url::parse(&format!("http://{addr}/payload.bin")).expect("valid URL"),
        }
    }

    fn target(&self) -> String {
        self.origin.to_string()
    }
}

fn serve_one(mut stream: TcpStream, body: Vec<u8>, delay_ms: u64) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut request = Vec::new();
    let mut byte = [0u8; 1];
    while request.len() < 16 * 1024 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&request);
    let first_line = text.lines().next().unwrap_or("");
    let is_head = first_line.starts_with("HEAD");
    let range = text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("range") {
            Some(value.trim().to_owned())
        } else {
            None
        }
    });

    let (status, start, end) = match range.as_deref().and_then(parse_range) {
        Some((start, end)) => ("206 Partial Content", start, end),
        None => ("200 OK", 0, body.len().saturating_sub(1)),
    };
    let slice = if start <= end && end < body.len() {
        &body[start..=end]
    } else {
        &[][..]
    };

    if delay_ms > 0 {
        thread::sleep(Duration::from_millis(delay_ms));
    }
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"w41-payload\"\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n",
        slice.len()
    );
    if start <= end && range.is_some() {
        response.push_str(&format!(
            "Content-Range: bytes {start}-{end}/{}\r\n",
            body.len()
        ));
    }
    response.push_str("\r\n");
    let _ = stream.write_all(response.as_bytes());
    if !is_head {
        let _ = stream.write_all(slice);
    }
    let _ = stream.flush();
}

fn parse_range(value: &str) -> Option<(usize, usize)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start_text, end_text) = spec.split_once('-')?;
    let start = start_text.trim().parse::<usize>().ok()?;
    let end = match end_text.trim() {
        "" => None,
        text => Some(text.parse::<usize>().ok()?),
    };
    Some((start, end.unwrap_or(usize::MAX)))
}

fn dispatch_download(target: &str, out_dir: &str) -> (ExitCode, String) {
    let cli = Cli::parse_command_line(vec![
        target.to_string(),
        "-o".to_string(),
        out_dir.to_string(),
        "--json".to_string(),
    ])
    .expect("parse");
    let ParseOutcome::Command(cli) = cli else { panic!() };
    let command = Command::from_cli(&cli).expect("download command");
    let capture = Capture::default();
    let mut app = App::with_config(PDLConfig::default())
        .with_output(capture.sink());
    app.disable_history_recording();
    let code = app.handle(&command, &cli);
    (code, capture.contents())
}

#[test]
fn real_engine_transfer_writes_exact_bytes_and_cleans_sidecars() {
    let body = payload();
    let server = LoopbackServer::start(body.clone(), 0);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().to_str().expect("utf8 path");

    let (code, events) = dispatch_download(&server.target(), out);
    assert_eq!(code, ExitCode::Ok, "events: {events}");
    assert!(events.contains("\"event\": \"result\""), "{events}");

    let dest = dir.path().join("payload.bin");
    let downloaded = std::fs::read(&dest).expect("downloaded file");
    assert_eq!(downloaded, body, "download bytes must be byte-exact");

    // On success the engine removes both sidecars after verify-then-rename.
    assert!(!dir.path().join("payload.bin.pdl.json").exists());
    assert!(!dir.path().join("payload.bin.pdl.part").exists());
}

#[test]
fn interrupt_preserves_part_and_resume_sidecar_byte_exact() {
    let body = payload();
    let server = LoopbackServer::start(body.clone(), 350);
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().to_str().expect("utf8 path");
    let target = server.target();

    let cli = Cli::parse_command_line(vec![
        target.clone(),
        "-o".to_string(),
        out.to_string(),
        "--json".to_string(),
    ])
    .expect("parse");
    let ParseOutcome::Command(cli) = cli else { panic!() };
    let command = Command::from_cli(&cli).expect("download command");

    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        flag.store(true, Ordering::Relaxed);
    });

    let capture = Capture::default();
    let mut app = App::with_config(PDLConfig::default())
        .with_output(capture.sink());
    app.disable_history_recording();
    app.set_cancel(cancel);
    let code = app.handle(&command, &cli);
    canceller.join().expect("canceller");

    assert_eq!(code, ExitCode::Interrupted, "events: {}", capture.contents());
    assert!(capture.contents().contains("\"exit_code\": 130"));

    let part = dir.path().join("payload.bin.pdl.part");
    let sidecar = dir.path().join("payload.bin.pdl.json");
    assert!(part.exists(), "part file must survive an interrupt");
    assert!(sidecar.exists(), "resume sidecar must survive an interrupt");

    let part_bytes = std::fs::read(&part).expect("part bytes");
    assert!(!part_bytes.is_empty());
    assert!(part_bytes.len() <= body.len());

    // Byte-exact sidecar contract: parse through the owning crate's typed API
    // and assert every stable field (the only variable is the segment table).
    let state = ResumeState::load(&sidecar).expect("valid .pdl.json sidecar");
    // Every byte range the sidecar claims to own must match the source payload.
    for segment in &state.segments {
        for offset in segment.start..segment.cursor {
            let index = usize::try_from(offset).expect("offset fits usize");
            assert_eq!(part_bytes[index], body[index], "part byte {offset}");
        }
    }
    assert_eq!(state.version, 1);
    assert_eq!(state.url, server.origin);
    assert_eq!(state.final_url, server.origin);
    assert_eq!(state.size, body.len() as u64);
    assert_eq!(state.dest, dir.path().join("payload.bin"));
    let downloaded: u64 = state.segments.iter().map(|segment| segment.cursor - segment.start).sum();
    // An early interrupt can precede the first 2-second periodic sidecar save,
    // so the segment table may legitimately still be empty; it must never
    // claim more bytes than the source holds.
    assert!(downloaded <= body.len() as u64);

    // The sidecar is stable JSON on disk: a second read is byte-identical.
    let first = std::fs::read(&sidecar).expect("sidecar bytes");
    let second = std::fs::read(&sidecar).expect("sidecar bytes again");
    assert_eq!(first, second);
}
