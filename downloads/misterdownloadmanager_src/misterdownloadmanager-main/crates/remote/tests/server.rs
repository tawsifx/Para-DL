//! The remote control end to end on this computer (127.0.0.1 only): the page, pairing, the
//! checks every request goes through, and commands reaching the app marked as remote.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_remote::{Config, Remote, Status};

/// A remote control on 127.0.0.1, any port, with a stand-in app that answers every command
/// with `{"seen": …}` and records it.
fn start(
    rt: &tokio::runtime::Runtime,
) -> (
    Remote,
    String,
    Arc<Mutex<Vec<mdm_ipc::Command>>>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<mdm_ipc::Incoming>();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        while let Ok(inc) = rx.recv() {
            let answer = serde_json::json!({
                "seen": inc.cmd.query.clone().or(inc.cmd.action.clone()).or(inc.cmd.url.clone()),
                "remote": inc.cmd.remote,
            })
            .to_string();
            log.lock().unwrap().push(inc.cmd);
            if let Some(r) = inc.reply {
                let _ = r.send(answer);
            }
        }
    });
    let sink = mdm_ipc::Sink::new(tx, Arc::new(|| {}));
    let texts: mdm_remote::Texts = Arc::new(|lang: &str| mdm_remote::PageTexts {
        lang: if lang.starts_with("de") {
            "de".into()
        } else {
            "en".into()
        },
        rtl: false,
        texts: [("Pair".to_owned(), "Koppeln".to_owned())].into(),
    });
    let remote = Remote::new(rt.handle().clone(), dir.path().join("remote"), sink, texts);
    remote.apply(Config {
        on: true,
        port: 0,
        listen: "127.0.0.1".into(),
        hosts: vec!["mdm.example".into()],
    });
    let Status::Listening(addrs) = remote.status() else {
        panic!("not listening: {:?}", remote.status());
    };
    assert_eq!(addrs.len(), 1);
    assert!(addrs[0].ip().is_loopback());
    let base = format!("https://127.0.0.1:{}/", addrs[0].port());
    (remote, base, seen, dir)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap()
}

#[test]
fn pair_then_control() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (remote, base, seen, _dir) = start(&rt);
    rt.block_on(async {
        let c = client();
        // The page, hardened.
        let page = c.get(&base).send().await.unwrap();
        assert_eq!(page.status(), 200);
        let csp = page.headers()["content-security-policy"].to_str().unwrap().to_owned();
        assert!(csp.contains("frame-ancestors 'none'") && csp.contains("script-src 'self'"));
        assert_eq!(page.headers()["x-frame-options"], "DENY");
        assert!(page.text().await.unwrap().contains("app.js"));
        // Texts in the browser's language.
        let t: serde_json::Value = c
            .get(format!("{base}api/texts?lang=de-DE,de"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(t["lang"], "de");
        assert_eq!(t["texts"]["Pair"], "Koppeln");

        // Nothing without pairing.
        let r = c.get(format!("{base}api/list")).send().await.unwrap();
        assert_eq!(r.status(), 401);
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth("0123456789abcdef")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "a made-up token");

        // No code shown: refused.
        let pair = |code: &str| {
            c.post(format!("{base}api/pair"))
                .json(&serde_json::json!({ "code": code, "name": "Test phone" }))
                .send()
        };
        assert_eq!(pair("12345678").await.unwrap().status(), 410);
        let code = remote.start_pairing();
        tokio::time::sleep(mdm_remote::pairing::PACE).await;
        let wrong = if code == "00000000" { "11111111" } else { "00000000" };
        let r = pair(wrong).await.unwrap();
        assert_eq!(r.status(), 403);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["left"], 4);
        // Too soon after a wrong one, even with the right code.
        assert_eq!(pair(&code).await.unwrap().status(), 429);
        tokio::time::sleep(mdm_remote::pairing::PACE).await;
        let r = pair(&code).await.unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        let token = body["token"].as_str().unwrap().to_owned();
        assert_eq!(remote.pairing().1.as_deref(), Some("Test phone"));
        assert_eq!(remote.devices().len(), 1);

        // Paired: commands reach the app, marked remote.
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["seen"], "list");
        assert_eq!(body["remote"], true);
        let r = c
            .post(format!("{base}api/add"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "url": "https://example.com/a.zip", "queue": "Main download queue" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        {
            let log = seen.lock().unwrap();
            let add = log.last().unwrap();
            assert!(add.remote && add.silent && add.later);
            assert_eq!(add.queue.as_deref(), Some("Main download queue"));
        }
        // A torrent file: its bytes, never a path.
        let r = c
            .post(format!("{base}api/torrent?start=true"))
            .bearer_auth(&token)
            .body(b"d4:infod4:name1:xee".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        {
            let log = seen.lock().unwrap();
            let t = log.last().unwrap();
            assert!(t.torrent_file.is_some() && t.url.is_none() && !t.later);
        }
        let r = c
            .post(format!("{base}api/torrent"))
            .bearer_auth(&token)
            .body(b"<html>".to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        // Only the list's actions.
        let r = c
            .post(format!("{base}api/act"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "action": "exit", "ids": [] }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);

        // Another site's page, a rebinding name: refused before anything is looked at.
        let r = c
            .post(format!("{base}api/add"))
            .bearer_auth(&token)
            .header("origin", "https://evil.example")
            .json(&serde_json::json!({ "url": "https://example.com/b.zip" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403);
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth(&token)
            .header("host", "evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 421);
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth(&token)
            .header("host", "mdm.example")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "a name the user added");
        let own = base.trim_end_matches('/').to_owned();
        let r = c
            .post(format!("{base}api/act"))
            .bearer_auth(&token)
            .header("origin", own)
            .json(&serde_json::json!({ "action": "pause", "ids": [1] }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "its own page");

        // The page opened from a link on another site: shown (it holds nothing).
        let r = c
            .get(&base)
            .header("sec-fetch-site", "cross-site")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth(&token)
            .header("sec-fetch-site", "cross-site")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "but its API isn't for other sites");

        // Plain http: sent on to https.
        let plain = base.replacen("https", "http", 1);
        let r = c.get(&plain).send().await.unwrap();
        assert_eq!(r.status(), 301);
        assert_eq!(r.headers()["location"], base.as_str());

        // Removed in Options: refused at once.
        let id = remote.devices()[0].id.clone();
        remote.remove(&id);
        let r = c
            .get(format!("{base}api/list"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401);
    });
    // Turned off: the port is let go.
    let port = base
        .rsplit(':')
        .next()
        .unwrap()
        .trim_end_matches('/')
        .parse::<u16>()
        .unwrap();
    remote.stop();
    assert_eq!(remote.status(), Status::Off);
    std::thread::sleep(Duration::from_millis(300));
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}

#[test]
fn never_every_address() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (tx, _rx) = std::sync::mpsc::channel();
    let dir = tempfile::tempdir().unwrap();
    let texts: mdm_remote::Texts = Arc::new(|_: &str| mdm_remote::PageTexts::default());
    let remote = Remote::new(
        rt.handle().clone(),
        dir.path().to_owned(),
        mdm_ipc::Sink::new(tx, Arc::new(|| {})),
        texts,
    );
    for listen in ["0.0.0.0", "::", "203.0.113.9"] {
        remote.apply(Config {
            on: true,
            port: 0,
            listen: listen.into(),
            hosts: Vec::new(),
        });
        assert_eq!(remote.status(), Status::NoAddress, "{listen}");
    }
}
