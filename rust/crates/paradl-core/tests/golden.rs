//! Golden contract tests for `paradl-core`.
//!
//! Every numeric literal below was captured by running the Python oracle in the
//! repository (`.venv/bin/python` with `paradl.core.models` / `paradl.core.config`),
//! never invented. These tests are the RED half of W0-3's TDD cycle.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use paradl_core::clock::{Clock, FakeClock, SystemClock};
use paradl_core::config::{CONFIG_SCHEMA_VERSION, PDLConfig};
use paradl_core::models::{
    DIRECT_EXTENSIONS, IMAGE_DOMAINS, MEDIA_DOMAINS, RateInput, TargetInfo, TargetType,
    TransferResult, WatchdogMode, format_rate_for_aria2, format_size, format_speed,
    has_direct_extension, is_image_domain, is_media_domain, parse_rate, sanitize_filename,
};
use paradl_core::paths::{config_file, get_paradl_home, history_db_file};
use paradl_core::{Error, ExitCode, get_active_engine, set_active_engine};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn temp_dir(label: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "paradl-core-golden-{}-{}-{}",
        std::process::id(),
        label,
        n
    ));
    std::fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn with_paradl_home<F: FnOnce()>(dir: &PathBuf, f: F) {
    let _guard = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev = std::env::var_os("PARADL_HOME");
    // SAFETY: serialized by ENV_LOCK; no other thread reads PARADL_HOME during f().
    unsafe { std::env::set_var("PARADL_HOME", dir) };
    f();
    // SAFETY: same lock scope as above.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("PARADL_HOME", v),
            None => std::env::remove_var("PARADL_HOME"),
        }
    }
}

// ---------------------------------------------------------------------------
// L15 / exit codes (charter section 3)
// ---------------------------------------------------------------------------

#[test]
fn exit_code_values_match_python_int_enum() {
    assert_eq!(ExitCode::Ok.as_i32(), 0);
    assert_eq!(ExitCode::Failure.as_i32(), 1);
    assert_eq!(ExitCode::Usage.as_i32(), 2);
    assert_eq!(ExitCode::VerificationFailed.as_i32(), 3);
    assert_eq!(ExitCode::MissingDependency.as_i32(), 4);
    assert_eq!(ExitCode::Interrupted.as_i32(), 130);
    assert_eq!(ExitCode::Ok.to_string(), "0");
    assert_eq!(i32::from(ExitCode::Interrupted), 130);
}

#[test]
fn error_variants_map_to_exit_codes() {
    assert_eq!(Error::Ok.exit_code(), ExitCode::Ok);
    assert_eq!(Error::failure("x").exit_code(), ExitCode::Failure);
    assert_eq!(Error::usage("x").exit_code(), ExitCode::Usage);
    assert_eq!(
        Error::verification_failed("x").exit_code(),
        ExitCode::VerificationFailed
    );
    assert_eq!(
        Error::missing_dependency("x").exit_code(),
        ExitCode::MissingDependency
    );
    assert_eq!(Error::interrupted("x").exit_code(), ExitCode::Interrupted);
    assert_eq!(
        Error::from(std::io::Error::other("x")).exit_code(),
        ExitCode::Failure
    );
}

#[test]
fn media_error_kind_mapping_matches_contract() {
    assert_eq!(
        Error::from_media_kind("metadata", "m").exit_code(),
        ExitCode::Failure
    );
    assert_eq!(
        Error::from_media_kind("download", "m").exit_code(),
        ExitCode::Failure
    );
    assert_eq!(
        Error::from_media_kind("conversion", "m").exit_code(),
        ExitCode::Failure
    );
    assert_eq!(
        Error::from_media_kind("dependency", "m").exit_code(),
        ExitCode::MissingDependency
    );
    assert_eq!(
        Error::from_media_kind("usage", "m").exit_code(),
        ExitCode::Usage
    );
    assert_eq!(
        Error::from_media_kind("interrupted", "m").exit_code(),
        ExitCode::Interrupted
    );
    assert_eq!(
        Error::from_media_kind("unknown", "m").exit_code(),
        ExitCode::Failure
    );
}

