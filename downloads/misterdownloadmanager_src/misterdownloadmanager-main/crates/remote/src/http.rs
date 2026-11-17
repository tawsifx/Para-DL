//! The remote control's web server: the page (`web/`, built in) and its API. Every API request
//! but pairing and the page's texts needs a paired device's token (`Authorization: Bearer …`);
//! every request must name this server (`Host`) and come from its own page (`Origin`), and
//! every answer forbids framing, sniffing and caching.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Query, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::Inner;
use crate::pairing::Check;

/// The largest .torrent file taken (as the app's own limit, mdm_torrent::MAX_TORRENT_FILE).
const MAX_TORRENT: usize = 10 << 20;
/// The largest other request body.
const MAX_BODY: usize = 64 << 10;
/// The longest address taken.
const MAX_URL: usize = 8192;
/// How long the app gets to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(5);

pub struct State {
    inner: Arc<Inner>,
    pub hosts: Vec<String>,
    generation: u64,
}

impl State {
    pub fn new(inner: Arc<Inner>, hosts: Vec<String>, generation: u64) -> Arc<State> {
        Arc::new(State {
            inner,
            hosts,
            generation,
        })
    }
}

type S = axum::extract::State<Arc<State>>;

/// The connection's other end (for the log of pairing tries; tokens are never logged).
#[derive(Clone, Copy, Debug)]
pub struct Peer(pub SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, crate::tls::Listener>>
    for Peer
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, crate::tls::Listener>) -> Self {
        Peer(*stream.remote_addr())
    }
}

pub fn router(state: Arc<State>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", PAGE) }),
        )
        .route(
            "/app.js",
            get(|| async { asset("text/javascript; charset=utf-8", SCRIPT) }),
        )
        .route(
            "/app.css",
            get(|| async { asset("text/css; charset=utf-8", STYLE) }),
        )
        .route(
            "/icon.png",
            get(|| async { ([(header::CONTENT_TYPE, "image/png")], ICON).into_response() }),
        )
        .route(
            "/manifest.webmanifest",
            get(|| async { asset("application/manifest+json", MANIFEST) }),
        )
        .route("/api/texts", get(texts))
        .route("/api/pair", post(pair))
        .route("/api/list", get(list))
        .route("/api/add", post(add))
        .route(
            "/api/torrent",
            post(torrent).layer(DefaultBodyLimit::max(MAX_TORRENT)),
        )
        .route("/api/act", post(act))
        .route("/api/queue", post(queue))
        .route("/api/forget", post(forget))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

static PAGE: &str = include_str!("../web/index.html");
static SCRIPT: &str = include_str!("../web/app.js");
static STYLE: &str = include_str!("../web/app.css");
/// The app's icon (art/), for the page's tab and a phone's home screen.
static ICON: &[u8] = include_bytes!("../../app/assets/icons/app/mdm-256.png");
static MANIFEST: &str = include_str!("../web/manifest.webmanifest");

fn asset(kind: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, kind)], body).into_response()
}

/// An error answer, shaped as the automation API's (`mdm_ipc::api::ErrorAnswer`).
fn error(status: StatusCode, code: &str) -> Response {
    let body = mdm_ipc::api::line(&mdm_ipc::api::ErrorAnswer {
        error: code.replace('-', " "),
        code: Some(code.to_owned()),
    });
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn json(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Checks every request before it is handled, and hardens every answer.
async fn guard(s: S, req: Request, next: Next) -> Response {
    // A connection opened before the remote control was turned off or moved.
    if s.inner.state.lock().unwrap().generation != s.generation {
        return error(StatusCode::SERVICE_UNAVAILABLE, "stopped");
    }
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    // DNS rebinding: a web page's own name pointed at this computer.
    if !crate::addrs::host_allowed(crate::addrs::host_of(&host), &s.hosts) {
        return error(StatusCode::MISDIRECTED_REQUEST, "unknown-host");
    }
    // Another site's page sending requests here (its scripts can't read the token, but none of
    // its requests are taken either). The page itself may be opened from a link anywhere.
    if req.uri().path().starts_with("/api/") && !same_origin(headers, &host) {
        return error(StatusCode::FORBIDDEN, "other-origin");
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    for (name, value) in [
        (
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
             connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'none'; \
             frame-ancestors 'none'",
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::CACHE_CONTROL, "no-store"),
    ] {
        h.insert(name, HeaderValue::from_static(value));
    }
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    res
}

/// No `Origin`, or this server's own (`https://` + the `Host` asked for); and not a request a
/// browser marks as sent from another site.
fn same_origin(headers: &HeaderMap, host: &str) -> bool {
    let cross = headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("cross-site"));
    let origin = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok());
    // MDM's browser extension (#157): its requests carry its own origin, which no web page can
    // send, and a token like any other.
    if origin.is_some_and(|o| {
        [
            "chrome-extension://",
            "moz-extension://",
            "safari-web-extension://",
        ]
        .iter()
        .any(|p| o.starts_with(p))
    }) {
        return true;
    }
    let origin_ok = match origin {
        None => true,
        Some(o) => o.eq_ignore_ascii_case(&format!("https://{host}")),
    };
    origin_ok && !cross
}

/// A request from a paired device: its id.
pub struct Paired(String);

impl FromRequestParts<Arc<State>> for Paired {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, s: &Arc<State>) -> Result<Self, Response> {
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty() && t.len() <= 128);
        let device = token.and_then(|t| s.inner.devices.lock().unwrap().check(t));
        match device {
            Some(d) => Ok(Paired(d.id)),
            None => Err(error(StatusCode::UNAUTHORIZED, "not-paired")),
        }
    }
}

