//! RED-first parity tests for `paradl-cli`.
//!
//! Every expected string in this file was captured from the READ-ONLY Python
//! oracle (`.venv/bin/python` calling `build_parser(PDLConfig()).print_help()` and
//! `run_cli` / `build_throttle_parser`), not invented.

use paradl_cli::*;

fn parse(args: &[&str]) -> Cli {
    let argv: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    match Cli::parse_command_line(argv).expect("parse must succeed") {
        ParseOutcome::Command(cli) => cli,
        other => panic!("expected a command, got {other:?}"),
    }
}

#[test]
fn retired_flags_are_rejected() {
    for flag in ["--http3", "--no-http3", "--accept-ranges", "--total",
                 "--install", "--info", "--hash"] {
        let argv = vec![flag.to_string(), "x".to_string()];
        let err = Cli::parse_command_line(argv).expect_err("retired flag must not parse");
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {flag}");
        assert!(err.to_string().contains("unrecognized arguments:"), "{flag}: {err}");
    }
}

#[test]
fn http3_probe_flag_is_retained() {
    let cli = parse(&["netprobe", "https://example.com/f", "--http3-probe"]);
    assert!(cli.http3_probe, "F59 flag must survive");
    assert!(!parse(&["https://example.com/f"]).http3_probe);
}

#[test]
fn help_text_is_byte_exact_oracle_output() {
    let fixture = include_str!("fixtures/help.txt");
    assert_eq!(HELP_TEXT, fixture, "embedded help constant drifted from oracle");
    assert_eq!(Cli::render_help(), fixture, "clap help output differs from oracle");
    assert!(fixture.starts_with("usage: parallel-dl [-h] [-a] [-v] [-l]"));
}

#[test]
fn help_does_not_advertise_retired_flags() {
    let usage = usage_block();
    for token in ["[--http3]", "[--no-http3]", "[--install ", "[--info ", "[--hash "] {
        assert!(!usage.contains(token), "usage block still lists {token}");
    }
    assert!(HELP_TEXT.contains("--http3-probe"), "F59 flag must stay documented");
    for line in HELP_TEXT.lines().map(str::trim_start) {
        for stale in ["--http3 ", "--no-http3 ", "--install ", "--info ", "--hash "] {
            assert!(!line.starts_with(stale), "option line still lists {stale}: {line}");
        }
    }
}

#[test]
fn docs_do_not_advertise_retired_flags() {
    let cli_readme = include_str!("../README.md");
    let net_readme = include_str!("../../paradl-net/README.md");
    for stale in [
        "the dropped adapter flags `--http3`/`--doh-endpoint`/`--accept-ranges`/`--total`",
        "The legacy `--http3` flag is kept compatible by",
    ] {
        assert!(!cli_readme.contains(stale) && !net_readme.contains(stale), "stale docs: {stale}");
    }
    assert!(cli_readme.contains("--http3-probe"), "F59 must stay documented");
    assert!(net_readme.contains("note_http3_drop"), "public compat shim must stay documented");
}

#[test]
fn version_text_is_byte_exact_oracle_output() {
    const ORACLE_VERSION: &str = "parallel-dl (paradl) v1.0.0\n";
    assert_eq!(VERSION_TEXT, ORACLE_VERSION);
    let argv = vec!["--version".to_string()];
    assert_eq!(
        Cli::parse_command_line(argv).unwrap(),
        ParseOutcome::Version
    );
}

#[test]
fn help_flag_routes_to_help_outcome() {
    let argv = vec!["--help".to_string()];
    assert_eq!(Cli::parse_command_line(argv).unwrap(), ParseOutcome::Help);
}

#[test]
fn m1_representative_flags_from_test_bug_fixes() {
    // tests/unit/test_bug_fixes.py::test_m1_cli_flags_parsing
    let cli = parse(&[
        "https://example.com/f.zip",
        "--watchdog",
        "off",
        "--no-caffeinate",
        "--no-net-watch",
    ]);
    assert_eq!(cli.watchdog.as_deref(), Some("off"));
    assert!(!cli.caffeinate, "--no-caffeinate flips the flag");
    assert!(!cli.net_watch, "--no-net-watch flips the flag");
}