// ---------------------------------------------------------------------------
// Rate parsing / formatting (byte-exact from test_models.py)
// ---------------------------------------------------------------------------

#[test]
fn parse_rate_golden_values() {
    let cases: &[(&str, f64)] = &[
        ("1000", 1000.0),
        ("1K", 1024.0),
        ("1KB", 1024.0),
        ("1KB/s", 1024.0),
        ("1KBps", 1024.0),
        ("1M", 1048576.0),
        ("1MB/s", 1048576.0),
        ("4MBps", 4194304.0),
        ("1Kbps", 128.0),
        ("1Mbps", 131072.0),
        ("1G", 1073741824.0),
        ("1T", 1099511627776.0),
        ("1MiB/s", 1048576.0),
        ("1Mib/s", 131072.0),
        ("1Mbit/s", 131072.0),
        ("1Mbits/s", 131072.0),
        ("1b", 0.125),
        ("1ib", 0.125),
        ("1kib", 128.0),
        ("1bit", 0.125),
        ("1bits", 0.125),
        ("1byte", 1.0),
        ("1bytes", 1.0),
        ("8b", 1.0),
        ("1000.5K", 1024512.0),
        ("0", 0.0),
        ("off", 0.0),
        ("none", 0.0),
        ("false", 0.0),
        ("disable", 0.0),
        ("disabled", 0.0),
        (" 1.5 MB/s ", 1572864.0),
        ("2.5Gbps", 335544320.0),
        ("1kB", 1024.0),
        ("1kb", 128.0),
    ];
    for (input, expected) in cases {
        assert_eq!(
            parse_rate(RateInput::Text((*input).to_owned())).unwrap(),
            *expected,
            "parse_rate({input:?})"
        );
    }
    assert_eq!(parse_rate(RateInput::Int(1000)).unwrap(), 1000.0);
    assert_eq!(parse_rate(RateInput::Float(4.0)).unwrap(), 4.0);
    assert_eq!(parse_rate(RateInput::None).unwrap(), 0.0);
}

#[test]
fn parse_rate_invalid_inputs() {
    assert!(parse_rate(RateInput::Text("abc".into())).is_err());
    assert!(parse_rate(RateInput::Text("-5".into())).is_err());
    assert!(parse_rate(RateInput::Text(String::new())).is_err());
    assert_eq!(
        parse_rate(RateInput::Text("-5".into()))
            .unwrap_err()
            .to_string(),
        "Unparseable rate specification: '-5'"
    );
    assert_eq!(
        parse_rate(RateInput::Text(String::new()))
            .unwrap_err()
            .to_string(),
        "Empty rate string"
    );
    assert_eq!(
        parse_rate(RateInput::Int(-5)).unwrap_err().to_string(),
        "Rate cannot be negative: -5"
    );
}

#[test]
fn format_rate_for_aria2_golden_values() {
    assert_eq!(
        format_rate_for_aria2("1M").unwrap().as_deref(),
        Some("1048576")
    );
    assert_eq!(
        format_rate_for_aria2("4MBps").unwrap().as_deref(),
        Some("4194304")
    );
    assert_eq!(
        format_rate_for_aria2("1Kbps").unwrap().as_deref(),
        Some("128")
    );
    assert_eq!(format_rate_for_aria2("off").unwrap(), None);
    assert_eq!(format_rate_for_aria2("0").unwrap(), None);
    assert_eq!(
        format_rate_for_aria2("1024").unwrap().as_deref(),
        Some("1024")
    );
    assert_eq!(
        format_rate_for_aria2("1.5K").unwrap().as_deref(),
        Some("1536")
    );
    assert!(format_rate_for_aria2("-1").is_err());
}

