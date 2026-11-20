//! End-to-end tests against a stub one-shot worker launched with `sh -c`.
//!
//! The stub emits canned protocol frames on stdout, exactly like the real
//! `python -m paradl_media` worker, so the Rust client is exercised at the
//! process boundary (spawn, stdin request, stdout parsing, error mapping).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use paradl_media::{
    MediaRequest, MediaResolver, MediaRoute, NoopProgressSink, ProgressSink, WorkerLaunch,
    WorkerMediaResolver,
};

/// Shared shell helpers: consume the single request line, extract `request_id`
/// and `op`, then provide emitters for each frame type.
const PRELUDE: &str = r#"
read -r req || exit 0
id=$(printf '%s' "$req" | sed -n 's/.*"request_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
op=$(printf '%s' "$req" | sed -n 's/.*"op"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
result() {
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"7.0"},"caps":["ejs","mutagen","ffmpeg"],"data":%s}\n' "$id" "$1"
}
raw_result() {
  printf '{"protocol":"%s","request_id":"%s","frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"},"caps":[],"data":{}}\n' "$1" "$id"
}
result_with_top_claims() {
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"unknown"},"caps":[],"claims":false,"data":{"claims":true,"duration":12.5}}\n' "$id"
}
result_no_versions() {
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"result","ok":true,"data":{}}\n' "$id"
}
progress() {
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"progress","downloaded_bytes":1024,"total_bytes":4096,"speed_bps":512,"eta_seconds":6.0}\n' "$id"
}
error() {
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"error","ok":false,"error":{"kind":"%s","message":"%s"},"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"}}\n' "$id" "$1" "$2"
}
"#;

fn stub(body: &str) -> WorkerMediaResolver {
    let script = format!("{PRELUDE}\n{body}\n");
    WorkerMediaResolver::with_launch(WorkerLaunch::new("sh", ["-c".to_owned(), script]))
}

#[derive(Default)]
struct Collect(Vec<String>);

impl ProgressSink for Collect {
    fn on_progress(&mut self, line: &str) {
        self.0.push(line.to_owned());
    }
}

fn media_request() -> MediaRequest {
    MediaRequest::new(
        "https://example.com/watch?v=abc",
        PathBuf::from("/tmp/paradl-media-out"),
    )
}

#[tokio::test]
async fn stub_worker_progress_is_re_emitted_as_redacted_events() {
    paradl_events::register_secret("S3cr3tTok3n");
    let resolver = stub(
        r#"
case "$op" in
  claims) result '{"claims":true}' ;;
  resolve|download_managed) progress; result '{"filename":"song.m4a","http_headers":{"Referer":"https://example.com/"},"audio_format_produced":"passthrough_aac","paths":[]}' ;;
  *) error usage "unexpected op $op" ;;
esac
"#,
    );
    let request = MediaRequest::new(
        "https://example.com/S3cr3tTok3n.mp4",
        PathBuf::from("/tmp/paradl-media-out"),
    );
    let mut sink = Collect::default();

    let result = resolver.download_managed(request, &mut sink).await.unwrap();
    assert_eq!(result.route, MediaRoute::Managed);
    let data = result.data.expect("managed result carries worker data");
    assert_eq!(data.filename, "song.m4a");
    assert_eq!(
        data.http_headers.get("Referer").map(String::as_str),
        Some("https://example.com/"),
    );
    assert_eq!(data.audio_format_produced.as_deref(), Some("passthrough_aac"));

    assert_eq!(sink.0.len(), 1, "one progress frame -> one event line");
    let line = &sink.0[0];
    assert!(line.starts_with("{\"event\": \"progress\", \"timestamp\": "), "{line}");
    assert!(line.contains("\"downloaded_bytes\": 1024"), "{line}");
    assert!(line.contains("\"total_bytes\": 4096"), "{line}");
    assert!(!line.contains("S3cr3tTok3n"), "secret leaked into event: {line}");
}

#[tokio::test]
async fn claims_reads_data_claims_and_tolerates_extra_metadata() {
    // W3-4a puts the boolean at data["claims"] and may add extra metadata keys;
    // a top-level `claims` key is not part of the protocol and is ignored.
    let resolver = stub(
        r#"
case "$op" in
  claims) result_with_top_claims ;;
  resolve) result '{"filename":"clip.m4a","http_headers":{},"audio_format_produced":null,"paths":[],"duration":12.5,"uploader":"someone"}' ;;
  *) error usage "unexpected op" ;;