#[test]
fn defaults_match_pdlconfig_oracle() {
    let cli = parse(&["https://example.com/f.zip"]);
    assert_eq!(cli.engine, Engine::Rust);
    assert_eq!(cli.connections, 16);
    assert_eq!(cli.output.as_deref(), None);
    assert_eq!(cli.anti_throttle.as_deref(), Some("4MBps"));
    assert_eq!(cli.anti_throttle_grace, 5);
    assert_eq!(cli.stall_timeout, 30.0);
    assert_eq!(cli.host, "127.0.0.1");
    assert_eq!(cli.port, 8000);
    assert_eq!(cli.history_limit, 15);
    assert_eq!(cli.interval, 900);
    assert_eq!(cli.duration, 60.0);
    assert!(cli.audio);
    assert!(cli.lossless);
    assert!(cli.thumbnail);
    assert!(cli.caffeinate);
    assert!(cli.net_watch);
    assert!(!cli.http3_probe);
}

#[test]
fn engine_choice_and_connection_clamp_input_parse() {
    let cli = parse(&["--engine", "aria2", "-c", "99", "-o", "/tmp/dl"]);
    assert_eq!(cli.engine, Engine::Aria2);
    assert_eq!(cli.connections, 99); // clamping is W1-2's job, parsing is ours
    assert_eq!(cli.output.as_deref(), Some("/tmp/dl"));
}

#[test]
fn alias_flags_are_accepted() {
    let cli = parse(&[
        "--doh-endpoint",
        "https://dns.example/dns-query",
        "--header",
        "X-Test: 1",
        "--cookies",
        "sid=1",
        "--audio-only",
        "--embed-metadata",
        "--embed-thumbnail",
        "--output-dir",
        "/tmp/out",
    ]);
    assert_eq!(cli.doh_endpoint.as_deref(), Some("https://dns.example/dns-query"));
    assert_eq!(cli.headers, vec!["X-Test: 1"]);
    assert_eq!(cli.cookies.as_deref(), Some("sid=1"));
    assert!(cli.audio_only && cli.embed_metadata && cli.embed_thumbnail);
    assert_eq!(cli.output.as_deref(), Some("/tmp/out"));

    let cli = parse(&["--no-anti-throttle", "--no-require-ac", "--failed"]);
    assert_eq!(cli.effective_anti_throttle(), None);
    assert_eq!(cli.effective_require_ac(), Some(false));
    assert_eq!(cli.effective_history_success(), Some(false));
}

#[test]
fn repeatable_and_optional_value_flags_parse() {
    let cli = parse(&[
        "--mirror", "https://m1/", "--mirror", "https://m2/", "--when-fast", "-y",
    ]);
    assert_eq!(cli.mirrors, vec!["https://m1/", "https://m2/"]);
    assert_eq!(cli.when_fast.as_deref(), Some("DEFAULT"));
    assert!(cli.yes);
}

#[test]
fn usage_errors_map_to_exit_code_two() {
    let cases: &[&[&str]] = &[
        &["install"],
        &["info"],
        &["hash"],
        &["extract"],
        &["netprobe"],
        &["bench"],
        &["history", "not-an-int"],
    ];
    for args in cases {
        let cli = parse(args);
        let err = Command::from_cli(&cli).unwrap_err();
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {args:?}");
    }
    // clap-level parse failures also map to usage errors
    let invalid: &[&[&str]] = &[
        &["--engine", "bogus"],
        &["--audio-codec", "bogus"],
        &["-c", "abc"],
        &["--bogus"],
    ];
    for args in invalid {
        let argv = args.iter().map(|s| (*s).to_string()).collect();
        let err = Cli::parse_command_line(argv).unwrap_err();
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {args:?}");
    }
}

#[test]
fn install_usage_message_is_byte_exact() {
    let err = Command::from_cli(&parse(&["install"])).unwrap_err();
    assert_eq!(err.to_string(), USAGE_INSTALL);
    assert_eq!(err.to_string(), "  Usage: pdl install <target> [--yes]\n");
}

