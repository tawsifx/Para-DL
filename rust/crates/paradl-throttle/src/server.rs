//! VPS ground-truth test server used by module M9.
//!
//! The pure [`build_payload_block`] helper is always available. The Tokio TCP
//! and UDP server is compiled only when the `server` feature is enabled,
//! keeping the default build free of optional networking runtimes.

const MP4_FTYP_HEADER: &[u8] =
    b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom\x00\x01\x00\x00mdat";
const ZIP_LOCAL_HEADER: &[u8] = b"PK\x03\x04\x14\x00\x00\x00\x00\x00";

/// Return a deterministic payload block and its content type.
///
/// `random` uses a byte generator of `(i * 73 + 29) & 0xFF`; `video` prefixes a
/// minimal MP4 `ftyp`/`mdat` header; `file` prefixes a ZIP local-file header.
/// Unknown kinds fall back to `random`, matching the Python server.
///
/// ```
/// use paradl_throttle::server::build_payload_block;
/// let (block, ctype) = build_payload_block("video", 65_536);
/// assert_eq!(ctype, "video/mp4");
/// assert_eq!(block.len(), 65_536);
/// assert!(block.starts_with(&[0, 0, 0, 0x18]));
/// ```
pub fn build_payload_block(payload_type: &str, size: usize) -> (Vec<u8>, &'static str) {
    let base: Vec<u8> = (0..size.max(64))
        .map(|i| ((i * 73 + 29) & 0xFF) as u8)
        .collect();
    match payload_type.trim().to_ascii_lowercase().as_str() {
        "video" => {
            let mut block = MP4_FTYP_HEADER.to_vec();
            block.extend_from_slice(base.get(MP4_FTYP_HEADER.len()..).unwrap_or_default());
            (block, "video/mp4")
        }
        "file" => {
            let mut block = ZIP_LOCAL_HEADER.to_vec();
            block.extend_from_slice(base.get(ZIP_LOCAL_HEADER.len()..).unwrap_or_default());
            (block, "application/zip")
        }
        _ => (base, "application/octet-stream"),
    }
}

#[cfg(feature = "server")]
mod tokio_server {
    use std::collections::BTreeMap;
    use std::sync::LazyLock;
    use std::time::Duration;

