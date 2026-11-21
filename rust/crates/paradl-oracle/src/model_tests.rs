//! Golden assertions mirrored from `tests/unit/test_models.py` and `tests/unit/test_config.py`.

use crate::fixtures::{fixture, fixture_str};

#[test]
fn golden_parse_rate() {
    let g = fixture("core_models.json");
    let rates = &g["parse_rate"];
    let expect = [
        ("None", 0.0),
        ("1000", 1000.0),
        ("1K", 1024.0),
        ("1KB/s", 1024.0),
        ("1KBps", 1024.0),
        ("1M", 1048576.0),
        ("4MBps", 4194304.0),
        ("1Kbps", 128.0),
        ("1Mbps", 131072.0),
        ("0", 0.0),
        ("off", 0.0),
        ("none", 0.0),
        ("1GiB/s", 1073741824.0),
        ("8Mbit/s", 1048576.0),
        ("1.5MB", 1572864.0),
        (" 2.5 MB/s ", 2621440.0),
    ];
    for (key, want) in expect {
        let got = rates[key]["value"]
            .as_f64()
            .unwrap_or_else(|| panic!("parse_rate[{key}] was not a number: {}", rates[key]));
        assert_eq!(got, want, "parse_rate({key})");
    }
    // Unparseable/negative/empty inputs must carry the exact ValueError text.
    assert_eq!(
        rates["abc"]["error"],
        "Unparseable rate specification: 'abc'"
    );
    assert_eq!(rates["-5"]["error"], "Unparseable rate specification: '-5'");
    assert_eq!(rates[""]["error"], "Empty rate string");
    assert_eq!(rates["abc"]["error_type"], "ValueError");
}

#[test]
fn golden_format_rate_for_aria2() {
    let g = fixture("core_models.json");
    let r = &g["format_rate_for_aria2"];
    assert_eq!(r["1MB/s"]["value"], "1048576");
    assert_eq!(r["4MBps"]["value"], "4194304");
    assert_eq!(r["1Kbps"]["value"], "128");
    assert_eq!(r["512K"]["value"], "524288");
    assert!(r["off"]["value"].is_null());
    assert!(r["None"]["value"].is_null());
    assert!(r["0"]["value"].is_null());
}

#[test]
fn golden_format_size() {
    let g = fixture("core_models.json");
    let s = &g["format_size"];
    let expect = [
        ("None", "Unknown size"),
        ("0", "0 B"),
        ("0.0", "0 B"),
        ("512", "512.0 B"),
        ("1024", "1.0 KB"),
        ("1536.0", "1.5 KB"),
        ("3145728", "3.0 MB"),
        ("1073741824", "1.0 GB"),
        ("1099511627776", "1.0 TB"),
        ("1125899906842624", "1.0 PB"),
        ("-1", "0 B"),
    ];
    for (key, want) in expect {
        assert_eq!(s[key].as_str().unwrap(), want, "format_size({key})");
    }
}

#[test]
fn golden_format_speed() {
    let g = fixture("core_models.json");
    let s = &g["format_speed"];
    let expect = [
        ("0", "0 B/s"),
        ("500", "500 B/s"),
        ("1536", "1.5 KB/s"),
        ("2621440.0", "2.5 MB/s"),
        ("1024", "1.0 KB/s"),
        ("1048576", "1.0 MB/s"),
        ("500.6", "501 B/s"),
        ("-1", "-1 B/s"),
    ];
    for (key, want) in expect {
        assert_eq!(s[key].as_str().unwrap(), want, "format_speed({key})");
    }
}

