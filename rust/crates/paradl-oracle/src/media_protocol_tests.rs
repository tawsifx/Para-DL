//! `paradl-media/1` schema conformance tests: version handshake (N, N−1, N+1, unknown,
//! missing), protocol frames, byte-exact worker contracts and audio-format literals.
//!
//! The media worker is Python (`python -m paradl_media`), one-shot, JSON-lines over stdin/stdout.
//! Its protocol is specified authoritatively in `docs/superpowers/framework-charter.md` §4.

use crate::fixtures::fixture;

/// Accepted, or rejected with a Rust-side exit code and diagnostic.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Handshake succeeded; proceed with the worker.
    Accept,
    /// Fail closed with the given exit code and diagnostic.
    Reject(i32, &'static str),
}

/// Reference implementation of the `paradl-media/1` version handshake rule.
///
/// Major version must match; minor additions are tolerated; missing/unparseable versions fail
/// closed. This mirrors the charter's handshake rules and is the oracle for boundary-lane tests.
fn check_protocol_version(protocol: Option<&str>) -> Verdict {
    let Some(raw) = protocol else {
        return Verdict::Reject(4, "protocol version missing");
    };
    let Some(rest) = raw.strip_prefix("paradl-media/") else {
        return Verdict::Reject(4, "unparseable protocol version");
    };
    let major = rest.split('.').next().unwrap_or("");
    match major.parse::<u32>() {
        Ok(1) => Verdict::Accept, // any minor (1.0, 1.1, 1.999) is tolerated
        Ok(_) => Verdict::Reject(4, "unsupported protocol major version"),
        Err(_) => Verdict::Reject(4, "unparseable protocol version"),
    }
}

#[test]
fn golden_media_protocol_schema() {
    let g = fixture("media_protocol.json");
    assert_eq!(g["protocol_id"], "paradl-media/1");
    assert_eq!(g["current_major"].as_i64().unwrap(), 1);

    let fields = |key: &str| -> Vec<&str> {
        g[key]
            .as_array()
            .unwrap_or_else(|| panic!("missing {key}"))
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect()
    };
    assert_eq!(
        fields("request_fields"),
        vec![
            "protocol",
            "request_id",
            "op",
            "target",
            "out_dir",
            "options"
        ]
    );
    assert_eq!(
        fields("options_fields"),
        vec!["audio_only", "format", "cookies_from_browser"]
    );
    assert_eq!(
        fields("progress_fields"),
        vec![
            "protocol",
            "request_id",
            "frame",
            "downloaded_bytes",
            "total_bytes",
            "speed_bps",
            "eta_seconds",
        ]
    );
    assert_eq!(
        fields("result_fields"),
        vec![
            "protocol",
            "request_id",
            "frame",
            "ok",
            "versions",
            "caps",
            "data",
        ]
    );
    assert_eq!(
        fields("error_fields"),
        vec!["protocol", "request_id", "frame", "ok", "error", "versions"]
    );
    assert_eq!(
        fields("versions_fields"),
        vec![
            "worker_version",
            "yt_dlp_version",
            "mutagen_version",
            "ffmpeg_version"
        ]
    );
    assert_eq!(
        fields("data_fields"),
        vec!["filename", "http_headers", "audio_format_produced", "paths"]
    );
    assert_eq!(
        fields("ops"),
        vec!["resolve", "download_managed", "finalize", "claims"]
    );
    assert_eq!(fields("frames"), vec!["progress", "result", "error"]);
    assert_eq!(fields("caps"), vec!["ejs", "mutagen", "ffmpeg"]);
    assert_eq!(
        fields("error_kinds"),
        vec![
            "metadata",
            "download",
            "conversion",
            "dependency",
            "usage",
            "interrupted",
        ]
    );
}

#[test]
fn golden_error_kind_to_exit_code_mapping() {
    let g = fixture("media_protocol.json");
    let m = &g["error_kind_to_exit_code"];
    for (kind, code) in [
        ("metadata", 1),
        ("download", 1),
        ("conversion", 1),
        ("dependency", 4),
        ("usage", 2),
        ("interrupted", 130),
    ] {
        assert_eq!(m[kind].as_i64().unwrap(), code, "error.kind={kind}");
    }
    // The worker NEVER emits exit code 3 — checksums are Rust-owned after finalize.
    assert!(
        m.as_object()
            .unwrap()
            .values()
            .all(|v| v.as_i64() != Some(3))
    );
    assert_eq!(g["worker_never_emits"].as_i64().unwrap(), 3);
}

