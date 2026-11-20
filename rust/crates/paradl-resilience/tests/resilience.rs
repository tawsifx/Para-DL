//! Golden and behavioral contracts for the resilience crate.
//!
//! Every event produced here is a typed `paradl_events` value serialized by the
//! single emitter — this crate never calls `json.dumps` / writes stdout.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use paradl_resilience::auth::{
    EnvLookup, InMemorySecretStore, Service, TokenSource, keychain_load_spec,
    keychain_store_spec, lookup_token,
};
use paradl_resilience::events::{
    ManualClock, ResilienceConfig, ResilienceEventSource, ResilienceEventType,
};
use paradl_resilience::hooks::{HookRunner, HookType};
use paradl_resilience::plist::{PlistConfig, PlistStart, generate_launchd_plist, parse_start_at_time};
use paradl_resilience::queue::QueueScheduler;
use paradl_resilience::Emitted;
use paradl_store::{QueueJob, SqliteStore, Store};
use serde_json::{Value, json};

fn scheduler() -> (tempfile::TempDir, QueueScheduler) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open_at(dir.path().join("history.db")).unwrap();
    let queue = store.open_queue().unwrap();
    (dir, QueueScheduler::new(queue))
}

fn field_set(line: &str) -> std::collections::BTreeSet<String> {
    let value: Value = serde_json::from_str(line.trim_end()).unwrap();
    value.as_object().unwrap().keys().cloned().collect()
}

struct MapEnv(HashMap<String, String>);

impl EnvLookup for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