#[test]
fn golden_sanitize_filename() {
    let g = fixture("core_models.json");
    let s = &g["sanitize_filename"];
    let expect = [
        ("a/b:c*d?e\"f<g>h|i", "a_b_c_d_e_f_g_h_i"),
        ("a\\b:c", "a_b_c"),
        ("a\u{0}b", "a_b"),
        ("...", "download"),
        (".", "download"),
        ("  ..  ", "download"),
        ("CON", "_CON"),
        ("con", "_con"),
        ("NUL.txt", "_NUL.txt"),
        ("CON.txt", "_CON.txt"),
        ("PRN", "_PRN"),
        ("AUX", "_AUX"),
        ("COM1", "_COM1"),
        ("COM9.log", "_COM9.log"),
        ("LPT1", "_LPT1"),
        ("LPT9.txt", "_LPT9.txt"),
        ("CON ", "_CON"),
        ("NUL.", "_NUL."),
        ("  file  ", "file"),
        ("normal_name.txt", "normal_name.txt"),
        ("", "download"),
        ("   ", "download"),
        ("...hidden", "...hidden"),
        ("COM0", "COM0"),
        ("COM10", "COM10"),
        ("LPT0", "LPT0"),
    ];
    for (key, want) in expect {
        assert_eq!(s[key].as_str().unwrap(), want, "sanitize_filename({key:?})");
    }
}

#[test]
fn golden_sanitize_filename_200_char_cap() {
    let g = fixture("core_models.json");
    let cap = &g["sanitize_200_cap"];
    assert_eq!(cap["input_len"], 205);
    assert_eq!(cap["output_len"], 200);
    let out = cap["output"].as_str().unwrap();
    assert_eq!(out.len(), 200);
    assert!(out.chars().all(|c| c == 'x'));
}

#[test]
fn golden_domain_classification() {
    let g = fixture("core_models.json");
    let m = &g["is_media_domain"];
    for (url, want) in [
        ("https://www.youtube.com/watch?v=x", true),
        ("https://youtu.be/x", true),
        ("https://soundcloud.com/artist/track", true),
        ("https://vimeo.com/12345", true),
        ("https://twitch.tv/channel", true),
        ("https://music.youtube.com/x", true),
        ("https://example.com/file.zip", false),
        ("https://github.com/owner/repo", false),
    ] {
        assert_eq!(m[url].as_bool().unwrap(), want, "is_media_domain({url})");
    }
    let i = &g["is_image_domain"];
    for (url, want) in [
        ("https://image.pollinations.ai/prompt/sunset", true),
        ("https://i.imgur.com/abcde", true),
        ("https://i.redd.it/xyz", true),
        ("https://cdn.discordapp.com/attachments/123", true),
        ("https://images.unsplash.com/photo", true),
        ("https://example.com/image.png", false),
    ] {
        assert_eq!(i[url].as_bool().unwrap(), want, "is_image_domain({url})");
    }
}

#[test]
fn golden_enum_values_and_exit_codes() {
    let g = fixture("core_models.json");
    let e = &g["enums"];
    let tt = &e["TargetType"];
    for (name, val) in [
        ("TORRENT", "torrent"),
        ("DIRECT", "direct"),
        ("STREAM", "stream"),
        ("SEARCH", "search"),
        ("GITHUB", "github"),
        ("HF", "huggingface"),
        ("IPSW", "ipsw"),
        ("PYPI", "pypi"),
        ("BREW", "brew"),
    ] {
        assert_eq!(tt[name].as_str().unwrap(), val);
    }
    let wm = &e["WatchdogMode"];
    assert_eq!(wm["V1"], "v1");
    assert_eq!(wm["V2"], "v2");
    assert_eq!(wm["OFF"], "off");
    let ec = &e["ExitCode"];
    for (name, val) in [
        ("OK", 0),
        ("FAILURE", 1),
        ("USAGE", 2),
        ("VERIFICATION_FAILED", 3),
        ("MISSING_DEPENDENCY", 4),
        ("INTERRUPTED", 130),
    ] {
        assert_eq!(ec[name].as_i64().unwrap(), val, "ExitCode::{name}");
    }
}

#[test]
fn golden_direct_extensions_and_domain_lists() {
    let g = fixture("core_models.json");
    let exts = g["DIRECT_EXTENSIONS"].as_array().unwrap();
    for ext in [
        ".zip", ".iso", ".dmg", ".exe", ".msi", ".pdf", ".mp4", ".mp3", ".tar", ".gz",
    ] {
        assert!(
            exts.iter().any(|v| v.as_str() == Some(ext)),
            "missing {ext}"
        );
    }
    let media = g["MEDIA_DOMAINS"].as_array().unwrap();
    for d in [
        "youtube.com",
        "youtu.be",
        "soundcloud.com",
        "vimeo.com",
        "twitch.tv",
    ] {
        assert!(media.iter().any(|v| v.as_str() == Some(d)), "missing {d}");
    }
    let image = g["IMAGE_DOMAINS"].as_array().unwrap();
    for d in [
        "image.pollinations.ai",
        "i.imgur.com",
        "i.redd.it",
        "cdn.discordapp.com",
    ] {
        assert!(image.iter().any(|v| v.as_str() == Some(d)), "missing {d}");
    }
}