#[test]
fn subcommand_usage_messages_are_byte_exact() {
    // Captured from the piped oracle (`run_cli`), i.e. Rich markup already stripped.
    let expectations: &[(&[&str], &str)] = &[
        (&["info"], "  Usage: pdl info <target>\n"),
        (&["hash"], "  Usage: pdl hash <file>\n"),
        (&["extract"], "  Usage: pdl extract <archive> \n"),
        (
            &["netprobe"],
            "  Usage: pdl netprobe <url> [--duration 60] [--http3-probe]\n",
        ),
        (
            &["bench"],
            "  Usage: pdl bench <url> [--watchdog v1,v2,off] [--duration 60]\n",
        ),
        (&["pip"], "  Usage: pdl pip  <package>\n"),
        (&["pypi", "install"], "  Usage: pdl pypi install <package>\n"),
        (&["brew"], "  Usage: pdl brew  <formula>\n"),
        (&["brew", "install"], "  Usage: pdl brew install <formula>\n"),
        (&["history", "abc"], "  Invalid history limit: 'abc'\n"),
    ];
    for (argv, expected) in expectations {
        let err = Command::from_cli(&parse(argv)).unwrap_err();
        assert_eq!(err.to_string(), *expected, "argv: {argv:?}");
        assert_eq!(err.exit_code(), ExitCode::Usage);
    }
}

#[test]
fn download_and_hash_subcommand_classify() {
    let cli = parse(&["https://example.com/f.zip"]);
    assert!(matches!(Command::from_cli(&cli).unwrap(), Command::Download(_)));
    let cli = parse(&["hash", "local.bin"]);
    assert!(matches!(Command::from_cli(&cli).unwrap(), Command::Hash(h) if h.file == "local.bin"));
}

#[test]
fn install_info_extract_history_clean_classify() {
    let cmd = Command::from_cli(&parse(&["install", "astral-sh/uv"])).unwrap();
    assert!(matches!(cmd, Command::Install(i) if i.target == "astral-sh/uv"));

    let cmd = Command::from_cli(&parse(&["info", "https://example.com/f.zip"])).unwrap();
    assert!(matches!(cmd, Command::Info(i) if i.target == "https://example.com/f.zip"));

    let cmd = Command::from_cli(&parse(&["hash", "f.bin"])).unwrap();
    assert!(matches!(cmd, Command::Hash(h) if h.file == "f.bin"));

    let cmd = Command::from_cli(&parse(&["extract", "a.zip", "dest"])).unwrap();
    assert!(matches!(cmd, Command::Extract(e) if e.archive == "a.zip" && e.dest_dir.as_deref() == Some("dest")));

    let cmd = Command::from_cli(&parse(&["history", "7"])).unwrap();
    assert!(matches!(cmd, Command::History(h) if h.limit == Some(7)));

    let cmd = Command::from_cli(&parse(&["clean", "/tmp"])).unwrap();
    assert!(matches!(cmd, Command::Clean(c) if c.dir.as_deref() == Some("/tmp")));
}

#[test]
fn auth_config_profiles_queue_classify_with_documented_defaults() {
    assert_eq!(
        Command::from_cli(&parse(&["auth"])).unwrap(),
        Command::Auth(AuthCommand { action: AuthAction::Status, service: None })
    );
    assert_eq!(
        Command::from_cli(&parse(&["auth", "set", "github"])).unwrap(),
        Command::Auth(AuthCommand { action: AuthAction::Set, service: Some("github".into()) })
    );
    assert_eq!(
        Command::from_cli(&parse(&["config"])).unwrap(),
        Command::Config(ConfigCommand { action: ConfigAction::Show, key: None, value: None })
    );
    assert_eq!(
        Command::from_cli(&parse(&["profiles"])).unwrap(),
        Command::Profiles(ProfilesCommand { action: ProfilesAction::List, host: None })
    );
    assert!(matches!(
        Command::from_cli(&parse(&["queue"])).unwrap(),
        Command::Queue(q) if q.action == QueueAction::List
    ));
    assert!(matches!(
        Command::from_cli(&parse(&["queue", "add", "gh:a/b"])).unwrap(),
        Command::Queue(q) if q.action == QueueAction::Add && q.args == ["gh:a/b"]
    ));
}

