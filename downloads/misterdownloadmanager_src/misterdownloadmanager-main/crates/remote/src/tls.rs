//! The remote control's certificate and its HTTPS listener. A self-signed certificate is made
//! once (`remote/cert.der`, its key `remote/key.der`, readable by the user only) and kept: its
//! fingerprint, shown in Options, is how a device can tell it talks to this MDM the first time
//! its browser warns about it, and the same certificate every time is what lets the browser
//! remember that choice. It is made again only when it runs out (800 days, under the limit
//! Apple's systems accept) or is deleted.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// How long the certificate lasts, and how long before its end a new one is made.
const VALID_DAYS: i64 = 800;
const RENEW_DAYS: i64 = 30;
/// Connections open at once on one listener; more wait to be accepted.
const MAX_CONNECTIONS: usize = 64;
/// Time a client gets for the TLS handshake (or a plain HTTP request's head).
const HANDSHAKE: Duration = Duration::from_secs(10);
/// A connection that sends and receives nothing this long is closed, so connections held open
/// doing nothing can't take every place (`MAX_CONNECTIONS`) from the devices using the page.
const IDLE: Duration = Duration::from_secs(60);

/// The certificate in use: the server's TLS settings and the fingerprint people compare.
#[derive(Clone)]
pub struct Identity {
    pub config: Arc<rustls::ServerConfig>,
    /// SHA-256 of the certificate, as `AB:CD:…` (what browsers show).
    pub fingerprint: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Made {
    /// The last day it is valid, in days since 1970-01-01.
    until: i64,
}

/// Today, in days since 1970-01-01 (UTC).
fn today() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    secs.div_euclid(86_400)
}

/// The civil date (year, month, day) of `days` since 1970-01-01 (H. Hinnant's algorithm).
fn civil(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// The certificate kept in `dir`, or a new one for `names` (the computer's name, `localhost`
/// and its addresses) when there is none or it runs out soon.
pub fn identity(dir: &Path, names: &[String]) -> Result<Identity, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (cert_path, key_path, made_path) = (
        dir.join("cert.der"),
        dir.join("key.der"),
        dir.join("cert.json"),
    );
    let kept = (|| {
        let made: Made = serde_json::from_slice(&std::fs::read(&made_path).ok()?).ok()?;
        (made.until - today() > RENEW_DAYS).then_some(())?;
        Some((
            std::fs::read(&cert_path).ok()?,
            std::fs::read(&key_path).ok()?,
        ))
    })();
    let (cert, key) = match kept {
        Some(pair) => pair,
        None => {
            let (cert, key) = make(names)?;
            mdm_ipc::private_write(&key_path, &key).map_err(|e| e.to_string())?;
            mdm_ipc::private_write(&cert_path, &cert).map_err(|e| e.to_string())?;
            let made = serde_json::to_vec(&Made {
                until: today() + VALID_DAYS,
            })
            .map_err(|e| e.to_string())?;
            mdm_ipc::private_write(&made_path, &made).map_err(|e| e.to_string())?;
            (cert, key)
        }
    };
    let fingerprint = {
        use sha2::Digest;
        sha2::Sha256::digest(&cert)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":")
    };
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.into()],
            rustls::pki_types::PrivateKeyDer::Pkcs8(key.into()),
        )
        .map_err(|e| e.to_string())?;
    Ok(Identity {
        config: Arc::new(config),
        fingerprint,
    })
}

/// A new self-signed certificate for `names`: (certificate, key), both DER.
fn make(names: &[String]) -> Result<(Vec<u8>, Vec<u8>), String> {
    use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair};
    let e = |e: rcgen::Error| e.to_string();
    let mut params = CertificateParams::new(names.to_vec()).map_err(e)?;
    params
        .distinguished_name
        .push(DnType::CommonName, "Mister Download Manager");
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let (y, m, d) = civil(today() - 1);
    params.not_before = rcgen::date_time_ymd(y, m, d);
    let (y, m, d) = civil(today() + VALID_DAYS);
    params.not_after = rcgen::date_time_ymd(y, m, d);
    let key = KeyPair::generate().map_err(e)?;
    let cert = params.self_signed(&key).map_err(e)?;
    Ok((cert.der().to_vec(), key.serialize_der()))
}

/// One accepted connection: TLS, closed once idle for [`IDLE`], and its place among the
/// listener's `MAX_CONNECTIONS`.
pub struct Conn {
    io: Idle<TlsStream<TcpStream>>,
    _slot: OwnedSemaphorePermit,
}

impl AsyncRead for Conn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Conn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// `io`, which fails with `TimedOut` once nothing was read or written for `limit` while the
/// server waits on it (hyper then closes the connection).
pub struct Idle<T> {
    io: T,
    limit: Duration,
    timer: Pin<Box<tokio::time::Sleep>>,
}

impl<T> Idle<T> {
    pub fn new(io: T, limit: Duration) -> Idle<T> {
        Idle {
            io,
            limit,
            timer: Box::pin(tokio::time::sleep(limit)),
        }
    }

    fn active(&mut self) {
        let next = tokio::time::Instant::now() + self.limit;
        self.timer.as_mut().reset(next);
    }

    /// Waiting on the other end: its time is up?
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.timer.as_mut().poll(cx).is_ready()
    }
}

fn idle_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the connection was idle too long")
}

