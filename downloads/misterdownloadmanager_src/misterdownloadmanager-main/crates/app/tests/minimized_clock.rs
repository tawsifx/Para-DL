//! #174: while the main window is minimized (not to the notification area), egui runs only the
//! app's logic, never its UI. The queues, the Scheduler's starts and the list's saving ran in the
//! UI, so a minimized MDM started nothing until it was restored. The real app, off the screens
//! (MDM_OFFSCREEN), with a data folder of its own and the lab server on 127.0.0.1: its main
//! window minimized (`/show minimized`, no input), a queue of three files, one at a time,
//! scheduled to start a few seconds ahead. Every file must come down, one after the other, and
//! the Download complete windows open.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test minimized_clock -- --ignored`.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use mdm_labserver::{Config, LabServer};

const APP: &str = env!("CARGO_BIN_EXE_misterdownloadmanager");

/// The app's log so far (every file of `logs/`).
fn log(data: &Path) -> String {
    let mut all = String::new();
    if let Ok(dir) = std::fs::read_dir(data.join("logs")) {
        for f in dir.flatten() {
            all += &std::fs::read_to_string(f.path()).unwrap_or_default();
        }
    }
    all
}

/// Wait until `done` holds, failing if the app quits first or `wait` passes.
fn wait_until(app: &mut Child, data: &Path, what: &str, wait: Duration, done: impl Fn() -> bool) {
    let until = Instant::now() + wait;
    loop {
        let text = log(data);
        assert!(!text.contains("panic"), "the app panicked:\n{text}");
        if done() {
            return;
        }
        if let Some(status) = app.try_wait().unwrap() {
            panic!("the app closed ({status}) before {what}:\n{text}");
        }
        assert!(Instant::now() < until, "no {what} after {wait:?}:\n{text}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn second_launch(data: &Path, args: &[&str]) {
    let status = Command::new(APP)
        .args(args)
        .env("MDM_DATA_DIR", data)
        .env("MDM_OFFSCREEN", "1")
        .status()
        .unwrap();
    assert!(status.success(), "second launch {args:?}: {status}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "opens the app's windows (off-screen): needs OpenGL or Direct3D; run with --ignored"]
async fn a_scheduled_queue_runs_while_minimized() {
    let server = LabServer::start(Config::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let files = dir.path().join("files");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&files).unwrap();
    let items: Vec<serde_json::Value> = (1..=3)
        .map(|i| {
            serde_json::json!({
                "id": i,
                "url": server.url(&format!("/gen/1M/q{i}.bin?seed={i}&rate=128K")),
                "file_name": format!("q{i}.bin"),
                "dest": files.join(format!("q{i}.bin")),
                "size": null, "downloaded": 0, "status": "Queued", "category": "General",
                "description": "", "resumable": false, "added": "Nov 06 10:00:00 2026",
                "log": [],
            })
        })
        .collect();
    // The start: a little ahead, so the window is minimized well before it.
    let at = chrono::Local::now() + chrono::Duration::seconds(12);
    use chrono::Timelike;
    let schedule = serde_json::json!({
        "start_at": [at.hour(), at.minute(), at.second()], "days": [true, true, true, true, true, true, true],
    });
    let list = serde_json::json!({
        "version": 2, "next_id": 4, "items": items,
        "queues": [
            { "name": "Main download queue", "files_at_once": 1, "items": [1, 2, 3],
              "sync": false, "schedule": schedule },
            { "name": "Synchronization queue", "files_at_once": 4, "items": [], "sync": true },
        ],
        "settings": {
            "language": "en", "language_asked": true, "helpers_asked": true,
            "extension_never_ask": true, "update_check": false, "clipboard_autostart": false,
            "minimize_to_tray": false, "show_complete": true,
            "torrent_dht": false, "torrent_lsd": false, "torrent_upnp": false,
            "torrent_incoming": false,
        },
    });
    std::fs::write(data.join("downloads.json"), list.to_string()).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        let mut app = Command::new(APP)
            .env("MDM_DATA_DIR", &data)
            .env("MDM_OFFSCREEN", "1")
            .spawn()
            .unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let wait = Duration::from_secs(60);
            wait_until(&mut app, &data, "main window", wait, || {
                log(&data).contains("window opened: main")
            });
            second_launch(&data, &["/show", "minimized"]);
            let done = |i: u32| {
                std::fs::metadata(files.join(format!("q{i}.bin"))).is_ok_and(|m| m.len() == 1 << 20)
            };
            assert!(!done(1), "nothing before the scheduled start");
            wait_until(&mut app, &data, "the three files", wait, || {
                (1..=3).all(done)
            });
            wait_until(&mut app, &data, "Download complete", wait, || {
                log(&data).contains("window opened: complete-3")
            });
        }));
        let _ = Command::new(APP)
            .arg("/exit")
            .env("MDM_DATA_DIR", &data)
            .env("MDM_OFFSCREEN", "1")
            .status();
        let until = Instant::now() + Duration::from_secs(15);
        while app.try_wait().unwrap().is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = app.kill();
        let _ = app.wait();
        outcome
    })
    .await
    .unwrap();
    // One file at a time: each was asked for once its predecessor had come down.
    let log = server.log();
    let first_ask = |i: u32| {
        log.iter()
            .filter(|e| e.path.ends_with(&format!("q{i}.bin")))
            .map(|e| e.t_start_ms)
            .min()
    };
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    let (a, b, c) = (first_ask(1), first_ask(2), first_ask(3));
    assert!(
        a.is_some() && b.is_some() && c.is_some(),
        "{a:?} {b:?} {c:?}"
    );
    assert!(a <= b && b <= c, "in queue order: {a:?} {b:?} {c:?}");
}
