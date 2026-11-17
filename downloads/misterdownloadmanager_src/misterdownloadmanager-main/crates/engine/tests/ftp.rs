//! End-to-end over FTP and FTPS: the engine downloads from the lab FTP server and the file is
//! verified byte for byte against the generator.

use std::time::Duration;

use mdm_engine::{Destination, Download, Error, Options, Request};
use mdm_labserver::content;
use mdm_labserver::ftp::{FtpConfig, FtpServer, FtpTls};
use reqwest::header::{AUTHORIZATION, HeaderMap};

async fn server(config: FtpConfig) -> FtpServer {
    FtpServer::start("127.0.0.1:0".parse().unwrap(), config)
        .await
        .unwrap()
}

/// Fast retries; an FTPS server's test CA trusted and `localhost` at the server's address.
fn opts(s: &FtpServer) -> Options {
    Options {
        retry_delay: Duration::from_millis(50),
        extra_roots: s.ca_der().into_iter().collect(),
        hosts: vec![("localhost".into(), vec!["127.0.0.1".parse().unwrap()])],
        ..Options::default()
    }
}

fn request(url: String, headers: HeaderMap, dir: &std::path::Path) -> Request {
    Request {
        url: url.parse().unwrap(),
        headers,
        dest: Destination::Dir(dir.to_owned()),
    }
}

/// The file at `path` downloaded whole and right; its size.
async fn fetch(s: &FtpServer, path: &str, seed: u64, opts: Options, headers: HeaderMap) -> u64 {
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(request(s.url(path), headers, dir.path()), opts)
        .wait()
        .await
        .expect("download succeeds");
    check(&out.path, seed);
    out.size
}

fn check(path: &std::path::Path, seed: u64) {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(seed, 0, &mut want);
    assert!(data == want, "content mismatch");
}

#[tokio::test]
async fn a_file_comes_over_several_connections() {
    let s = server(FtpConfig {
        rate: Some(2 << 20),
        ..FtpConfig::default()
    })
    .await;
    let size = fetch(&s, "/gen/4M/s3/a.bin", 3, opts(&s), HeaderMap::new()).await;
    assert_eq!(size, 4 << 20);
    assert!(s.peak_transfers() >= 4, "parallel: {}", s.peak_transfers());
}

#[tokio::test]
async fn sessions_are_reused_between_parts() {
    // Every other session is slow: the fast ones finish early and take over the rest of the
    // slow ones' parts, on the sessions they already have (no new connect and login).
    let s = server(FtpConfig {
        rate: Some(4 << 20),
        slow_every: Some(2),
        slow_rate: 512 << 10,
        ..FtpConfig::default()
    })
    .await;
    let o = Options {
        max_connections: 4,
        ..opts(&s)
    };
    fetch(&s, "/gen/8M/s2/z.bin", 2, o, HeaderMap::new()).await;
    assert!(
        s.transfers() > s.sessions(),
        "{} sessions for {} transfers",
        s.sessions(),
        s.transfers()
    );
}

#[tokio::test]
async fn active_mode_works_too() {
    let s = server(FtpConfig {
        rate: Some(4 << 20),
        ..FtpConfig::default()
    })
    .await;
    let o = Options {
        ftp_passive: false,
        ..opts(&s)
    };
    assert_eq!(
        fetch(&s, "/gen/1M/s4/b.bin", 4, o, HeaderMap::new()).await,
        1 << 20
    );
}

