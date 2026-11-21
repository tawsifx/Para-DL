//! Contract tests for the `paradl-media/1` wire protocol.
//!
//! The handshake and error-code matrices are read straight from the read-only
//! golden fixture (`paradl-oracle/fixtures/media_protocol.json`) so this crate
//! cannot drift from the agreed schema.

use paradl_core::ExitCode;
use paradl_media::{
    PROTOCOL_ID, PROTOCOL_MAJOR, WorkerFrame, check_protocol, error_kind_to_exit_code,
};

const GOLDEN: &str = include_str!("../../paradl-oracle/fixtures/media_protocol.json");

fn frame_line(id: &str, protocol: Option<&str>, fields: &str) -> String {
    let prefix = match protocol {
        Some(value) => format!("\"protocol\":\"{value}\","),
        None => String::new(),
    };
    format!("{{{prefix}\"request_id\":\"{id}\",{fields}}}")
}

#[test]
fn handshake_matrix_matches_golden_fixture() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden JSON parses");
    assert_eq!(golden["protocol_id"].as_str(), Some(PROTOCOL_ID));
    assert_eq!(
        golden["current_major"].as_u64(),
        Some(PROTOCOL_MAJOR as u64)
    );

    let cases = golden["version_conformance"]
        .as_array()
        .expect("version_conformance array");
    assert!(cases.len() >= 5, "fixture must cover N, N±1, unknown, missing");
    for case in cases {
        let label = case["label"].as_str().unwrap_or("<unlabelled>");
        let version = case["protocol"].as_str(); // JSON null -> None
        let got = check_protocol(version);
        if case["accepted"].as_bool() == Some(true) {
            assert!(got.is_ok(), "{label}: expected acceptance, got {got:?}");
        } else {
            let error = got.expect_err(label);
            assert_eq!(
                error.exit_code().as_i32(),
                case["exit_code"].as_i64().unwrap() as i32,
                "{label}: wrong exit code",
            );
            let diagnostic = case["diagnostic"].as_str().unwrap_or_default();
            assert!(
                error.to_string().contains(diagnostic),
                "{label}: diagnostic {error:?} must contain {diagnostic:?}",
            );
        }
    }
}

#[test]
fn error_kind_mapping_matches_golden_fixture() {
    let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden JSON parses");
    let mapping = golden["error_kind_to_exit_code"].as_object().unwrap();
    for (kind, expected) in mapping {
        let expected = expected.as_i64().unwrap() as i32;
        assert_eq!(
            error_kind_to_exit_code(kind).as_i32(),
            expected,
            "kind {kind}",
        );
        assert_ne!(expected, 3, "the worker never emits verification failure");
    }
    // Unknown kinds fail closed to 1, never to 3 or 0.
    assert_eq!(error_kind_to_exit_code("verification").as_i32(), 1);
    assert_eq!(error_kind_to_exit_code("something-new").as_i32(), 1);
}

#[test]
fn parses_progress_frame_fields() {
    let id = "req-progress";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"progress","downloaded_bytes":0,"total_bytes":null,"speed_bps":0,"eta_seconds":null"#,
    );
    let WorkerFrame::Progress(progress) = WorkerFrame::parse(&line, id).unwrap() else {
        panic!("expected a progress frame");
    };
    assert_eq!(progress.downloaded_bytes, 0);
    assert_eq!(progress.total_bytes, None);
    assert_eq!(progress.speed_bps, 0);
    assert_eq!(progress.eta_seconds, None);

    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"progress","downloaded_bytes":1024,"total_bytes":4096,"speed_bps":512,"eta_seconds":6.25"#,
    );
    let WorkerFrame::Progress(progress) = WorkerFrame::parse(&line, id).unwrap() else {
        panic!("expected a progress frame");
    };
    assert_eq!(progress.downloaded_bytes, 1024);
    assert_eq!(progress.total_bytes, Some(4096));
    assert_eq!(progress.speed_bps, 512);
    assert_eq!(progress.eta_seconds, Some(6.25));

    // W3-4a worker computes speed as a float; both forms must be accepted.
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"progress","downloaded_bytes":7,"total_bytes":null,"speed_bps":512.75,"eta_seconds":null"#,
    );
    let WorkerFrame::Progress(progress) = WorkerFrame::parse(&line, id).unwrap() else {
        panic!("expected a progress frame");
    };
    assert_eq!(progress.speed_bps, 512);
}

#[test]
fn terminal_result_tolerates_extra_worker_metadata_keys() {
    let id = "req-extra";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"unknown"},"caps":[],"data":{"filename":"clip.m4a","http_headers":{},"audio_format_produced":null,"paths":[],"duration":212.5,"webpage_url":"https://example.com/watch?v=x","thumbnail":"https://example.com/thumb.jpg","uploader":"someone"}"#,
    );
    let WorkerFrame::Result { data, .. } = WorkerFrame::parse(&line, id).unwrap() else {
        panic!("expected a result frame");
    };
    let data = paradl_media::MediaData::from_value(&data).unwrap();
    assert_eq!(data.filename, "clip.m4a");
    assert!(data.audio_format_produced.is_none());
}