#[test]
fn format_size_golden_values() {
    let cases: &[(Option<f64>, &str)] = &[
        (None, "Unknown size"),
        (Some(0.0), "0 B"),
        (Some(512.0), "512.0 B"),
        (Some(1024.0), "1.0 KB"),
        (Some(1536.0), "1.5 KB"),
        (Some(3145728.0), "3.0 MB"),
        (Some(1099511627776.0), "1.0 TB"),
        (Some(1125899906842624.0), "1.0 PB"),
        (Some(1152921504606846976.0), "1024.0 PB"),
        (Some(1048575.0), "1024.0 KB"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_size(*input), *expected, "format_size({input:?})");
    }
}

#[test]
fn format_speed_golden_values() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0 B/s"),
        (500.0, "500 B/s"),
        (1024.0, "1.0 KB/s"),
        (1536.0, "1.5 KB/s"),
        (1048576.0, "1.0 MB/s"),
        (2621440.0, "2.5 MB/s"),
        (1048575.0, "1024.0 KB/s"),
        (1000.0, "1000 B/s"),
        (999999.0, "976.6 KB/s"),
    ];
    for (input, expected) in cases {
        assert_eq!(format_speed(*input), *expected, "format_speed({input})");
    }
}

// ---------------------------------------------------------------------------
// sanitize_filename (L02)
// ---------------------------------------------------------------------------

#[test]
fn sanitize_filename_golden_values() {
    let cases: &[(&str, &str)] = &[
        ("a/b:c*d?e\"f<g>h|i", "a_b_c_d_e_f_g_h_i"),
        ("a\0b", "a_b"),
        ("...", "download"),
        ("CON", "_CON"),
        ("NUL.txt", "_NUL.txt"),
        ("  file  ", "file"),
        ("CON.txt", "_CON.txt"),
        ("aux", "_aux"),
        ("COM1", "_COM1"),
        ("LPT9", "_LPT9"),
        ("com10", "com10"),
        ("normal.txt", "normal.txt"),
        ("..", "download"),
        (".", "download"),
        ("   ", "download"),
        ("..\\..\\evil", ".._.._evil"),
        ("  CON  ", "_CON"),
        ("file.", "file."),
        ("a.b.CON", "a.b.CON"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            sanitize_filename(input),
            *expected,
            "sanitize_filename({input:?})"
        );
    }
}

#[test]
fn sanitize_filename_caps_at_200_chars() {
    let long = "a".repeat(250);
    assert_eq!(sanitize_filename(&long).chars().count(), 200);
    assert_eq!(sanitize_filename(&long), "a".repeat(200));
}

// ---------------------------------------------------------------------------
// Domain lists / suffix matching (single-source constants)
// ---------------------------------------------------------------------------

#[test]
fn media_domain_classification_golden_values() {
    let cases: &[(&str, bool)] = &[
        ("https://www.youtube.com/watch?v=x", true),
        ("https://youtu.be/x", true),
        ("https://soundcloud.com/artist/track", true),
        ("https://vimeo.com/12345", true),
        ("https://twitch.tv/channel", true),
        ("https://example.com/file.zip", false),
        ("https://github.com/owner/repo", false),
        ("https://youtube.com.evil.com/x", false),
        ("https://youtube.com./x", true),
        ("https://YOUTUBE.COM/x", true),
        ("not a url", false),
        ("", false),
    ];
    for (url, expected) in cases {
        assert_eq!(is_media_domain(url), *expected, "is_media_domain({url:?})");
    }
    assert_eq!(MEDIA_DOMAINS.len(), 14);
    assert!(MEDIA_DOMAINS.contains(&"youtube.com"));
    assert!(MEDIA_DOMAINS.contains(&"x.com"));
}

#[test]
fn image_domain_classification_golden_values() {
    let cases: &[(&str, bool)] = &[
        ("https://pollinations.ai/prompt/sunset", true),
        ("https://image.pollinations.ai/prompt/sunset", true),
        ("https://foo.image.pollinations.ai/x", true),
        ("https://images.unsplash.com/photo", true),
        ("https://i.imgur.com/abcde", true),
        ("https://i.redd.it/xyz", true),
        ("https://cdn.discordapp.com/attachments/123", true),
        ("https://example.com/image.png", false),
    ];
    for (url, expected) in cases {
        assert_eq!(is_image_domain(url), *expected, "is_image_domain({url:?})");
    }
    assert_eq!(IMAGE_DOMAINS.len(), 5);
    assert!(IMAGE_DOMAINS.contains(&"i.imgur.com"));
}