#[test]
fn golden_config_defaults() {
    let g = fixture("config_defaults.json");
    assert_eq!(g["schema_version"].as_i64().unwrap(), 2);
    let c = &g["asdict"];
    assert_eq!(c["schema_version"].as_i64().unwrap(), 2);
    assert_eq!(c["connections"].as_i64().unwrap(), 16);
    assert_eq!(c["watchdog"].as_str().unwrap(), "v2");
    assert_eq!(c["anti_throttle"].as_str().unwrap(), "4MBps");
    assert_eq!(c["anti_throttle_grace"].as_i64().unwrap(), 5);
    assert_eq!(c["history_limit"].as_i64().unwrap(), 50);
    assert!(c["caffeinate"].as_bool().unwrap());
    assert!(c["net_watch"].as_bool().unwrap());
    assert_eq!(c["stall_timeout"].as_f64().unwrap(), 30.0);
    assert!(!c["require_ac"].as_bool().unwrap());
    assert!(!c["pause_on_low_power"].as_bool().unwrap());
    assert_eq!(c["isp"].as_str().unwrap(), "auto");
    assert_eq!(c["engine"].as_str().unwrap(), "rust");
    assert!(!c["bin_dir"].as_str().unwrap().is_empty());
}

#[test]
fn fixtures_are_valid_json_text() {
    // Each fixture is a JSON object (never a bare value), so later crates can parse by key.
    for name in crate::fixtures::FIXTURE_NAMES {
        let text = fixture_str(name);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v.is_object(), "fixture {name} is not a JSON object");
    }
}

// ---------------------------------------------------------------------------
// Contract tests against future crates — RED until those crates land (GREEN then).
// ---------------------------------------------------------------------------

/// The minimal surface a future `paradl-core` implementation must satisfy.
pub trait CoreContract {
    /// Sanitize a filename exactly as `paradl.models.sanitize_filename`.
    fn sanitize_filename(name: &str) -> String;
    /// Format a speed exactly as `paradl.models.format_speed`.
    fn format_speed(bps: f64) -> String;
}

/// Placeholder used only so the ignored contract tests compile before W0-3 exists.
pub struct PendingCore;

impl CoreContract for PendingCore {
    fn sanitize_filename(_name: &str) -> String {
        unimplemented!("paradl-core (W0-3) not yet implemented")
    }
    fn format_speed(_bps: f64) -> String {
        unimplemented!("paradl-core (W0-3) not yet implemented")
    }
}

/// A future crate that implements [`CoreContract`] must reproduce every sanitize golden.
#[test]
#[ignore = "depends on paradl-core (W0-3): remove ignore once the crate exists"]
fn contract_sanitize_filename_matches_golden() {
    let g = fixture("core_models.json");
    let s = &g["sanitize_filename"];
    for (key, val) in s.as_object().unwrap() {
        let want = val.as_str().unwrap();
        assert_eq!(
            PendingCore::sanitize_filename(key),
            want,
            "sanitize_filename({key:?})"
        );
    }
    let cap = &g["sanitize_200_cap"];
    let input = "x".repeat(cap["input_len"].as_u64().unwrap() as usize);
    assert_eq!(
        PendingCore::sanitize_filename(&input),
        cap["output"].as_str().unwrap()
    );
}

/// A future crate must reproduce every format_speed golden, including the negative edge case.
#[test]
#[ignore = "depends on paradl-core (W0-3): remove ignore once the crate exists"]
fn contract_format_speed_matches_golden() {
    let g = fixture("core_models.json");
    for (key, val) in g["format_speed"].as_object().unwrap() {
        let bps: f64 = key.parse().unwrap();
        assert_eq!(PendingCore::format_speed(bps), val.as_str().unwrap());
    }
}
