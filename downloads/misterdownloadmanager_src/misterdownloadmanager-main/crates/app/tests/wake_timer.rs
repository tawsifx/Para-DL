//! #142: Scheduler → "Wake the computer to run this queue". The real app, off the screens
//! (MDM_OFFSCREEN), with a data folder of its own and the lab server on 127.0.0.1: a queue with
//! the option, "Once at" today, starting 75 s ahead. The computer is never put to sleep: the
//! test checks that the wake is set a minute before the start (and on Windows that the wake timer
//! goes off then), that the queue starts at its time and keeps the computer awake while it runs,
//! and that the wake is cancelled once there is no start left to wake for.
//!
//! It opens windows, which needs OpenGL or Direct3D, so it runs on request:
//! `cargo test -p mdm-app --test wake_timer -- --ignored --nocapture`.

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

/// The log's line holding `what`, waited for (failing if the app quits first or `wait` passes).
fn wait_for(app: &mut Child, data: &Path, what: &str, wait: Duration) -> String {
    let until = Instant::now() + wait;
    loop {
        let text = log(data);
        assert!(!text.contains("panic"), "the app panicked:\n{text}");
        if let Some(line) = text.lines().find(|l| l.contains(what)) {
            println!("{line}");
            return line.to_owned();
        }
        if let Some(status) = app.try_wait().unwrap() {
            panic!("the app closed ({status}) before {what:?}:\n{text}");
        }
        assert!(
            Instant::now() < until,
            "no {what:?} after {wait:?}:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "opens the app's windows (off-screen) and takes 90 s; run with --ignored"]
async fn the_computer_is_woken_for_the_queue_and_kept_awake_while_it_runs() {
    let server = LabServer::start(Config::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let files = dir.path().join("files");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&files).unwrap();
    let item = serde_json::json!({
        "id": 1,
        "url": server.url("/gen/2M/w1.bin?seed=1&rate=256K"),
        "file_name": "w1.bin",
        "dest": files.join("w1.bin"),
        "size": null, "downloaded": 0, "status": "Queued", "category": "General",
        "description": "", "resumable": false, "added": "Nov 06 10:00:00 2026",
        "log": [],
    });
    let start = chrono::Local::now() + chrono::Duration::seconds(75);
    use chrono::{Datelike, Timelike};
    let schedule = serde_json::json!({
        "start_at": [start.hour(), start.minute(), start.second()],
        "once": [start.year(), start.month(), start.day()],
        "wake": true,
    });
    let list = serde_json::json!({
        "version": 2, "next_id": 2, "items": [item],
        "queues": [
            { "name": "Main download queue", "files_at_once": 1, "items": [1],
              "sync": false, "schedule": schedule },
            { "name": "Synchronization queue", "files_at_once": 4, "items": [], "sync": true },
        ],
        "settings": {
            "language": "en", "language_asked": true, "helpers_asked": true,
            "extension_never_ask": true, "update_check": false, "clipboard_autostart": false,
            "minimize_to_tray": false, "show_complete": false,
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
            // Set at once, for a minute before the start.
            let set = wait_for(
                &mut app,
                &data,
                "wake: the computer wakes at",
                Duration::from_secs(30),
            );
            let wake = (start - chrono::Duration::seconds(60))
                .with_timezone(&chrono::Utc)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            assert!(set.contains(&wake), "{set:?} is not for {wake} UTC");
            assert!(set.contains("Main download queue"), "{set}");
            // Windows: the timer itself goes off then (the computer, awake, just goes on).
            #[cfg(windows)]
            wait_for(
                &mut app,
                &data,
                "wake: the wake timer went off",
                Duration::from_secs(40),
            );
            // Linux, where users may set the wake alarm: the chip has it.
            #[cfg(target_os = "linux")]
            if std::fs::OpenOptions::new()
                .write(true)
                .open("/sys/class/rtc/rtc0/wakealarm")
                .is_ok()
            {
                let alarm = std::fs::read_to_string("/sys/class/rtc/rtc0/wakealarm").unwrap();
                println!("wakealarm: {}", alarm.trim());
            }
            let done = || std::fs::metadata(files.join("w1.bin")).is_ok_and(|m| m.len() == 2 << 20);
            assert!(!done(), "nothing before the scheduled start");
            // The start, and the computer kept awake while the queue runs.
            wait_for(
                &mut app,
                &data,
                "wake: keeping the computer awake",
                Duration::from_secs(70),
            );
            // "Once at" today: nothing left to wake for after the start.
            wait_for(
                &mut app,
                &data,
                "wake: no wake set",
                Duration::from_secs(10),
            );
            let until = Instant::now() + Duration::from_secs(60);
            while !done() {
                assert!(Instant::now() < until, "w1.bin didn't come down");
                std::thread::sleep(Duration::from_millis(250));
            }
            // Done: the computer may sleep again.
            wait_for(
                &mut app,
                &data,
                "wake: no longer keeping the computer awake",
                Duration::from_secs(10),
            );
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
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