#[test]
fn direct_extensions_are_a_single_constant_with_suffix_matching() {
    assert_eq!(DIRECT_EXTENSIONS.len(), 58);
    for ext in [
        ".zip", ".iso", ".dmg", ".exe", ".msi", ".pdf", ".mp4", ".mp3", ".tar", ".gz",
    ] {
        assert!(DIRECT_EXTENSIONS.contains(&ext), "missing {ext}");
    }
    assert!(has_direct_extension("https://host/pkg/file.zip"));
    assert!(has_direct_extension("https://host/file.tar.gz?x=1"));
    assert!(has_direct_extension("https://host/FILE.PDF"));
    assert!(has_direct_extension("file.ZIP"));
    assert!(!has_direct_extension("https://host/page.html"));
    assert!(!has_direct_extension("https://host/noext"));
}

// ---------------------------------------------------------------------------
// Enum string constants
// ---------------------------------------------------------------------------

#[test]
fn enum_string_values_match_python_strenum() {
    assert_eq!(TargetType::Torrent.as_str(), "torrent");
    assert_eq!(TargetType::Direct.as_str(), "direct");
    assert_eq!(TargetType::Stream.as_str(), "stream");
    assert_eq!(TargetType::Search.as_str(), "search");
    assert_eq!(TargetType::Github.as_str(), "github");
    assert_eq!(TargetType::Hf.as_str(), "huggingface");
    assert_eq!(TargetType::IpSw.as_str(), "ipsw");
    assert_eq!(TargetType::Pypi.as_str(), "pypi");
    assert_eq!(TargetType::Brew.as_str(), "brew");
    assert_eq!(WatchdogMode::V1.as_str(), "v1");
    assert_eq!(WatchdogMode::V2.as_str(), "v2");
    assert_eq!(WatchdogMode::Off.as_str(), "off");
    assert_eq!(TargetType::Direct.to_string(), "direct");
}

// ---------------------------------------------------------------------------
// Config (G01 / G02)
// ---------------------------------------------------------------------------

#[test]
fn config_schema_version_and_defaults_match_oracle() {
    assert_eq!(CONFIG_SCHEMA_VERSION, 2);
    let cfg = PDLConfig::default();
    assert_eq!(cfg.schema_version, 2);
    assert_eq!(cfg.connections, 16);
    assert_eq!(cfg.watchdog, "v2");
    assert_eq!(cfg.anti_throttle.as_deref(), Some("4MBps"));
    assert!(!cfg.bin_dir.is_empty());
    assert_eq!(cfg.output_dir, ".");
    assert_eq!(cfg.audio_codec, "aac");
    assert_eq!(cfg.video_quality, "best");
    assert_eq!(cfg.history_limit, 50);
    assert_eq!(cfg.anti_throttle_grace, 5);
    assert_eq!(cfg.stall_timeout, 30.0);
    assert_eq!(cfg.isp, "auto");
    assert_eq!(cfg.engine, "rust");
}