#[test]
fn pip_brew_doctor_server_tui_resume_throttle_classify() {
    let cmd = Command::from_cli(&parse(&["pip", "install", "requests"])).unwrap();
    assert!(matches!(cmd, Command::Pip(p) if p.action == PipAction::Install && p.packages == ["pip:requests"]));

    let cmd = Command::from_cli(&parse(&["brew", "update"])).unwrap();
    assert!(matches!(cmd, Command::Brew(b) if b.action == BrewAction::Update));

    assert_eq!(Command::from_cli(&parse(&["doctor"])).unwrap(), Command::Doctor);
    assert_eq!(Command::from_cli(&parse(&["diagnose"])).unwrap(), Command::Doctor);
    assert_eq!(Command::from_cli(&parse(&["tui"])).unwrap(), Command::Tui);

    let cmd = Command::from_cli(&parse(&["server", "--port", "8765", "--reload"])).unwrap();
    assert!(matches!(cmd, Command::Server(s) if s.port == 8765 && s.reload));

    // `--all` is a global argparse/clap flag, so it is not a positional argument:
    // `paradl-app` reads it from the Cli passed to Dispatch (faithful to dispatch.py).
    let cli = parse(&["resume", "--all"]);
    assert!(cli.all_flag);
    let cmd = Command::from_cli(&cli).unwrap();
    assert!(matches!(cmd, Command::Resume(r) if r.args.is_empty()));

    // `throttle` is intercepted before the main parser (Python dispatch.py fast path).
    let argv = vec!["throttle".into(), "full".into(), "--seed".into(), "7".into()];
    let outcome = Cli::parse_command_line(argv).unwrap();
    assert!(matches!(
        outcome,
        ParseOutcome::Throttle(t)
            if t.subcommand == ThrottleSubcommand::Full && t.seed == 7
    ));
    // A plain `throttle full` also routes through the fast path.
    let argv = vec!["throttle".into(), "full".into()];
    assert!(matches!(
        Cli::parse_command_line(argv).unwrap(),
        ParseOutcome::Throttle(t) if t.subcommand == ThrottleSubcommand::Full
    ));
}

#[test]
fn every_parity_subcommand_is_recognised() {
    let table: &[(&str, &[&str])] = &[
        ("auth", &[]),
        ("doctor", &[]),
        ("install", &["a/b"]),
        ("info", &["url"]),
        ("hash", &["f"]),
        ("extract", &["a.zip"]),
        ("history", &[]),
        ("clean", &[]),
        ("config", &[]),
        ("diagnose", &[]),
        ("netprobe", &["url"]),
        ("bench", &["url"]),
        ("profiles", &[]),
        ("queue", &[]),
        ("resume", &[]),
        ("server", &[]),
        ("tui", &[]),
        ("throttle", &[]),
        ("pip", &["install", "pkg"]),
        ("pypi", &["install", "pkg"]),
        ("brew", &["update"]),
    ];
    assert_eq!(table.len(), SUBCOMMANDS.len());
    for (name, rest) in table {
        assert!(SUBCOMMANDS.contains(name), "missing from SUBCOMMANDS: {name}");
        let mut argv = vec![*name];
        argv.extend_from_slice(rest);
        let argv_owned: Vec<String> = argv.iter().map(|s| (*s).to_string()).collect();
        if *name == "throttle" {
            assert!(
                matches!(
                    Cli::parse_command_line(argv_owned).unwrap(),
                    ParseOutcome::Throttle(_)
                ),
                "throttle did not use the fast path"
            );
            continue;
        }
        let cli = parse(&argv);
        assert!(
            !matches!(Command::from_cli(&cli).unwrap(), Command::Download(_)),
            "{name} did not classify as a subcommand"
        );
    }
}

#[test]
fn throttle_help_text_is_byte_exact_oracle_output() {
    let fixture = include_str!("fixtures/throttle_help.txt");
    assert_eq!(THROTTLE_HELP_TEXT, fixture);
    assert_eq!(ThrottleCli::render_help(), fixture);
}