esac
"#,
    );
    assert!(resolver.claims("https://example.com/x").await.unwrap());
    let result = resolver.resolve(media_request()).await.unwrap();
    assert_eq!(result.route, MediaRoute::Managed);
    let data = result.data.expect("extra keys must not fail parsing");
    assert_eq!(data.filename, "clip.m4a");
}

#[tokio::test]
async fn claims_false_falls_back_to_direct_without_worker_data() {
    let resolver = stub("result '{\"claims\":false}'");
    let sink = Collect::default();
    let result = resolver.resolve(media_request()).await.unwrap();
    assert_eq!(result.route, MediaRoute::Direct);
    assert!(result.data.is_none());
    assert!(sink.0.is_empty());
    assert!(!resolver.claims("https://example.com/watch?v=abc").await.unwrap());
}

#[tokio::test]
async fn dependency_error_maps_to_exit_code_4() {
    let resolver = stub(
        r#"
case "$op" in
  claims) result '{"claims":true}' ;;
  *) error dependency "yt-dlp missing" ;;
esac
"#,
    );
    let error = resolver.resolve(media_request()).await.unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 4);
    assert!(error.to_string().contains("yt-dlp missing"), "{error}");
}

#[tokio::test]
async fn interrupted_error_maps_to_exit_code_130() {
    let resolver = stub("error interrupted \"SIGINT\"");
    let error = resolver.claims("https://example.com/watch?v=abc").await.unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 130);
    // The client never deletes `.pdl.part` / `.pdl.json`; nothing to clean here.
}

#[tokio::test]
async fn interrupted_error_preserves_part_and_sidecar_files() {
    let out_dir = std::env::temp_dir().join(format!(
        "paradl-media-sidecar-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&out_dir).unwrap();
    let part = out_dir.join("clip.m4a.pdl.part");
    let sidecar = out_dir.join("clip.m4a.pdl.json");
    std::fs::write(&part, b"partial").unwrap();
    std::fs::write(&sidecar, b"{}").unwrap();

    let resolver = stub("error interrupted \"SIGINT\"");
    let request = MediaRequest::new("https://example.com/watch?v=abc", out_dir.clone());
    let error = resolver
        .download_managed(request, &mut NoopProgressSink)
        .await
        .unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 130);
    assert!(part.exists(), ".pdl.part must survive interruption");
    assert!(sidecar.exists(), ".pdl.json must survive interruption");
    let _ = std::fs::remove_dir_all(&out_dir);
}

#[tokio::test]
async fn unknown_major_version_fails_closed() {
    let resolver = stub("raw_result 'paradl-media/2'");
    let error = resolver.claims("https://example.com/watch?v=abc").await.unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 4);
    assert!(
        error.to_string().contains("unsupported protocol major version"),
        "{error}",
    );
}

#[tokio::test]
async fn missing_versions_on_terminal_frame_fails_closed() {
    let resolver = stub("result_no_versions");
    let error = resolver.claims("https://example.com/watch?v=abc").await.unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 4);
    assert!(error.to_string().contains("versions"), "{error}");
}

#[tokio::test]
async fn chatty_worker_stdout_never_becomes_an_event() {
    let resolver = stub("printf 'human log line\\n'; result '{\"claims\":true}'");
    let mut sink = Collect::default();
    let error = resolver
        .download_managed(media_request(), &mut sink)
        .await
        .unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 1);
    assert!(sink.0.iter().all(|line| !line.contains("human log line")));
}

#[tokio::test]
async fn timeout_terminates_a_stalled_worker() {
    let resolver = WorkerMediaResolver::with_launch(WorkerLaunch::new(
        "sh",
        ["-c", "sleep 5; result '{\"claims\":true}'"],
    ))
    .with_timeout(Duration::from_millis(300));
    let start = Instant::now();
    let error = resolver
        .claims("https://example.com/watch?v=abc")
        .await
        .unwrap_err();
    assert_eq!(error.exit_code().as_i32(), 1);
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(start.elapsed() < Duration::from_secs(3), "worker was not killed");
}

#[tokio::test]
async fn request_line_carries_protocol_and_op() {
    let resolver = stub(
        r#"
case "$req" in
  *'"protocol":"paradl-media/1"'*) ;;
  *) error usage "missing protocol on request" ;;
esac
case "$op" in
  claims|resolve|download_managed) result '{"claims":true}' ;;
  *) error usage "bad op" ;;
esac
"#,
    );
    assert!(resolver.claims("https://example.com/watch?v=abc").await.unwrap());
    assert!(resolver.domains().contains(&"youtube.com"));
    let _ = NoopProgressSink;
}