#[test]
fn config_validation_golden_values() {
    let mut c = PDLConfig {
        connections: 32,
        ..Default::default()
    };
    c.validate().unwrap();
    assert_eq!(c.connections, 16);

    let mut c = PDLConfig {
        anti_throttle_grace: 1,
        ..Default::default()
    };
    c.validate().unwrap();
    assert_eq!(c.anti_throttle_grace, 3);

    let mut c = PDLConfig {
        anti_throttle: Some("none".into()),
        ..Default::default()
    };
    c.validate().unwrap();
    assert_eq!(c.anti_throttle, None);

    let mut c = PDLConfig {
        connections: 0,
        ..Default::default()
    };
    assert!(
        c.validate()
            .unwrap_err()
            .to_string()
            .contains("connections must be >=")
    );
    let mut c = PDLConfig {
        audio_codec: "vorbis".into(),
        ..Default::default()
    };
    assert!(
        c.validate()
            .err()
            .unwrap()
            .to_string()
            .contains("audio_codec must be one of")
    );
    let mut c = PDLConfig {
        video_quality: "8k".into(),
        ..Default::default()
    };
    assert!(
        c.validate()
            .err()
            .unwrap()
            .to_string()
            .contains("video_quality must be one of")
    );
    let mut c = PDLConfig {
        watchdog: "v3".into(),
        ..Default::default()
    };
    assert!(
        c.validate()
            .err()
            .unwrap()
            .to_string()
            .contains("watchdog must be one of")
    );
    let mut c = PDLConfig {
        stall_timeout: -1.0,
        ..Default::default()
    };
    assert!(
        c.validate()
            .err()
            .unwrap()
            .to_string()
            .contains("cannot be negative")
    );
    let mut c = PDLConfig {
        isp: String::new(),
        ..Default::default()
    };
    assert!(
        c.validate()
            .err()
            .unwrap()
            .to_string()
            .contains("isp must be a non-empty string")
    );
}

#[test]
fn config_set_coercion_golden_values() {
    let dir = temp_dir("set");
    with_paradl_home(&dir, || {
        let mut cfg = PDLConfig::default();
        assert!(cfg.set("caffeinate", "true").0);
        assert!(cfg.caffeinate);
        assert!(cfg.set("caffeinate", "false").0);
        assert!(!cfg.caffeinate);
        assert!(cfg.set("connections", "8").0);
        assert_eq!(cfg.connections, 8);
        assert!(cfg.set("stall_timeout", "60.5").0);
        assert_eq!(cfg.stall_timeout, 60.5);
        let (ok, msg) = cfg.set("nonexistent_key", "value");
        assert!(!ok);
        assert!(msg.contains("Unknown configuration key"));
        assert!(!cfg.set("schema_version", "99").0);
    });
}

#[test]
fn config_set_invalid_value_messages_match_oracle() {
    let dir = temp_dir("set-invalid");
    with_paradl_home(&dir, || {
        let mut cfg = PDLConfig::default();
        let cases: &[(&str, &str, &str)] = &[
            (
                "caffeinate",
                "maybe",
                "Invalid boolean value for caffeinate: 'maybe' (expected true/false)",
            ),
            (
                "connections",
                "abc",
                "Invalid integer value for connections: 'abc'",
            ),
            (
                "stall_timeout",
                "xx",
                "Invalid numeric value for stall_timeout: 'xx'",
            ),
            (
                "engine",
                "bogus",
                "engine must be one of ['aria2', 'rust'], got 'bogus'",
            ),
            (
                "watchdog",
                "v9",
                "watchdog must be one of ['off', 'v1', 'v2'], got 'v9'",
            ),
            (
                "anti_throttle",
                "garbage",
                "Unparseable rate specification: 'garbage'",
            ),
            ("connections", "-2", "connections must be >= 1, got -2"),
        ];
        for (key, value, expected) in cases {
            let (ok, msg) = cfg.set(key, value);
            assert!(!ok, "set({key}, {value}) should fail");
            assert_eq!(msg, *expected, "set({key}, {value}) message");
        }
    });
}

#[test]
fn config_set_engine_syncs_active_engine() {
    let dir = temp_dir("engine-set");
    with_paradl_home(&dir, || {
        let mut cfg = PDLConfig::default();
        assert!(cfg.set("engine", "aria2").0);
        assert_eq!(cfg.engine, "aria2");
        assert_eq!(get_active_engine(), "aria2");
        assert!(cfg.set("engine", "rust").0);
        assert_eq!(cfg.engine, "rust");
        assert_eq!(get_active_engine(), "rust");
    });
}

