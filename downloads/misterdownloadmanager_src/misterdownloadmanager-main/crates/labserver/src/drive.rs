//! A fake Google Drive for the engine's tests (#34). Google's host names are told apart by the
//! `Host` header (the tests resolve them to the lab server, `HOSTS`), and each answers as Google
//! does: the same statuses and redirects, pages with the structure Drive's have (the virus-scan
//! form, the `uc-error-*` paragraphs), and the lab's deterministic files (`content`), so a
//! download is checked byte for byte. The pages are written here, not copied from Google.
//!
//! An ID says what it is (`id`): `KIND-SIZE[-FLAG…]`, padded to Drive's 25 or more characters.
//! The kinds and flags are listed in the server's help (`server::HELP`).

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::{Body, HttpBody};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use bytes::Bytes;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::content;
use crate::faults::has_cookie;
use crate::server::{Shared, serve, unix_ms};

const DRIVE: &str = "drive.google.com";
const USERCONTENT: &str = "drive.usercontent.google.com";
const DOCS: &str = "docs.google.com";
const ACCOUNTS: &str = "accounts.google.com";
const EXPORTS: &str = "doc-0.docs.googleusercontent.com";

/// The host names the fake Drive answers on: resolve them to the lab server.
pub const HOSTS: [&str; 5] = [DRIVE, USERCONTENT, DOCS, ACCOUNTS, EXPORTS];

/// The signed-in cookie the fake Drive takes (`private`).
pub const SIGNED_IN: &str = "SID=lab";

/// The resource key the `rk` kind wants.
pub const RESOURCE_KEY: &str = "0-lab";

/// A fake Drive ID: `kind`, the file's `size` (`64K`, `8M`) and `flags`, padded to 28 characters.
pub fn id(kind: &str, size: &str, flags: &[&str]) -> String {
    let mut id = format!("{kind}-{size}");
    for f in flags {
        id.push('-');
        id.push_str(f);
    }
    if id.len() < 28 {
        id.push('-');
        while id.len() < 28 {
            id.push('0');
        }
    }
    id
}

