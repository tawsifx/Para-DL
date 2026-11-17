//! The helpers through the chosen network adapter (#35). yt-dlp, gallery-dl and the ffmpeg that
//! yt-dlp starts can't be held to an adapter themselves: yt-dlp's `--source-address` was seen not
//! to hold on Windows (#108: given an address without a route, it still connected from the
//! Ethernet adapter's), an address alone doesn't hold on Linux (it routes by destination, the weak
//! host model), and none of them can bind a device or an interface. So while an adapter is chosen
//! they are given this proxy (`--proxy`, and `HTTP_PROXY`… for what they start), and it opens
//! their connections through [`mdm_engine::net`], bound as MDM's own are, with the chosen DNS. The
//! same code holds them on Windows, Linux and macOS, IPv4 and IPv6: the binding is the engine's.
//!
//! - It listens on 127.0.0.1 only, on a port the system picks (no firewall asks about a socket
//!   that only this computer can reach), and wants a password of its own (Basic proxy
//!   authentication), so no other program uses it.
//! - It takes `CONNECT host:port` (HTTPS and anything else tunnelled) and plain `http://`
//!   requests (absolute form, sent on with `Connection: close`). Nothing else.
//! - It fails closed: while the adapter is down, or a host has no address of a family the adapter
//!   has, the helper is answered `502` with the reason and nothing is sent. When the route
//!   changes, every open connection is closed at once ([`Relay::route_changed`]); the app stops
//!   the downloads too, and starts them again through the new route.

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use mdm_engine::{DnsChoice, Route};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// A request's or a response's head may be at most this long.
const HEAD_MAX: usize = 64 * 1024;
/// A helper that connected has this long to send its request.
const HEAD_TIME: Duration = Duration::from_secs(30);
/// How long each of a server's addresses may take to connect.
const CONNECT_TIME: Duration = Duration::from_secs(10);
/// The user name in the relay's address (any is taken: the password is what counts).
const USER: &str = "mdm";

/// Where a relay's connections go, read for each one: the network adapter's route and the DNS.
pub type Net = Arc<dyn Fn() -> (Route, DnsChoice) + Send + Sync>;

/// A running relay; it stops when dropped.
pub struct Relay {
    addr: SocketAddr,
    secret: String,
    changed: watch::Sender<u64>,
    _rt: tokio::runtime::Runtime,
}

struct Inner {
    secret: String,
    net: Net,
    changed: watch::Receiver<u64>,
}

impl Relay {
    /// A relay on 127.0.0.1 whose connections go through `net`, on a runtime of its own (the
    /// helpers are started from threads without one).
    pub fn start(net: Net) -> io::Result<Relay> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let mut bytes = [0u8; 18];
        aws_lc_rs::rand::fill(&mut bytes).map_err(|_| io::Error::other("no random numbers"))?;
        let secret = hex::encode(bytes);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mdm-relay")
            .enable_all()
            .build()?;
        let listener = {
            let _in = rt.enter();
            TcpListener::from_std(listener)?
        };
        let (changed, rx) = watch::channel(0);
        let inner = Arc::new(Inner {
            secret: secret.clone(),
            net,
            changed: rx,
        });
        rt.spawn(accept(listener, inner));
        Ok(Relay {
            addr,
            secret,
            changed,
            _rt: rt,
        })
    }

    /// The proxy address to give a helper, its password in it.
    pub fn url(&self) -> String {
        format!("http://{USER}:{}@{}", self.secret, self.addr)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The route changed: every open connection closes now (it was opened through the old one).
    pub fn route_changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }
}