impl<T: AsyncRead + Unpin> AsyncRead for Idle<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        match Pin::new(&mut this.io).poll_read(cx, buf) {
            Poll::Ready(r) => {
                this.active();
                Poll::Ready(r)
            }
            Poll::Pending if this.expired(cx) => Poll::Ready(Err(idle_error())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Idle<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        match Pin::new(&mut this.io).poll_write(cx, buf) {
            Poll::Ready(r) => {
                this.active();
                Poll::Ready(r)
            }
            Poll::Pending if this.expired(cx) => Poll::Ready(Err(idle_error())),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

/// The HTTPS listener axum serves from: connections are accepted and their handshakes done on
/// tasks of their own (a slow client holds up no other), at most `MAX_CONNECTIONS` at once. A
/// plain `http://` request is answered with the `https://` address (`redirect`).
pub struct Listener {
    rx: mpsc::Receiver<(Conn, SocketAddr)>,
    local: SocketAddr,
}

impl Listener {
    /// Serve `tcp` with `tls`; `host_ok` says which `Host` names a plain request is sent on to.
    pub fn new(
        tcp: TcpListener,
        tls: Arc<rustls::ServerConfig>,
        host_ok: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> io::Result<Listener> {
        let local = tcp.local_addr()?;
        let (tx, rx) = mpsc::channel(16);
        let acceptor = TlsAcceptor::from(tls);
        let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
        tokio::spawn(async move {
            // Until the server stops (its end of the channel dropped): the port is let go then.
            loop {
                let slot = tokio::select! {
                    _ = tx.closed() => return,
                    s = slots.clone().acquire_owned() => match s {
                        Ok(s) => s,
                        Err(_) => return,
                    },
                };
                let accepted = tokio::select! {
                    _ = tx.closed() => return,
                    a = tcp.accept() => a,
                };
                let (tcp, peer) = match accepted {
                    Ok(c) => c,
                    Err(_) => {
                        // Out of file handles and the like: try again in a moment.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let (tx, acceptor, host_ok) = (tx.clone(), acceptor.clone(), host_ok.clone());
                tokio::spawn(async move {
                    let _ = tcp.set_nodelay(true);
                    // TLS starts with a handshake record (22); anything else is plain HTTP.
                    let mut first = [0u8; 1];
                    match tokio::time::timeout(HANDSHAKE, tcp.peek(&mut first)).await {
                        Ok(Ok(1)) if first[0] == 22 => {}
                        Ok(Ok(1)) => {
                            redirect(tcp, local.port(), &*host_ok).await;
                            return;
                        }
                        _ => return,
                    }
                    if let Ok(Ok(io)) = tokio::time::timeout(HANDSHAKE, acceptor.accept(tcp)).await
                    {
                        let io = Idle::new(io, IDLE);
                        let _ = tx.send((Conn { io, _slot: slot }, peer)).await;
                    }
                });
            }
        });
        Ok(Listener { rx, local })
    }
}

impl axum::serve::Listener for Listener {
    type Io = Conn;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            // The accepting task ended: nothing more comes.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// A plain `http://` request: answered with a redirect to the same address over HTTPS (to a
/// name this server answers to only), then closed.
async fn redirect(mut tcp: TcpStream, port: u16, host_ok: &(dyn Fn(&str) -> bool + Send + Sync)) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    let read = tokio::time::timeout(HANDSHAKE, async {
        while head.len() < 8192 && !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match tcp.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => head.extend_from_slice(&chunk[..n]),
            }
        }
    })
    .await;
    if read.is_err() {
        return;
    }
    let text = String::from_utf8_lossy(&head);
    let host = text.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("host")
            .then(|| value.trim().to_owned())
    });
    let answer = match host {
        Some(h) if host_ok(crate::addrs::host_of(&h)) => {
            let name = crate::addrs::host_of(&h);
            format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: https://{name}:{port}/\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            )
        }
        _ => "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
    };
    let _ = tcp.write_all(answer.as_bytes()).await;
    let _ = tcp.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(20_398), (2025, 11, 6));
    }

    /// A connection quiet for the limit fails its read (the server closes it); one that keeps
    /// talking doesn't.
    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_is_closed() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let limit = Duration::from_secs(60);
        let (server, mut client) = tokio::io::duplex(64);
        let mut server = Idle::new(server, limit);
        let mut buf = [0u8; 8];
        // Talking every 50 s: never idle for the limit.
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(50)).await;
            client.write_all(b"x").await.unwrap();
            assert_eq!(server.read(&mut buf).await.unwrap(), 1);
        }
        // Then nothing: the read fails once the limit has passed.
        let err = server.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn kept_until_it_runs_out() {
        let dir = tempfile::tempdir().unwrap();
        let names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
        let a = identity(dir.path(), &names).unwrap();
        let b = identity(dir.path(), &names).unwrap();
        assert_eq!(
            a.fingerprint, b.fingerprint,
            "the same certificate each start"
        );
        assert_eq!(a.fingerprint.len(), 32 * 3 - 1);
        // Near its end: a new one.
        let made = serde_json::to_vec(&Made { until: today() + 5 }).unwrap();
        std::fs::write(dir.path().join("cert.json"), made).unwrap();
        let c = identity(dir.path(), &names).unwrap();
        assert_ne!(a.fingerprint, c.fingerprint);
    }
}
