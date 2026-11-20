//! Runnable walkthrough of the `paradl-media/1` client using a stub worker.
//!
//! No Python or yt-dlp is needed: `sh -c` reads the single request line and
//! emits canned `progress` and `result` frames, exactly like the real worker.

use std::path::PathBuf;

use paradl_media::{
    MediaRequest, MediaResolver, NoopProgressSink, ProgressSink, WorkerLaunch,
    WorkerMediaResolver,
};

struct PrintLines;

impl ProgressSink for PrintLines {
    fn on_progress(&mut self, line: &str) {
        print!("worker progress event: {line}");
    }
}

const STUB: &str = r#"
read -r req || exit 0
id=$(printf '%s' "$req" | sed -n 's/.*"request_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
op=$(printf '%s' "$req" | sed -n 's/.*"op"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
if [ "$op" = "claims" ]; then
  printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"7.0"},"caps":["ejs"],"data":{"claims":true}}\n' "$id"
  exit 0
fi
printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"progress","downloaded_bytes":50,"total_bytes":100,"speed_bps":25,"eta_seconds":2.0}\n' "$id"
printf '{"protocol":"paradl-media/1","request_id":"%s","frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"7.0"},"caps":["ejs"],"data":{"filename":"clip.m4a","http_headers":{"Referer":"https://example.com/"},"audio_format_produced":"passthrough_aac","paths":["/tmp/clip.m4a"]}}\n' "$id"
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = WorkerMediaResolver::with_launch(WorkerLaunch::new("sh", ["-c", STUB]));
    let request = MediaRequest::new(
        "https://youtube.com/watch?v=demo",
        PathBuf::from("/tmp/paradl-media-example"),
    );

    println!("claims: {}", resolver.claims(&request.target).await?);
    let result = resolver.resolve(request.clone()).await?;
    println!("route: {:?}, filename: {:?}", result.route, result.data.map(|d| d.filename));

    let result = resolver.download_managed(request, &mut PrintLines).await?;
    println!("downloaded {:?} with caps {:?}", result.data.and_then(|d| d.paths.first().cloned()), result.caps);

    let _ = NoopProgressSink;
    Ok(())
}