#[test]
fn worker_handshake_error_kind_dependency_maps_to_exit_4() {
    let id = "req-handshake-error";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"error","ok":false,"error":{"kind":"dependency","message":"unsupported protocol major version: 2"},"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"}"#,
    );
    let WorkerFrame::Error { kind, message, .. } = WorkerFrame::parse(&line, id).unwrap() else {
        panic!("expected an error frame");
    };
    assert_eq!(kind, paradl_media::ErrorKind::Dependency);
    let error = paradl_media::error_kind_to_error(kind.as_str(), message);
    assert_eq!(error.exit_code(), ExitCode::MissingDependency);
    assert_eq!(error.exit_code().as_i32(), 4);
}

#[test]
fn parses_terminal_result_and_versions() {
    let id = "req-result";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"2026.10.26","mutagen_version":"1.48.1","ffmpeg_version":"7.0"},"caps":["ejs","mutagen","ffmpeg"],"data":{"filename":"song.m4a","http_headers":{"Referer":"https://example.com/","X-Count":7},"audio_format_produced":"passthrough AAC (from mp4a source, no re-encode)","paths":["/tmp/song.m4a"]}"#,
    );
    let WorkerFrame::Result {
        versions,
        caps,
        data,
    } = WorkerFrame::parse(&line, id).unwrap()
    else {
        panic!("expected a terminal result frame");
    };
    assert_eq!(versions.worker_version, "0.1.0");
    assert_eq!(versions.yt_dlp_version, "2026.10.26");
    assert_eq!(versions.mutagen_version, "1.48.1");
    assert_eq!(versions.ffmpeg_version, "7.0");
    assert_eq!(caps, ["ejs", "mutagen", "ffmpeg"]);

    let data = paradl_media::MediaData::from_value(&data).unwrap();
    assert_eq!(data.filename, "song.m4a");
    assert_eq!(
        data.http_headers.get("Referer").map(String::as_str),
        Some("https://example.com/"),
    );
    // Worker contract: header values are coerced to str, keys unchanged.
    assert_eq!(
        data.http_headers.get("X-Count").map(String::as_str),
        Some("7"),
    );
    assert_eq!(
        data.audio_format_produced.as_deref(),
        Some("passthrough AAC (from mp4a source, no re-encode)"),
    );
    assert_eq!(data.paths.len(), 1);
}

#[test]
fn terminal_error_kind_maps_to_core_error_and_keeps_versions() {
    let id = "req-error";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"error","ok":false,"error":{"kind":"dependency","message":"yt-dlp missing"},"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"}"#,
    );
    match WorkerFrame::parse(&line, id).unwrap() {
        WorkerFrame::Error { kind, message, versions } => {
            assert_eq!(kind.as_str(), "dependency");
            assert_eq!(message, "yt-dlp missing");
            assert_eq!(versions.worker_version, "0.1.0");
            let error = paradl_media::error_kind_to_error(kind.as_str(), message);
            assert_eq!(error.exit_code(), ExitCode::MissingDependency);
        }
        other => panic!("expected an error frame, got {other:?}"),
    }
}

#[test]
fn terminal_frame_without_versions_fails_closed() {
    let id = "req-noversions";
    let line = frame_line(
        id,
        Some(PROTOCOL_ID),
        r#""frame":"result","ok":true,"caps":[],"data":{}"#,
    );
    let error = WorkerFrame::parse(&line, id).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::MissingDependency);
    assert!(error.to_string().contains("versions"), "{error}");
}

#[test]
fn missing_protocol_field_fails_closed() {
    let id = "req-noprotocol";
    let line = frame_line(
        id,
        None,
        r#""frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"},"caps":[],"data":{}"#,
    );
    let error = WorkerFrame::parse(&line, id).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::MissingDependency);
    assert!(error.to_string().contains("protocol version missing"), "{error}");
}

#[test]
fn mismatched_request_id_is_a_failure() {
    let line = frame_line(
        "other-request",
        Some(PROTOCOL_ID),
        r#""frame":"result","ok":true,"versions":{"worker_version":"0.1.0","yt_dlp_version":"0","mutagen_version":"0","ffmpeg_version":"0"},"caps":[],"data":{}"#,
    );
    let error = WorkerFrame::parse(&line, "expected-request").unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failure);
    assert!(error.to_string().contains("request_id"), "{error}");
}

#[test]
fn malformed_or_unknown_frames_fail_closed() {
    let id = "req-malformed";
    let error = WorkerFrame::parse("not json", id).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failure);

    let line = frame_line(id, Some(PROTOCOL_ID), r#""frame":"surprise""#);
    let error = WorkerFrame::parse(&line, id).unwrap_err();
    assert_eq!(error.exit_code(), ExitCode::Failure);
    assert!(error.to_string().contains("frame"), "{error}");
}