#[test]
fn golden_byte_exact_worker_contracts() {
    let g = fixture("media_protocol.json");
    let c = &g["byte_exact_contracts"];
    assert_eq!(c["http_headers"], "values coerced to str, keys unchanged");
    assert_eq!(
        c["filename"],
        "sanitize_filename: 200-char cap, Windows reserved names"
    );
    assert_eq!(
        c["audio_format_produced"],
        "classify_audio_production (media.py:48-72)"
    );

    // filename is the same sanitize_filename golden asserted in model_tests.rs.
    let cm = fixture("core_models.json");
    assert_eq!(cm["sanitize_200_cap"]["output_len"].as_i64().unwrap(), 200);
    assert_eq!(cm["sanitize_filename"]["CON"], "_CON");
    assert_eq!(cm["sanitize_filename"]["NUL.txt"], "_NUL.txt");
}

#[test]
fn golden_audio_format_production_literals() {
    let g = fixture("media_protocol.json");
    let a = &g["audio_format_production"];
    let lossless = a["lossless_source_codecs"].as_array().unwrap();
    for codec in [
        "alac",
        "ape",
        "flac",
        "pcm",
        "pcm_f32le",
        "pcm_s16le",
        "pcm_s24le",
        "wav",
        "wv",
    ] {
        assert!(
            lossless.iter().any(|v| v.as_str() == Some(codec)),
            "missing {codec}"
        );
    }
    let cases = a["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 14);
    let expected = [
        ("passthrough AAC (from mp4a source, no re-encode)"),
        ("ALAC-wrapped lossy (transcoded from lossy mp4a source)"),
        ("ALAC-wrapped lossy (transcoded from lossy mp4a source)"),
        ("true lossless ALAC (from lossless alac source)"),
        ("true lossless FLAC (from flac source)"),
        ("FLAC (transcoded from lossy mp4a source)"),
        ("FLAC (transcoded from lossy opus source)"),
        ("uncompressed WAV PCM (from mp3 source)"),
        ("uncompressed WAV PCM (from pcm_s16le source)"),
        ("MP3 (from mp3 source)"),
        ("passthrough AAC (from unknown source, no re-encode)"),
        ("passthrough AAC (from unknown source, no re-encode)"),
        ("true lossless FLAC (from flac source)"),
        ("OPUS (from vorbis source)"),
    ];
    for (case, want) in cases.iter().zip(expected) {
        assert_eq!(case["result"].as_str().unwrap(), want, "case {case:?}");
    }
}

#[test]
fn golden_version_conformance_n_n_minus_1_n_plus_1_unknown_missing() {
    let g = fixture("media_protocol.json");
    let matrix = g["version_conformance"].as_array().unwrap();
    // Required coverage labels from the charter: N, N−1, N+1, unknown, missing.
    let mut labels = std::collections::BTreeSet::new();
    for case in matrix {
        let label = case["label"].as_str().unwrap();
        labels.insert(label);
        let raw = case["protocol"].as_str(); // null == missing
        let got = check_protocol_version(raw);
        let want_code = case["exit_code"].as_i64().unwrap();
        if case["accepted"].as_bool().unwrap() {
            assert_eq!(got, Verdict::Accept, "case {label}");
            assert_eq!(want_code, 0);
        } else {
            let Verdict::Reject(code, reason) = got else {
                panic!("case {label} should fail closed, got {got:?}");
            };
            assert_eq!(code as i64, want_code, "case {label} exit code");
            assert_eq!(reason, case["diagnostic"].as_str().unwrap(), "case {label}");
        }
    }
    for required in ["N", "N-1", "N+1", "unknown", "missing"] {
        assert!(
            labels.contains(required),
            "missing conformance case {required}"
        );
    }
}

/// Contract test for the future `paradl-media` Rust protocol client (W3-4b).
#[test]
#[ignore = "depends on paradl-media (W3-4b): remove ignore once the crate exists"]
fn contract_media_handshake_fails_closed() {
    let g = fixture("media_protocol.json");
    assert_eq!(g["protocol_id"], "paradl-media/1");
    unimplemented!("paradl-media (W3-4b) not yet implemented");
}
