//! VPS server golden: byte-level HTTP responses for the M9 ground-truth lanes.
#![cfg(feature = "server")]

use std::time::Duration;

use paradl_throttle::server::{VpsServer, VpsServerOptions, payload_headers_are_stable};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn request(port: u16, request: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn range_download_and_echo_responses() {
    assert!(payload_headers_are_stable());
    let server = VpsServer::bind(VpsServerOptions {
        host: "127.0.0.1".to_owned(),
        port: 0,
        udp_port: None,
        duration: None,
    })
    .await
    .unwrap();
    let port = server.info().port;

    let response = request(
        port,
        "GET /down?bytes=4096&type=random HTTP/1.1\r\nRange: bytes=0-15\r\nHost: localhost\r\n\r\n",
    )
    .await;
    let response_text = String::from_utf8_lossy(&response);
    assert!(response_text.starts_with("HTTP/1.1 206 Partial Content\r\n"));
    assert!(response_text.contains("Content-Range: bytes 0-15/4096\r\n"));
    assert!(response_text.contains("Content-Type: application/octet-stream\r\n"));
    assert!(response_text.contains("Content-Length: 16\r\n"));
    let body_start = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    assert_eq!(response.len() - body_start, 16);

    let echo = request(
        port,
        "GET /echo?x=1 HTTP/1.1\r\nHost: localhost\r\nX-Test: yes\r\n\r\n",
    )
    .await;
    let echo_text = String::from_utf8_lossy(&echo);
    assert!(echo_text.contains("\"path\": \"/echo?x=1\""));
    assert!(echo_text.contains("\"x-test\": \"yes\""));

    server.stop();
    let _ = tokio::time::timeout(
        Duration::from_millis(50),
        tokio::time::sleep(Duration::from_secs(1)),
    )
    .await;
}