/// Hand `cmd` (marked remote) to the app and answer with what it says.
async fn ask(s: &State, mut cmd: mdm_ipc::Command) -> Response {
    cmd.remote = true;
    let sink = s.inner.sink.clone();
    let answer = tokio::task::spawn_blocking(move || sink.ask(cmd, ANSWER_WAIT))
        .await
        .ok()
        .flatten();
    match answer {
        Some(a) => json(a),
        None => error(StatusCode::SERVICE_UNAVAILABLE, "no-answer"),
    }
}

#[derive(Deserialize)]
struct LangQuery {
    #[serde(default)]
    lang: String,
}

async fn texts(s: S, Query(q): Query<LangQuery>) -> Response {
    let lang: String = q.lang.chars().take(200).collect();
    let texts = (s.inner.texts)(&lang);
    json(serde_json::to_string(&texts).unwrap_or_default())
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    #[serde(default)]
    name: String,
}

async fn pair(
    s: S,
    ConnectInfo(peer): ConnectInfo<Peer>,
    axum::Json(b): axum::Json<PairBody>,
) -> Response {
    let check = s.inner.pairing.lock().unwrap().check(&b.code, peer.0.ip());
    match check {
        Check::Right => {
            let (device, token) = s.inner.devices.lock().unwrap().add(&b.name);
            s.inner.pairing.lock().unwrap().done(&device.name);
            tracing::info!("remote control: {} paired ({})", device.name, peer.0.ip());
            // The app shows it (Options' list, the pairing window).
            s.inner.sink.wake();
            json(
                serde_json::json!({ "token": token, "id": device.id, "name": device.name })
                    .to_string(),
            )
        }
        Check::Wrong { left } => {
            tracing::info!("remote control: a wrong pairing code ({})", peer.0.ip());
            let body = serde_json::json!({
                "error": "wrong code", "code": "wrong-code", "left": left
            });
            (
                StatusCode::FORBIDDEN,
                [(header::CONTENT_TYPE, "application/json")],
                body.to_string(),
            )
                .into_response()
        }
        Check::NoCode => error(StatusCode::GONE, "no-code"),
        Check::TooSoon => error(StatusCode::TOO_MANY_REQUESTS, "too-soon"),
    }
}

async fn list(s: S, _: Paired) -> Response {
    ask(
        &s,
        mdm_ipc::Command {
            query: Some("list".into()),
            ..Default::default()
        },
    )
    .await
}

#[derive(Deserialize)]
struct AddBody {
    url: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    queue: Option<String>,
    /// Start now (else it waits in `queue`, the main queue by default).
    #[serde(default)]
    start: bool,
}

async fn add(s: S, _: Paired, axum::Json(b): axum::Json<AddBody>) -> Response {
    let url = b.url.trim();
    if url.is_empty() || url.len() > MAX_URL {
        return error(StatusCode::BAD_REQUEST, "bad-address");
    }
    ask(
        &s,
        mdm_ipc::Command {
            url: Some(url.to_owned()),
            category: b.category.filter(|c| !c.is_empty()),
            queue: b.queue.filter(|q| !q.is_empty()),
            later: !b.start,
            silent: true,
            ..Default::default()
        },
    )
    .await
}

#[derive(Deserialize)]
struct TorrentQuery {
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    queue: Option<String>,
    #[serde(default)]
    start: bool,
}

async fn torrent(s: S, _: Paired, Query(q): Query<TorrentQuery>, body: Bytes) -> Response {
    // A .torrent file is a bencoded dictionary.
    if body.first() != Some(&b'd') {
        return error(StatusCode::BAD_REQUEST, "not-a-torrent");
    }
    ask(
        &s,
        mdm_ipc::Command {
            torrent_file: Some(body.to_vec()),
            category: q.category.filter(|c| !c.is_empty()),
            queue: q.queue.filter(|q| !q.is_empty()),
            later: !q.start,
            silent: true,
            ..Default::default()
        },
    )
    .await
}

#[derive(Deserialize)]
struct ActBody {
    action: String,
    ids: Vec<u64>,
}

async fn act(s: S, _: Paired, axum::Json(b): axum::Json<ActBody>) -> Response {
    if !matches!(b.action.as_str(), "pause" | "start" | "remove" | "delete") || b.ids.len() > 10_000
    {
        return error(StatusCode::BAD_REQUEST, "bad-action");
    }
    ask(
        &s,
        mdm_ipc::Command {
            action: Some(b.action),
            ids: b.ids,
            ..Default::default()
        },
    )
    .await
}

#[derive(Deserialize)]
struct QueueBody {
    name: String,
    start: bool,
}

async fn queue(s: S, _: Paired, axum::Json(b): axum::Json<QueueBody>) -> Response {
    let action = if b.start { "start_queue" } else { "stop_queue" };
    ask(
        &s,
        mdm_ipc::Command {
            action: Some(action.into()),
            queue: Some(b.name),
            ..Default::default()
        },
    )
    .await
}

/// The device unpairs itself (the page's Forget this device).
async fn forget(s: S, Paired(id): Paired) -> Response {
    s.inner.devices.lock().unwrap().remove(&id);
    s.inner.sink.wake();
    StatusCode::NO_CONTENT.into_response()
}