#[tokio::test]
async fn the_login_comes_from_the_address_or_the_authorization_header() {
    let s = server(FtpConfig {
        login: Some(("ann".into(), "p@ss".into())),
        ..FtpConfig::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let url = s
        .url("/gen/256K/c.bin")
        .replace("ftp://", "ftp://ann:p%40ss@");
    let out = Download::start(request(url, HeaderMap::new(), dir.path()), opts(&s))
        .wait()
        .await
        .expect("logged in from the address");
    check(&out.path, 0);
    // "Use authorization" / Sites Logins arrive as a Basic header.
    let mut h = HeaderMap::new();
    h.insert(AUTHORIZATION, "Basic YW5uOnBAc3M=".parse().unwrap()); // ann:p@ss
    fetch(&s, "/gen/256K/d.bin", 0, opts(&s), h).await;
    // A wrong login fails at once, without retrying.
    let started = std::time::Instant::now();
    let err = Download::start(
        request(s.url("/gen/256K/e.bin"), HeaderMap::new(), dir.path()),
        opts(&s),
    )
    .wait()
    .await
    .unwrap_err();
    assert!(matches!(err, Error::FtpReply { code: 530, .. }), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn a_missing_file_says_so() {
    let s = server(FtpConfig::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let err = Download::start(
        request(s.url("/nothing/here.bin"), HeaderMap::new(), dir.path()),
        opts(&s),
    )
    .wait()
    .await
    .unwrap_err();
    assert!(matches!(err, Error::FtpReply { code: 550, .. }), "{err}");
}

#[tokio::test]
async fn transfers_cut_short_are_continued() {
    let s = server(FtpConfig {
        cut: Some(200 << 10),
        ..FtpConfig::default()
    })
    .await;
    assert_eq!(
        fetch(&s, "/gen/2M/s5/f.bin", 5, opts(&s), HeaderMap::new()).await,
        2 << 20
    );
}

#[tokio::test]
async fn a_connection_limit_is_respected() {
    let s = server(FtpConfig {
        max_sessions: Some(2),
        rate: Some(2 << 20),
        ..FtpConfig::default()
    })
    .await;
    assert_eq!(
        fetch(&s, "/gen/2M/s6/g.bin", 6, opts(&s), HeaderMap::new()).await,
        2 << 20
    );
    assert!(s.refused() > 0, "the limit was hit");
}

#[tokio::test]
async fn a_server_without_rest_sends_the_file_on_one_connection() {
    let s = server(FtpConfig {
        rest: false,
        rate: Some(4 << 20),
        ..FtpConfig::default()
    })
    .await;
    assert_eq!(
        fetch(&s, "/gen/1M/s8/h.bin", 8, opts(&s), HeaderMap::new()).await,
        1 << 20
    );
}

/// Start a download of `path`, stop it once some data is on disk: the state file.
async fn paused(s: &FtpServer, path: &str, dir: &std::path::Path) -> std::path::PathBuf {
    let o = Options {
        save_interval: Duration::from_millis(100),
        ..opts(s)
    };
    let dl = Download::start(request(s.url(path), HeaderMap::new(), dir), o);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded > 0) {
            break f;
        }
        assert!(std::time::Instant::now() < deadline, "never started");
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    dl.cancel();
    let r = dl.wait().await;
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    mdm_engine::state::state_path(&file)
}

#[tokio::test]
async fn pause_and_resume_continues_where_it_stopped() {
    let s = server(FtpConfig {
        rate: Some(256 << 10),
        ..FtpConfig::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let state = paused(&s, "/gen/4M/s9/r.bin", dir.path()).await;
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert!(saved.downloaded() > 0 && saved.downloaded() < 4 << 20);
    assert!(saved.validators.last_modified.is_some(), "MDTM kept");
    let out = Download::resume(state.clone(), HeaderMap::new(), opts(&s))
        .unwrap()
        .wait()
        .await
        .expect("resumed");
    check(&out.path, 9);
    assert!(!state.exists());
}

#[tokio::test]
async fn resume_detects_a_changed_file() {
    let s = server(FtpConfig {
        rate: Some(256 << 10),
        ..FtpConfig::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let state = paused(&s, "/gen/4M/s10/x.bin", dir.path()).await;
    s.set_modified("20261001090000");
    let err = Download::resume(state.clone(), HeaderMap::new(), opts(&s))
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, Error::FileChanged), "{err}");
    // Unless modification times are to be ignored.
    let o = Options {
        ignore_mtime: true,
        ..opts(&s)
    };
    let out = Download::resume(state, HeaderMap::new(), o)
        .unwrap()
        .wait()
        .await
        .expect("resumed despite the new date");
    check(&out.path, 10);
}

#[tokio::test]
async fn the_password_is_not_written_to_the_resume_state() {
    let s = server(FtpConfig {
        login: Some(("ann".into(), "secret".into())),
        rate: Some(256 << 10),
        ..FtpConfig::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let url = s
        .url("/gen/4M/s11/p.bin")
        .replace("ftp://", "ftp://ann:secret@");
    let o = Options {
        save_interval: Duration::from_millis(100),
        ..opts(&s)
    };
    let dl = Download::start(request(url, HeaderMap::new(), dir.path()), o);
    let state = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded > 0) {
            break mdm_engine::state::state_path(&f);
        }
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    dl.cancel();
    let _ = dl.wait().await;
    let text = std::fs::read_to_string(&state).unwrap();
    assert!(!text.contains("secret"), "{text}");
    // Resumed with the login given again, as the app does.
    let mut h = HeaderMap::new();
    h.insert(AUTHORIZATION, "Basic YW5uOnNlY3JldA==".parse().unwrap()); // ann:secret
    let out = Download::resume(state, h, opts(&s))
        .unwrap()
        .wait()
        .await
        .expect("resumed with the login");
    check(&out.path, 11);
}

#[tokio::test]
async fn explicit_ftps_secures_control_and_data() {
    let s = server(FtpConfig {
        tls: FtpTls::Explicit,
        rate: Some(4 << 20),
        ..FtpConfig::default()
    })
    .await;
    assert!(s.url("/").starts_with("ftpes://localhost:"));
    assert_eq!(
        fetch(&s, "/gen/1M/s12/t.bin", 12, opts(&s), HeaderMap::new()).await,
        1 << 20
    );
    assert!(s.peak_transfers() >= 2);
}

#[tokio::test]
async fn implicit_ftps_works() {
    let s = server(FtpConfig {
        tls: FtpTls::Implicit,
        ..FtpConfig::default()
    })
    .await;
    assert_eq!(
        fetch(&s, "/gen/512K/s13/u.bin", 13, opts(&s), HeaderMap::new()).await,
        512 << 10
    );
}

#[tokio::test]
async fn an_untrusted_certificate_is_refused() {
    let s = server(FtpConfig {
        tls: FtpTls::Explicit,
        ..FtpConfig::default()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let o = Options {
        extra_roots: Vec::new(),
        ..opts(&s)
    };
    let err = Download::start(
        request(s.url("/gen/64K/v.bin"), HeaderMap::new(), dir.path()),
        o,
    )
    .wait()
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Net {
                kind: mdm_engine::NetKind::Tls,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn probe_tells_size_and_name() {
    let s = server(FtpConfig::default()).await;
    let info = mdm_engine::probe(
        s.url("/gen/300K/s1/name.iso").parse().unwrap(),
        HeaderMap::new(),
        &opts(&s),
    )
    .await
    .unwrap();
    assert_eq!(info.size, Some(300 << 10));
    assert_eq!(info.file_name, "name.iso");
}