async fn accept(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, inner).await;
                });
            }
            Ok(_) => {}
            // Out of sockets for a moment: try again shortly.
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// One helper's connection: its request, then the server's connection through the route, then
/// the bytes both ways until either side ends or the route changes.
async fn serve(mut client: TcpStream, inner: Arc<Inner>) -> io::Result<()> {
    let _ = client.set_nodelay(true);
    let mut changed = inner.changed.clone();
    changed.mark_unchanged();
    let mut buf = Vec::new();
    let head = match tokio::time::timeout(HEAD_TIME, read_head(&mut client, &mut buf)).await {
        Ok(Ok(Some(head))) => head,
        _ => return Ok(()),
    };
    let Some(req) = Request::parse(&head) else {
        return reply(&mut client, 400, "Bad Request").await;
    };
    if !req.authorized(&inner.secret) {
        return client
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: Basic realm=\"MDM\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
    }
    let Some(target) = req.target() else {
        return reply(&mut client, 400, "Bad Request").await;
    };
    let (route, dns) = (inner.net)();
    let mut server = match open(&route, &dns, &target.host, target.port).await {
        Ok(s) => s,
        Err(why) => return reply(&mut client, 502, &why).await,
    };
    let _ = server.set_nodelay(true);
    let forward = match &target.forward {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
            false
        }
        Some(head) => {
            server.write_all(head).await?;
            true
        }
    };
    // What the helper sent after its head (the start of a body, or of TLS).
    server.write_all(&buf).await?;
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    let up = async move {
        let _ = tokio::io::copy(&mut cr, &mut sw).await;
        let _ = sw.shutdown().await;
        // The helper is done sending; the answer may still be on its way.
        std::future::pending::<()>().await
    };
    let down = async move {
        if forward {
            pass_response_head(&mut sr, &mut cw).await?;
        }
        tokio::io::copy(&mut sr, &mut cw).await?;
        cw.shutdown().await
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
        // The route changed (or the relay stopped): this connection was held to the old one.
        _ = changed.changed() => {}
    }
    Ok(())
}

/// A connection to `host:port` through `route`, its name looked up with `dns` (as downloads'
/// are); else why not, for the helper's error.
async fn open(route: &Route, dns: &DnsChoice, host: &str, port: u16) -> Result<TcpStream, String> {
    let ips = mdm_engine::dns::lookup(dns, host, port, &[], route)
        .await
        .map_err(|e| e.to_string())?;
    let mut why = format!("{host} has no address");
    for ip in ips {
        let addr = SocketAddr::new(ip, port);
        match tokio::time::timeout(CONNECT_TIME, mdm_engine::net::tcp_connect(route, addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => why = e.to_string(),
            Err(_) => why = format!("{addr} didn't answer"),
        }
    }
    Err(why)
}

/// An answer of the relay's own, then the connection closes. `why` becomes the reason phrase,
/// which is what yt-dlp shows ("Tunnel connection failed: 502 the network adapter … is not
/// connected").
async fn reply(client: &mut TcpStream, code: u16, why: &str) -> io::Result<()> {
    let why: String = why
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    let text = format!(
        "HTTP/1.1 {code} {why}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{why}",
        why.len()
    );
    client.write_all(text.as_bytes()).await?;
    client.shutdown().await
}

/// The next head (up to and with its blank line) from `r`; what came after it stays in `buf`.
/// None: the stream ended first.
async fn read_head<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> io::Result<Option<Vec<u8>>> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok(Some(std::mem::replace(buf, rest)));
        }
        if buf.len() > HEAD_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// A plain request's answer: its head goes to the helper with `Connection: close` (so the helper