    use regex::Regex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream, UdpSocket};
    use tokio::time::timeout;

    use super::{MP4_FTYP_HEADER, ZIP_LOCAL_HEADER, build_payload_block};

    static RANGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"bytes=(\d+)-(\d*)").unwrap());
    use crate::{Error, Result};

    /// Configuration for a VPS ground-truth server run.
    #[derive(Debug, Clone)]
    pub struct VpsServerOptions {
        /// Bind address (normally `127.0.0.1`).
        pub host: String,
        /// TCP port (0 asks the OS for an ephemeral port).
        pub port: u16,
        /// UDP echo port; `None` reuses the bound TCP port.
        pub udp_port: Option<u16>,
        /// Stop after this duration; `None` runs until cancelled.
        pub duration: Option<Duration>,
    }

    impl Default for VpsServerOptions {
        fn default() -> Self {
            Self {
                host: "127.0.0.1".to_owned(),
                port: 8765,
                udp_port: None,
                duration: None,
            }
        }
    }

    /// Bound-port information returned by [`run_vps_server`].
    #[derive(Debug, Clone, PartialEq)]
    pub struct VpsServerInfo {
        /// Effective bind host.
        pub host: String,
        /// Actual TCP port.
        pub port: u16,
        /// Whether the UDP echo socket was started.
        pub udp: bool,
    }

    fn percent_decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) = (
                    (bytes[i + 1] as char).to_digit(16),
                    (bytes[i + 2] as char).to_digit(16),
                ) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn query_params(raw_path: &str) -> BTreeMap<String, String> {
        let mut params = BTreeMap::new();
        if let Some((_, query)) = raw_path.split_once('?') {
            for pair in query.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    params.insert(percent_decode(k), percent_decode(v));
                }
            }
        }
        params
    }

    async fn read_line(stream: &mut TcpStream) -> Result<String> {
        let mut line = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            let n = timeout(Duration::from_secs(5), stream.read(&mut byte))
                .await
                .map_err(|_| Error::failure("VPS server request timeout"))??;
            if n == 0 {
                break;
            }
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&line).trim().to_owned())
    }

    async fn handle_client(mut stream: TcpStream) {
        let result = handle_client_inner(&mut stream).await;
        if let Err(err) = result {
            tracing_debug(&err.to_string());
        }
        let _ = stream.shutdown().await;
    }

    fn tracing_debug(message: &str) {
        eprintln!("paradl-throttle server: {message}");
    }

    async fn handle_client_inner(stream: &mut TcpStream) -> Result<()> {
        let request_line = read_line(stream).await?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_ascii_uppercase();
        let raw_path = parts.next().unwrap_or("").to_owned();
        if method.is_empty() || raw_path.is_empty() {
            return Ok(());
        }
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        loop {
            let line = read_line(stream).await?;
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let path = raw_path.split(['?', '#']).next().unwrap_or("");
        let params = query_params(&raw_path);

        if method == "POST" && (path == "/up" || path == "/upload") {
            let content_len: usize = headers
                .get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let mut buffer = vec![0_u8; content_len.min(512 * 1024 * 1024)];
            let mut received = 0_usize;
            while received < buffer.len() {
                let n = timeout(Duration::from_secs(5), stream.read(&mut buffer[received..]))
                    .await
                    .map_err(|_| Error::failure("VPS upload timeout"))??;
                if n == 0 {
                    break;
                }
                received += n;
            }
            let body = format!("{{\"status\": \"ok\", \"received_bytes\": {received}}}");
            return write_json(stream, &body).await;
        }

        if path == "/echo" {
            let pairs: Vec<String> = headers
                .iter()
                .map(|(k, v)| format!("\"{k}\": \"{v}\""))
                .collect();
            let body = format!(
                "{{\"path\": \"{}\", \"headers\": {{{}}}}}",
                raw_path.replace('"', "\\\""),
                pairs.join(", ")
            );
            return write_json(stream, &body).await;
        }

        let total_size: i64 = params
            .get("bytes")
            .and_then(|v| v.parse().ok())
            .unwrap_or(4_194_304)
            .clamp(1024, 512 * 1024 * 1024);
        let payload_type = params.get("type").map(String::as_str).unwrap_or("random");
        let (block, content_type) = build_payload_block(payload_type, 65_536);

        let mut start = 0_i64;
        let mut end = total_size - 1;
        let mut is_206 = false;
        if let Some(range) = headers.get("range") {
            if let Some(caps) = RANGE_RE.captures(range) {
                start = caps[1].parse::<i64>().unwrap_or(0).min(total_size - 1);
                if !caps[2].is_empty() {
                    end = caps[2].parse::<i64>().unwrap_or(end).min(total_size - 1);
                }
                is_206 = true;
            }
        }
        let slice_len = (end - start + 1).max(0);
        let status_line = if is_206 {
            "HTTP/1.1 206 Partial Content\r\n"
        } else {
            "HTTP/1.1 200 OK\r\n"
        };
        let extra_range = if is_206 {
            format!("Content-Range: bytes {start}-{end}/{total_size}\r\n")
        } else {
            String::new()
        };
        let response_header = format!(
            "{status_line}Content-Type: {content_type}\r\nContent-Length: {slice_len}\r\nAccept-Ranges: bytes\r\n{extra_range}Connection: close\r\n\r\n"
        );
        stream.write_all(response_header.as_bytes()).await?;
        if method == "HEAD" {
            return Ok(());
        }
        let mut sent = 0_i64;
        while sent < slice_len {
            let step = (block.len() as i64).min(slice_len - sent) as usize;
            let offset = (sent % block.len() as i64) as usize;
            let chunk_end = (offset + step).min(block.len());
            stream.write_all(&block[offset..chunk_end]).await?;
            sent += (chunk_end - offset) as i64;
            if chunk_end < offset + step {
                // Wrap around the deterministic block for large ranges.
                let remaining = offset + step - chunk_end;
                stream.write_all(&block[..remaining]).await?;
                sent += remaining as i64;
            }
        }
        Ok(())
    }

    async fn write_json(stream: &mut TcpStream, body: &str) -> Result<()> {
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        Ok(())
    }

    async fn udp_echo(socket: UdpSocket) {
        let mut buf = [0_u8; 65_536];
        while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
            let _ = socket.send_to(&buf[..len], addr).await;
        }
    }

    /// A bound VPS server handle exposing the actual port before it runs.
    pub struct VpsServer {
        info: VpsServerInfo,
        server: tokio::task::JoinHandle<()>,
        udp_task: Option<tokio::task::JoinHandle<()>>,
    }

    impl VpsServer {
        /// Bind the TCP listener (and optional UDP echo socket) immediately.
        pub async fn bind(options: VpsServerOptions) -> Result<Self> {
            let listener = TcpListener::bind(format!("{}:{}", options.host, options.port))
                .await
                .map_err(Error::from)?;
            let port = listener.local_addr().map_err(Error::from)?.port();
            let udp_port = options.udp_port.unwrap_or(port);
            let udp_socket = UdpSocket::bind(format!("{}:{udp_port}", options.host))
                .await
                .ok();
            let udp = udp_socket.is_some();
            let server = tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(handle_client(stream));
                }
            });
            let udp_task = udp_socket.map(udp_echo).map(tokio::spawn);
            Ok(Self {
                info: VpsServerInfo {
                    host: options.host,
                    port,
                    udp,
                },
                server,
                udp_task,
            })
        }

        /// Return the bound host, port, and UDP status.
        pub fn info(&self) -> &VpsServerInfo {
            &self.info
        }

        /// Stop the TCP and UDP tasks.
        pub fn stop(&self) {
            self.server.abort();
            if let Some(task) = &self.udp_task {
                task.abort();
            }
        }
    }

    /// Bind and run the VPS server for `options.duration` (or until cancelled).
    pub async fn run_vps_server(options: VpsServerOptions) -> Result<VpsServerInfo> {
        let duration = options.duration;
        let server = VpsServer::bind(options).await?;
        let info = server.info().clone();
        if let Some(duration) = duration {
            tokio::time::sleep(duration).await;
        } else {
            std::future::pending::<()>().await;
        }
        server.stop();
        Ok(info)
    }

    /// Verify the payload headers used by the streaming endpoints.
    pub fn payload_headers_are_stable() -> bool {
        MP4_FTYP_HEADER[4..8] == *b"ftyp" && ZIP_LOCAL_HEADER[0..2] == *b"PK"
    }
}

#[cfg(feature = "server")]
pub use tokio_server::{
    VpsServer, VpsServerInfo, VpsServerOptions, payload_headers_are_stable, run_vps_server,
};