#[test]
fn throttle_parser_defaults_and_choices() {
    let cli = ThrottleCli::try_parse_from(["quick"]).unwrap();
    assert_eq!(cli.subcommand, ThrottleSubcommand::Quick);
    assert_eq!(cli.seed, 42);
    assert_eq!(cli.server_port, 8765);
    assert_eq!(cli.server_udp_port, 8766);
    assert_eq!(cli.server_host, "127.0.0.1");
    assert!(cli.html);

    let cli = ThrottleCli::try_parse_from([
        "coverage", "--ipv6", "--domestic", "bdix.net", "--data-budget", "3GB",
    ])
    .unwrap();
    assert_eq!(cli.subcommand, ThrottleSubcommand::Coverage);
    assert!(cli.ipv6 && !cli.ipv4);
    assert_eq!(cli.domestic_urls, vec!["bdix.net"]);
    assert_eq!(cli.data_budget.as_deref(), Some("3GB"));
}

#[test]
fn dispatch_trait_is_a_usable_seam_without_command_bodies() {
    let cli = parse(&["doctor", "--json"]);
    let cmd = Command::from_cli(&cli).unwrap();
    let mut noop = NoopDispatch;
    assert_eq!(dispatch(&mut noop, &cmd, &cli), ExitCode::Ok);
    assert_eq!(noop.handle(&cmd, &cli), ExitCode::Ok);
}

#[test]
fn cli_env_var_contract_is_documented() {
    assert!(CLI_ENV_VARS.contains(&"HF_TOKEN"));
    assert!(CLI_ENV_VARS.contains(&"GITHUB_TOKEN"));
    assert!(CLI_ENV_VARS.contains(&"PARADL_HOME"));
    assert!(CLI_ENV_VARS.contains(&"PYO3_PYTHON"));
}

// ---- Round-1 fix: byte-exact argparse error output -------------------------

fn parse_error(args: &[&str]) -> Error {
    let argv = args.iter().map(|s| (*s).to_string()).collect();
    Cli::parse_command_line(argv).unwrap_err()
}

#[test]
fn argparse_error_output_is_byte_exact_for_reviewed_cases() {
    let cases: &[(&[&str], &str)] = &[
        (&["--bogus"], include_str!("fixtures/error_unknown_arg.txt")),
        (&["--engine", "bogus"], include_str!("fixtures/error_engine_bogus.txt")),
        (&["-c", "abc"], include_str!("fixtures/error_connections_abc.txt")),
    ];
    for (argv, golden) in cases {
        let err = parse_error(argv);
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {argv:?}");
        assert_eq!(err.to_string(), *golden, "argv: {argv:?}");
        assert!(err.to_string().starts_with("usage: parallel-dl [-h]"));
        assert!(err.to_string().contains("parallel-dl: error: "));
    }
}

#[test]
fn argparse_error_output_is_byte_exact_for_additional_choices_and_types() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["--audio-codec", "foo"],
            "parallel-dl: error: argument --audio-codec: invalid choice: 'foo' (choose from lossless, native, mp3, flac, opus, wav, aac)\n",
        ),
        (
            &["-Q", "8k"],
            "parallel-dl: error: argument -Q/--quality: invalid choice: '8k' (choose from best, 4k, 1440p, 1080p, 720p, 480p)\n",
        ),
        (
            &["--type", "bogus"],
            "parallel-dl: error: argument --type: invalid choice: 'bogus' (choose from direct, stream, search, github, huggingface, ipsw, pypi, brew, torrent)\n",
        ),
        (
            &["--watchdog"],
            "parallel-dl: error: argument --watchdog: expected one argument\n",
        ),
        (
            &["--port", "x"],
            "parallel-dl: error: argument --port: invalid int value: 'x'\n",
        ),
        (
            &["--bogus", "--also"],
            "parallel-dl: error: unrecognized arguments: --bogus --also\n",
        ),
    ];
    let usage = usage_block();
    assert_eq!(usage.len(), 1850);
    for (argv, message) in cases {
        let expected = format!("{usage}{message}");
        assert_eq!(parse_error(argv).to_string(), expected, "argv: {argv:?}");
    }
}