#[test]
fn queue_events_match_golden_field_sets() {
    let (_dir, scheduler) = scheduler();
    let mut job = QueueJob::new("https://example.com/f", "/tmp/out");
    job.priority = 5;
    let added = scheduler.add_job(&job).unwrap();
    assert_eq!(added.name(), "queue_job_added");
    assert!(field_set(&added.to_jsonl()).contains("when_fast_bps"));

    let (jobs, listed) = scheduler.list_jobs(None).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(listed.name(), "queue_list");
    assert_eq!(field_set(&listed.to_jsonl()), ["event", "timestamp", "jobs"].into_iter().map(String::from).collect());

    let started = scheduler.start_job(&jobs[0], 16).unwrap();
    assert_eq!(started.name(), "queue_job_started");
    assert_eq!(
        field_set(&started.to_jsonl()),
        ["event", "timestamp", "job_id", "target", "priority", "connections"]
            .into_iter()
            .map(String::from)
            .collect()
    );

    let deferred = scheduler.defer(&jobs[0], "when_fast", Some("12:00".into()), Some(100.0), Some(200.0)).unwrap();
    assert_eq!(deferred.name(), "queue_job_deferred");
    assert_eq!(
        field_set(&deferred.to_jsonl()),
        ["event", "timestamp", "job_id", "reason", "start_at", "measured_bps", "threshold_bps"]
            .into_iter()
            .map(String::from)
            .collect()
    );

    let probe = scheduler.when_fast_gate("https://example.com/f", 200.0, 2, &|_| 100.0).unwrap();
    assert!(!probe.passed);
    assert_eq!(probe.events.len(), 2);
    assert_eq!(probe.events[0].name(), "when_fast_probe");
    assert_eq!(
        field_set(&probe.events[0].to_jsonl()),
        ["event", "timestamp", "url", "attempt", "measured_bps", "threshold_bps", "passed"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn queue_orders_by_priority_and_recovers_crashes() {
    let (_dir, scheduler) = scheduler();
    for (target, priority) in [("low", 0), ("high", 9), ("mid", 4)] {
        let mut job = QueueJob::new(target, "/tmp/out");
        job.priority = priority;
        scheduler.add_job(&job).unwrap();
    }
    let (jobs, _) = scheduler.list_jobs(None).unwrap();
    let targets: Vec<_> = jobs.iter().map(|job| job.target.as_str()).collect();
    assert_eq!(targets, ["high", "mid", "low"]);

    scheduler.start_job(&jobs[0], 16).unwrap();
    let recovered = scheduler.recover_crashed().unwrap().expect("one crash recovered");
    assert_eq!(recovered.name(), "queue_crash_recovered");
    assert!(recovered.to_jsonl().contains("\"recovered_count\": 1"));
    let (jobs, _) = scheduler.list_jobs(None).unwrap();
    assert!(jobs.iter().all(|job| job.status == "queued"));
}

#[test]
fn plist_is_deterministic_sorted_and_emits_typed_event() {
    let dir = tempfile::tempdir().unwrap();
    let config = PlistConfig {
        label: "com.example.worker".into(),
        program_arguments: vec!["/usr/local/bin/paradl".into(), "queue".into(), "run".into()],
        working_directory: dir.path().to_path_buf(),
        environment_variables: [("PARADL_HOME".to_string(), "/home/u/.paradl".to_string())]
            .into_iter()
            .collect(),
        logs_dir: dir.path().join("logs"),
        start: PlistStart::Interval(900),
    };
    let out = generate_launchd_plist(&config, Some(dir.path().to_path_buf())).unwrap();
    assert_eq!(out.lint, "OK");
    assert_eq!(out.emitted.name(), "launchd_plist_generated");
    assert!(out.xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(out.xml.contains("<key>Label</key>"));
    assert!(out.xml.contains("<integer>900</integer>"));
    let keys_in_order: Vec<_> = out
        .xml
        .lines()
        .filter_map(|line| line.strip_prefix("\t<key>"))
        .map(|line| line.trim_end_matches("</key>"))
        .collect();
    let mut sorted = keys_in_order.clone();
    sorted.sort_unstable();
    assert_eq!(keys_in_order, sorted, "plistlib sort_keys=True");
    assert_eq!(parse_start_at_time("12:34"), Some((12, 34)));
    assert_eq!(parse_start_at_time("nope"), None);

    let calendar = PlistConfig {
        start: PlistStart::Calendar { hour: 6, minute: 30 },
        ..config
    };
    let out = generate_launchd_plist(&calendar, Some(dir.path().join("cal.plist"))).unwrap();
    assert!(out.xml.contains("<key>StartCalendarInterval</key>"));
    assert!(out.xml.contains("<integer>30</integer>"));
}

#[cfg(unix)]
#[test]
fn hooks_run_with_json_stdin_timeout_and_typed_events() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let hooks = dir.path().join("hooks");
    let on_ok = hooks.join("on_complete");
    let on_fail = hooks.join("on_fail");
    std::fs::create_dir_all(&on_ok).unwrap();
    std::fs::create_dir_all(&on_fail).unwrap();

    let ok_script = on_ok.join("10-ok");
    std::fs::write(&ok_script, "#!/bin/sh\ncat > /dev/null\necho done >&2\nexit 0\n").unwrap();
    std::fs::set_permissions(&ok_script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fail_script = on_fail.join("20-fail");
    std::fs::write(&fail_script, "#!/bin/sh\ncat > /dev/null\necho bad >&2\nexit 3\n").unwrap();
    std::fs::set_permissions(&fail_script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let hang_script = on_fail.join("30-hang");
    std::fs::write(&hang_script, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&hang_script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let runner = HookRunner::new(hooks, Duration::from_millis(250));
    assert_eq!(runner.find_executables(HookType::OnComplete).unwrap().len(), 1);

    let events = runner
        .run_result_actions(&json!({"target": "https://e.test/f"}), true)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name(), "hook_executed");
    assert_eq!(
        field_set(&events[0].to_jsonl()),
        ["event", "timestamp", "hook", "success"].into_iter().map(String::from).collect()
    );

    let events = runner
        .run_result_actions(&json!({"target": "https://e.test/f"}), false)
        .unwrap();
    let names: Vec<_> = events.iter().map(Emitted::name).collect();
    assert!(names.contains(&"hook_failed"));
    assert!(names.contains(&"hook_timeout"));
    for event in &events {
        let line = event.to_jsonl();
        assert!(!line.contains("secret"));
        assert!(line.contains("\"event\": \"hook_"));
    }
}

#[test]
fn resilience_source_detects_wake_net_change_and_stall() {
    let clock = ManualClock::new(1_700_000_000.0, 1000.0);
    let config = ResilienceConfig {
        backend: "test".into(),
        ..ResilienceConfig::default()
    };
    let mut source = ResilienceEventSource::new(config, clock.clone());
    assert!(source.tick(0).is_none());

    clock.jump_wall(10.0);
    let wake = source.tick(0).expect("wake fires");
    assert_eq!(wake.event_type, ResilienceEventType::Wake);
    assert_eq!(wake.reason, "System wake detected (wall-clock jumped +10.0s over monotonic)");
    assert_eq!(wake.emitted.name(), "resilience_event");
    assert!(source.tick(0).is_none(), "debounce");

    source.inject_net_change("scutil: changed key [0] = State:/Network/Global/IPv4");
    clock.advance_mono(3.0);
    let net = source.tick(0).expect("net change fires");
    assert_eq!(net.event_type, ResilienceEventType::NetChange);
    assert_eq!(
        net.reason,
        "Network change detected via test (1 notification(s))"
    );

    clock.advance_mono(31.0);
    let stall = source.tick(0).expect("stall fires");
    assert_eq!(stall.event_type, ResilienceEventType::Stall);
    assert_eq!(
        stall.reason,
        "Transfer stalled: no byte progress for 31.0s (stall timeout 30.0s)"
    );
    assert_eq!(
        field_set(&stall.emitted.to_jsonl()),
        ["event", "timestamp", "event_type", "reason", "detail"]
            .into_iter()
            .map(String::from)
            .collect()
    );
}

#[test]
fn auth_lookup_precedence_and_no_secret_in_argv() {
    let env = MapEnv(HashMap::from([("HF_TOKEN".into(), "env-token-1234".into())]));
    let store = InMemorySecretStore::new();
    store.put(Service::Github, "github", "keychain-token-5678").unwrap();

    let hf = lookup_token(Service::Hf, &env, &store).unwrap();
    assert_eq!(hf.token, "env-token-1234");
    assert_eq!(hf.source, TokenSource::Env);

    let gh = lookup_token(Service::Github, &env, &store).unwrap();
    assert_eq!(gh.token, "keychain-token-5678");
    assert_eq!(gh.source, TokenSource::Keychain);

    let empty = MapEnv(HashMap::new());
    assert!(lookup_token(Service::Hf, &empty, &store).is_none());

    let spec = keychain_store_spec("paradl", "hf", "secret-token-9999");
    assert!(!spec.argv.join(" ").contains("secret-token-9999"));
    assert!(spec.stdin_payload.contains("secret-token-9999"));
    let load = keychain_load_spec("paradl", "hf");
    assert!(!load.iter().any(|arg| arg.contains("secret")));
    assert!(load.contains(&"find-generic-password".to_string()));
}

#[test]
fn resilience_never_emits_raw_json_dumps() {
    // Every typed event goes through paradl_events::emit_jsonl (spaces after
    // separators). A raw json.dumps would produce compact output.
    let (_dir, scheduler) = scheduler();
    let job = QueueJob::new("https://e.test", "/tmp");
    let line = scheduler.add_job(&job).unwrap().to_jsonl();
    assert!(line.contains("\": \""), "must use json.dumps separators: {line}");
    assert!(line.contains(", \""), "must use json.dumps separators: {line}");
    assert!(line.ends_with('\n'));
}

fn _assert_send_sync<T: Send + Sync>() {}
#[test]
fn public_types_are_thread_safe() {
    _assert_send_sync::<Emitted>();
    let _: Result<(), paradl_core::Error> = Ok(());
    let _ = Path::new("/tmp");
}