/// The content seed of the file `id` (`content::fill`).
pub fn seed(id: &str) -> u64 {
    // FNV-1a.
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The name the file `id` is served under (Content-Disposition).
pub fn file_name(id: &str) -> String {
    let kind = Lab::parse(id).map_or_else(String::new, |l| l.kind);
    match kind.as_str() {
        "uni" => "naïve résumé.bin".to_owned(),
        _ => format!("{kind}.bin"),
    }
}

/// The name a Google Doc's export comes under: `doc_name.<format>`.
pub fn doc_name(id: &str) -> String {
    format!(
        "Lab {}",
        Lab::parse(id).map_or_else(String::new, |l| l.kind)
    )
}

/// A parsed fake ID.
struct Lab {
    kind: String,
    size: String,
    flags: Vec<String>,
}

impl Lab {
    fn parse(id: &str) -> Option<Lab> {
        let mut parts = id.split('-');
        let kind = parts.next()?.to_owned();
        let size = parts.next()?.to_owned();
        content::parse_size(&size)?;
        let flags = parts
            .filter(|f| !f.bytes().all(|b| b == b'0'))
            .map(str::to_owned)
            .collect();
        Some(Lab { kind, size, flags })
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    /// `ttlN`: N ms.
    fn ttl(&self) -> Option<u64> {
        self.flags
            .iter()
            .find_map(|f| f.strip_prefix("ttl")?.parse().ok())
    }
}

/// Every address the lab's own routes don't take: Google's, by the `Host` header; else 404.
pub(crate) async fn handle(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let host_header = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (host, port) = match host_header.rsplit_once(':') {
        Some((h, p)) => (h.to_owned(), p.to_owned()),
        None => (host_header.clone(), "80".to_owned()),
    };
    // Not Google: an address the lab doesn't have, as before the fake Drive (not logged).
    if !HOSTS.contains(&host.as_str()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = uri.path().to_owned();
    let raw_query = uri.query().map(str::to_owned);
    let entry = shared.begin(
        peer,
        &method,
        path.clone(),
        raw_query.clone(),
        &headers,
        &[],
    );
    let q: HashMap<String, String> = raw_query
        .as_deref()
        .and_then(|q| serde_urlencoded::from_str(q).ok())
        .unwrap_or_default();
    let ask = Ask {
        port,
        query: raw_query.unwrap_or_default(),
        q,
        cookie: entry.cookie.clone(),
    };
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let answer = match (host.as_str(), segs.as_slice()) {
        (DRIVE, ["file", "d", id, ..] | ["file", "u", _, "d", id, ..]) => viewer(id),
        (DOCS, ["file", "d", id, ..]) => viewer(id),
        (DRIVE | DOCS | USERCONTENT, ["uc"]) => redirect(
            StatusCode::SEE_OTHER,
            ask.at(USERCONTENT, &format!("/download?{}", ask.query)),
        ),
        (DRIVE, ["open"]) => open(&ask),
        (USERCONTENT, ["download"]) => {
            return download(shared, entry, &method, &ask).await;
        }
        (DOCS, [_, "d", id, "edit"]) => Answer::Page(
            StatusCode::OK,
            format!(
                "<!doctype html><title>{} - Google Docs</title><p>The editor.</p>",
                doc_name(id)
            ),
        ),
        (DOCS, [kind, "d", id, "export"]) => redirect(
            StatusCode::TEMPORARY_REDIRECT,
            ask.at(
                EXPORTS,
                &format!(
                    "/export/tok/{id}?format={}&kind={kind}",
                    ask.q.get("format").map_or("pdf", String::as_str)
                ),
            ),
        ),
        (EXPORTS, ["export", "tok", id]) => {
            return export(&shared, entry, id, &ask);
        }
        (ACCOUNTS, ["ServiceLogin"]) => Answer::Page(
            StatusCode::OK,
            "<!doctype html><title>Sign in - Google Accounts</title><form><p>Sign in</p>\
             <input type=email name=identifier></form>"
                .to_owned(),
        ),
        _ => Answer::Page(StatusCode::NOT_FOUND, not_found()),
    };
    answer.send(&shared, entry)
}

/// What the request asked, for the handlers.
struct Ask {
    port: String,
    /// The query as sent.
    query: String,
    q: HashMap<String, String>,
    cookie: Option<String>,
}

impl Ask {
    /// An absolute address on `host` (this server, on the port asked).
    fn at(&self, host: &str, path_and_query: &str) -> String {
        format!("http://{host}:{}{path_and_query}", self.port)
    }

    fn signed_in(&self) -> bool {
        has_cookie(self.cookie.as_deref(), SIGNED_IN)
    }
}

/// A page or a redirect, logged as it is sent.
enum Answer {
    Page(StatusCode, String),
    Redirect(StatusCode, String),
}

impl Answer {
    fn send(self, shared: &Shared, mut entry: crate::server::LogEntry) -> Response {
        let (status, resp, outcome) = match self {
            Answer::Page(status, html) => (status, (status, Html(html)).into_response(), "page"),
            Answer::Redirect(status, to) => (
                status,
                (status, [(header::LOCATION, to)]).into_response(),
                "redirect",
            ),
        };
        let len = resp.body().size_hint().exact().unwrap_or(0);
        entry.body_len = len;
        entry.bytes_sent = len;
        shared.record(entry, status.as_u16(), outcome);
        resp
    }
}

fn redirect(status: StatusCode, to: String) -> Answer {
    Answer::Redirect(status, to)
}

fn not_found() -> String {
    "<!doctype html><title>Google Drive - Page Not Found</title><p>Sorry, the file you have \
     requested does not exist.</p>"
        .to_owned()
}

/// The viewer: a large page without a form (the engine must never need it).
fn viewer(id: &str) -> Answer {
    let name = file_name(id);
    Answer::Page(
        StatusCode::OK,
        format!(
            "<!doctype html><html><head><title>{name} - Google Drive</title>\
             <meta property=\"og:title\" content=\"{name}\"></head><body><div id=viewer></div>\
             <!-- {} --></body></html>",
            "x".repeat(150 << 10)
        ),
    )
}

/// `open?id=`: a file's viewer, or a Google Doc's editor.
fn open(ask: &Ask) -> Answer {
    let Some(id) = ask.q.get("id") else {
        return Answer::Page(StatusCode::NOT_FOUND, not_found());
    };
    let key = ask
        .q
        .get("resourcekey")
        .map_or_else(String::new, |k| format!("&resourcekey={k}"));
    let to = if id.starts_with("doc-") {
        ask.at(DOCS, &format!("/document/d/{id}/edit?usp=drive_open{key}"))
    } else {
        ask.at(DRIVE, &format!("/file/d/{id}/view?usp=drive_open{key}"))
    };
    redirect(StatusCode::TEMPORARY_REDIRECT, to)
}

/// The file host's `/download?id=…`: the file, the virus-scan page, or Drive's refusals.
async fn download(
    shared: Arc<Shared>,
    entry: crate::server::LogEntry,
    method: &Method,
    ask: &Ask,
) -> Response {
    let id = ask.q.get("id").cloned().unwrap_or_default();
    let Some(lab) = Lab::parse(&id) else {
        return Answer::Page(StatusCode::NOT_FOUND, not_found()).send(&shared, entry);
    };
    let answer = match lab.kind.as_str() {
        "small" | "uni" => None,
        "rk" if ask.q.get("resourcekey").map(String::as_str) == Some(RESOURCE_KEY) => None,
        "rk" | "gone" => Some(Answer::Page(StatusCode::NOT_FOUND, not_found())),
        "quota" | "doc" => Some(Answer::Page(StatusCode::OK, quota_page())),
        "noaccess" => Some(Answer::Page(
            StatusCode::FORBIDDEN,
            "<!doctype html><title>Google Drive - You need access</title><p>You need access</p>"
                .to_owned(),
        )),
        "private" if !ask.signed_in() => {
            let back = ask.at(USERCONTENT, &format!("/download?{}", ask.query));
            Some(redirect(
                StatusCode::FOUND,
                ask.at(
                    ACCOUNTS,
                    &format!(
                        "/ServiceLogin?continue={}",
                        utf8_percent_encode(&back, NON_ALPHANUMERIC)
                    ),
                ),
            ))
        }
        "big" | "private" => {
            let uuid_ok = !lab.flag("uuid")
                || ask
                    .q
                    .get("uuid")
                    .is_some_and(|u| shared.drive_uuids.lock().unwrap().contains(u));
            (!(ask.q.contains_key("confirm") && uuid_ok))
                .then(|| Answer::Page(StatusCode::OK, warning_page(&shared, ask, &id, &lab)))
        }
        _ => Some(Answer::Page(StatusCode::NOT_FOUND, not_found())),
    };
    if let Some(answer) = answer {
        return answer.send(&shared, entry);
    }
    // The file: no ETag (Drive sends none), its name, a link that may expire.
    let name = file_name(&id);
    let mut query = format!("seed={}&etag=none", seed(&id));
    let name_param = if lab.kind == "uni" { "cdstar" } else { "cd" };
    query.push_str(&format!(
        "&{name_param}={}",
        utf8_percent_encode(&name, NON_ALPHANUMERIC)
    ));
    if let Some(expires) = ask.q.get("expires") {
        query.push_str(&format!("&expires={expires}"));
    }
    serve(shared, entry, method, &lab.size, None, &query).await
}

/// The virus-scan warning ("can't scan this file for viruses"), with a fresh `uuid` in its form.
fn warning_page(shared: &Shared, ask: &Ask, id: &str, lab: &Lab) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let uuid = format!(
        "{:08x}-0000-4000-8000-{:012x}",
        unix_ms() as u32,
        NEXT.fetch_add(1, Ordering::SeqCst)
    );
    shared.drive_uuids.lock().unwrap().insert(uuid.clone());
    let mut inputs = vec![
        ("id", id.to_owned()),
        ("export", "download".to_owned()),
        ("confirm", "t".to_owned()),
        ("uuid", uuid),
    ];
    if ask.signed_in() {
        inputs.push(("at", "lab:token&1".to_owned()));
    }
    if let Some(ttl) = lab.ttl() {
        inputs.push(("expires", (unix_ms() + ttl).to_string()));
    }
    let inputs: String = inputs
        .iter()
        .map(|(n, v)| {
            format!(
                "<input type=\"hidden\" name=\"{n}\" value=\"{}\">",
                v.replace('&', "&amp;")
            )
        })
        .collect();
    format!(
        "<!DOCTYPE html><html><head><title>Google Drive - Virus scan warning</title></head>\
         <body><div class=\"uc-main\"><p class=\"uc-warning-caption\">Google Drive can&#39;t scan \
         this file for viruses.</p><p class=\"uc-warning-subcaption\"><span class=\"uc-name-size\">\
         <a href=\"/open?id={id}\">{name}</a> ({size})</span> is too large to scan.</p>\
         <form id=\"download-form\" action=\"{action}\" method=\"get\"><input type=\"submit\" \
         id=\"uc-download-link\" value=\"Download anyway\"/>{inputs}</form></div></body></html>",
        name = file_name(id),
        size = lab.size,
        action = ask.at(USERCONTENT, "/download"),
    )
}

/// "Quota exceeded": Drive's refusal page, a 200.
fn quota_page() -> String {
    "<!DOCTYPE html><html><head><title>Google Drive - Quota exceeded</title></head><body>\
     <div class=\"uc-main\"><p class=\"uc-error-caption\">Sorry, you can&#39;t view or download \
     this file at this time.</p><p class=\"uc-error-subcaption\">Too many users have viewed or \
     downloaded this file recently. Please try accessing the file again later.</p></div>\
     </body></html>"
        .to_owned()
}

/// A Google Doc's export: made on the fly, so chunked, without a length, ranges ignored.
fn export(shared: &Shared, mut entry: crate::server::LogEntry, id: &str, ask: &Ask) -> Response {
    let Some(lab) = Lab::parse(id) else {
        return Answer::Page(StatusCode::NOT_FOUND, not_found()).send(shared, entry);
    };
    let format = ask.q.get("format").cloned().unwrap_or_else(|| "pdf".into());
    let size = content::parse_size(&lab.size).unwrap_or(0) as usize;
    let mut data = vec![0u8; size];
    content::fill(seed(id), 0, &mut data);
    let chunks: Vec<Result<Bytes, Infallible>> = data
        .chunks(16 << 10)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    let mime = match format.as_str() {
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    };
    let name = format!("{}.{format}", doc_name(id));
    let disposition = format!(
        "attachment; filename=\"{name}\"; filename*=UTF-8''{}",
        utf8_percent_encode(&name, NON_ALPHANUMERIC)
    );
    entry.body_len = size as u64;
    entry.bytes_sent = size as u64;
    shared.record(entry, 200, "complete");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from_stream(futures::stream::iter(chunks)))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_say_what_they_are() {
        let id = id("big", "8M", &["uuid", "ttl500"]);
        assert!(id.len() >= 28, "{id}");
        let lab = Lab::parse(&id).unwrap();
        assert_eq!((lab.kind.as_str(), lab.size.as_str()), ("big", "8M"));
        assert!(lab.flag("uuid") && !lab.flag("0000"));
        assert_eq!(lab.ttl(), Some(500));
        assert_eq!(file_name(&id), "big.bin");
        assert_ne!(seed(&id), seed(&super::id("big", "8M", &[])));
    }
}