/// doesn't send its next request, maybe for another server, down this connection), interim
/// answers (`100 Continue`) as they are, then the rest as it comes.
async fn pass_response_head<R, W>(sr: &mut R, cw: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    loop {
        let Some(head) = read_head(sr, &mut buf).await? else {
            break;
        };
        let code = std::str::from_utf8(&head)
            .ok()
            .and_then(|h| h.split(' ').nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        if (100..200).contains(&code) && code != 101 {
            cw.write_all(&head).await?;
            continue;
        }
        if code == 101 {
            cw.write_all(&head).await?;
        } else {
            cw.write_all(&closing(&head)).await?;
        }
        break;
    }
    cw.write_all(&buf).await
}

/// `head` without its connection headers, ending with `Connection: close`.
fn closing(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.trim_end_matches("\r\n").split("\r\n");
    let mut out = String::new();
    out.push_str(lines.next().unwrap_or_default());
    out.push_str("\r\n");
    for line in lines {
        let name = line.split(':').next().unwrap_or_default().trim();
        if ["connection", "keep-alive", "proxy-connection"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
        {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

/// A helper's request head.
struct Request {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
}

/// Where a request goes: a tunnel (`forward` None) or a plain request sent on with this head.
struct Target {
    host: String,
    port: u16,
    forward: Option<Vec<u8>>,
}

impl Request {
    fn parse(head: &[u8]) -> Option<Request> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.trim_end_matches("\r\n").split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target, version) = (first.next()?, first.next()?, first.next()?);
        if first.next().is_some() || !version.starts_with("HTTP/1.") {
            return None;
        }
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
        Some(Request {
            method: method.to_owned(),
            target: target.to_owned(),
            version: version.to_owned(),
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Whether it carries the relay's password (any user name).
    fn authorized(&self, secret: &str) -> bool {
        use base64::Engine;
        let Some(given) = self
            .header("proxy-authorization")
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
            .and_then(|(_, b64)| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        else {
            return false;
        };
        let Some(at) = given.iter().position(|b| *b == b':') else {
            return false;
        };
        let pass = &given[at + 1..];
        // The same time for every wrong password of the right length.
        pass.len() == secret.len()
            && pass
                .iter()
                .zip(secret.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    /// Where it goes: `CONNECT host:port`, or an absolute `http://` address. Nothing else.
    fn target(&self) -> Option<Target> {
        if self.method.eq_ignore_ascii_case("CONNECT") {
            let (host, port) = split_host_port(&self.target)?;
            return Some(Target {
                host,
                port,
                forward: None,
            });
        }
        let url = url::Url::parse(&self.target).ok()?;
        if url.scheme() != "http" {
            return None;
        }
        let host = url.host_str()?.trim_matches(['[', ']']).to_owned();
        let port = url.port_or_known_default()?;
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        let mut head = format!("{} {path} {}\r\n", self.method, self.version);
        let mut has_host = false;
        for (name, value) in &self.headers {
            if [
                "proxy-authorization",
                "proxy-connection",
                "connection",
                "keep-alive",
            ]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
            {
                continue;
            }
            has_host |= name.eq_ignore_ascii_case("host");
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_host {
            let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
            head.push_str(&format!("Host: {authority}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        Some(Target {
            host,
            port,
            forward: Some(head.into_bytes()),
        })
    }
}

/// `host:port` or `[v6]:port`; the port is required.
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() || host.contains(['/', ' ', '@']) {
        return None;
    }
    Some((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_and_rewritten() {
        let r = Request::parse(
            b"GET http://files.test:8080/a/b?c=1 HTTP/1.1\r\nHost: files.test:8080\r\n\
              Proxy-Authorization: Basic bWRtOnNlY3JldA==\r\nProxy-Connection: keep-alive\r\n\
              Connection: keep-alive\r\nRange: bytes=0-\r\n\r\n",
        )
        .unwrap();
        assert!(r.authorized("secret"));
        assert!(!r.authorized("secreT"));
        assert!(!r.authorized("secre"));
        let t = r.target().unwrap();
        assert_eq!((t.host.as_str(), t.port), ("files.test", 8080));
        let head = String::from_utf8(t.forward.unwrap()).unwrap();
        assert_eq!(
            head,
            "GET /a/b?c=1 HTTP/1.1\r\nHost: files.test:8080\r\nRange: bytes=0-\r\n\
             Connection: close\r\n\r\n",
            "the password never goes on"
        );
        let c = Request::parse(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\n\r\n").unwrap();
        let t = c.target().unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.forward),
            ("2001:db8::1", 443, None)
        );
        assert!(!c.authorized("secret"), "no password, no relay");
        // Only CONNECT and absolute http:// addresses.
        for bad in [
            &b"GET /index.html HTTP/1.1\r\n\r\n"[..],
            b"GET https://x.test/ HTTP/1.1\r\n\r\n",
            b"GET ftp://x.test/f HTTP/1.1\r\n\r\n",
            b"CONNECT x.test HTTP/1.1\r\n\r\n",
            b"CONNECT x.test:0 HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                Request::parse(bad).and_then(|r| r.target()).is_none(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(Request::parse(b"GET / HTTP/2\r\n\r\n").is_none());
    }

    #[test]
    fn answers_say_connection_close() {
        let h = closing(b"HTTP/1.1 200 OK\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nContent-Length: 3\r\n\r\n");
        assert_eq!(
            String::from_utf8(h).unwrap(),
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n"
        );
    }
}