#[test]
fn config_load_syncs_active_engine() {
    let dir = temp_dir("engine-load");
    with_paradl_home(&dir, || {
        std::fs::write(config_file(), r#"{"engine":"aria2"}"#).unwrap();
        let cfg = PDLConfig::load().unwrap();
        assert_eq!(cfg.engine, "aria2");
        assert_eq!(get_active_engine(), "aria2");
        // Restore the process-wide engine so this test does not leak state.
        set_active_engine("rust").unwrap();
    });
}

#[test]
fn config_get_effective_isp_explicit_branch() {
    let cfg = PDLConfig {
        isp: "Comcast Cable".into(),
        ..Default::default()
    };
    assert_eq!(cfg.get_effective_isp(), "Comcast Cable");
}

#[test]
fn config_save_load_and_corrupt_backup_golden_values() {
    let dir = temp_dir("saveload");
    with_paradl_home(&dir, || {
        let home = get_paradl_home();
        let cfg_file = config_file();
        assert!(cfg_file.ends_with("config.json"));
        assert!(home.ends_with("paradl-core-golden-saveload-0") || home.is_absolute());

        PDLConfig::default().save().unwrap();
        assert!(cfg_file.exists());
        let text = std::fs::read_to_string(&cfg_file).unwrap();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["connections"], 16);
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );

        let loaded = PDLConfig::load().unwrap();
        assert_eq!(loaded.connections, 16);
        assert_eq!(loaded.watchdog, "v2");
    });

    let dir2 = temp_dir("corrupt");
    with_paradl_home(&dir2, || {
        std::fs::write(config_file(), "{invalid json").unwrap();
        let cfg = PDLConfig::load().unwrap();
        assert_eq!(cfg.connections, 16);
        assert!(dir2.join("config.json.bad").exists());
    });

    let dir3 = temp_dir("migrate");
    with_paradl_home(&dir3, || {
        std::fs::write(
            config_file(),
            r#"{"schema_version":1,"connections":8,"watchdog":"v1"}"#,
        )
        .unwrap();
        let cfg = PDLConfig::load().unwrap();
        assert_eq!(cfg.schema_version, 2);
        assert_eq!(cfg.connections, 8);
        assert_eq!(cfg.watchdog, "v1");
    });

    let dir4 = temp_dir("duronto");
    with_paradl_home(&dir4, || {
        std::fs::write(config_file(), r#"{"isp":"Duronto Internet"}"#).unwrap();
        let cfg = PDLConfig::load().unwrap();
        assert_eq!(cfg.isp, "auto");
    });
}

#[test]
fn config_paths_honor_paradl_home() {
    let dir = temp_dir("paths");
    with_paradl_home(&dir, || {
        let home = get_paradl_home();
        assert_eq!(home, dir.canonicalize().unwrap());
        assert_eq!(config_file(), home.join("config.json"));
        assert_eq!(history_db_file(), home.join("history.db"));
    });
}

// ---------------------------------------------------------------------------
// Clock abstraction
// ---------------------------------------------------------------------------

#[test]
fn clock_trait_and_fake_clock() {
    let system = SystemClock;
    assert!(system.now() > std::time::UNIX_EPOCH);

    let mut fake = FakeClock::default();
    assert_eq!(
        fake.now(),
        std::time::UNIX_EPOCH + std::time::Duration::from_nanos(1_700_000_000_000_000_000)
    );
    assert_eq!(fake.monotonic(), 1000.0);
    fake.sleep(2.5);
    assert_eq!(fake.monotonic(), 1002.5);
    assert_eq!(fake.sleep_history(), &[2.5]);
    fake.jump_wall(10.0);
    assert_eq!(fake.monotonic(), 1002.5);
    assert_eq!(
        fake.now(),
        std::time::UNIX_EPOCH + std::time::Duration::from_nanos(1_700_000_012_500_000_000)
    );
}

// ---------------------------------------------------------------------------
// Model structs are constructible / serializable (reused by downstream crates)
// ---------------------------------------------------------------------------

#[test]
fn model_structs_build_with_defaults() {
    let ti = TargetInfo {
        raw: "https://host/file.zip".into(),
        target_type: TargetType::Direct,
        clean_url: "https://host/file.zip".into(),
        ..Default::default()
    };
    assert_eq!(ti.filename_hint, None);
    let tr = TransferResult {
        success: true,
        ..Default::default()
    };
    assert_eq!(tr.exit_code, 0);
    assert_eq!(tr.bytes_downloaded, 0);
}