#[test]
fn help_and_version_precedence_follow_oracle_left_to_right() {
    // Captured from Python: all three exit 2; an earlier bad arg beats the action.
    let cases: &[(&[&str], &str)] = &[
        (
            &["--engine", "bogus", "--version"],
            "parallel-dl: error: argument --engine: invalid choice: 'bogus' (choose from aria2, rust)\n",
        ),
        (
            &["--engine", "bogus", "--help"],
            "parallel-dl: error: argument --engine: invalid choice: 'bogus' (choose from aria2, rust)\n",
        ),
        (
            &["--host", "--version"],
            "parallel-dl: error: argument --host: expected one argument\n",
        ),
    ];
    for (argv, message) in cases {
        let err = parse_error(argv);
        assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {argv:?}");
        assert_eq!(err.to_string(), format!("{}{message}", usage_block()), "argv: {argv:?}");
    }

    // The exact single-arg fast path still succeeds, as does help-first.
    let argv = vec!["--version".to_string()];
    assert_eq!(Cli::parse_command_line(argv).unwrap(), ParseOutcome::Version);
    let argv = vec!["--help".to_string()];
    assert_eq!(Cli::parse_command_line(argv).unwrap(), ParseOutcome::Help);
}

#[test]
fn version_first_wins_with_byte_exact_version_bytes() {
    // Oracle: `action="version"` exits at the moment --version is encountered.
    let cases: &[&[&str]] = &[
        &["--version", "--engine", "bogus"],
        &["--version", "-c", "abc"],
        &["--version", "--help"],
        &["--version", "--engine", "aria2"],
    ];
    for argv in cases {
        let argv_owned = argv.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            Cli::parse_command_line(argv_owned).unwrap(),
            ParseOutcome::Version,
            "argv: {argv:?}"
        );
    }

    // clap's Version action itself renders the exact oracle bytes (no auto-format).
    let err = Cli::try_parse_from(["--version"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    assert_eq!(err.to_string(), VERSION_TEXT);
    assert_eq!(VERSION_TEXT, "parallel-dl (paradl) v1.0.0\n");
}

#[test]
fn clap_short_v_alias_is_not_invented() {
    // Python argparse has no -V; it must remain an unrecognized argument (exit 2).
    let err = parse_error(&["-V"]);
    assert_eq!(err.exit_code(), ExitCode::Usage);
    assert_eq!(
        err.to_string(),
        format!("{}parallel-dl: error: unrecognized arguments: -V\n", usage_block())
    );
}

#[test]
fn unknown_action_messages_are_byte_exact() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["config", "bogus"],
            "  Error: Unknown config action: 'bogus' (expected show, set, reset, or reset-hosts)\n",
        ),
        (
            &["queue", "x"],
            "  Unknown queue action: 'x' (expected add, list, remove, pause, resume, run, or \nlaunchd-plist)\n",
        ),
        (
            &["auth", "x"],
            "  Unknown auth action 'x'. Use: pdl auth set|status|remove\n",
        ),
    ];
    for (argv, expected) in cases {
        let err = Command::from_cli(&parse(argv)).unwrap_err();
        assert_eq!(err.to_string(), *expected, "argv: {argv:?}");
        assert_eq!(err.exit_code(), ExitCode::Usage);
    }
    assert!(USAGE_CONFIG_UNKNOWN.contains("{action}"));
    assert!(USAGE_QUEUE_UNKNOWN.contains("{action}"));
    assert!(USAGE_AUTH_UNKNOWN.contains("{action}"));
}

#[test]
fn unknown_profiles_token_is_a_hostname_not_a_usage_error() {
    // Python: `pdl profiles x` lists profiles for host `x` and exits 0.
    let cli = parse(&["profiles", "example.com"]);
    let cmd = Command::from_cli(&cli).unwrap();
    assert!(matches!(
        cmd,
        Command::Profiles(p)
            if p.action == ProfilesAction::List && p.host.as_deref() == Some("example.com")
    ));
}
