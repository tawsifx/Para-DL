//! Resilience demo: queue scheduling, plist generation, hooks, wake detection
//! and auth lookup. Run with:
//!
//! ```bash
//! cargo run -p paradl-resilience --example resilience_demo
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use paradl_core::ExitCode;
use paradl_resilience::auth::{
    InMemorySecretStore, MapEnv, OsEnv, Service, lookup_token,
};
use paradl_resilience::events::{
    ManualClock, ResilienceConfig, ResilienceEventSource, ResilienceEventType,
};
use paradl_resilience::hooks::{HookRunner, HookType};
use paradl_resilience::plist::{PlistConfig, PlistStart};
use paradl_resilience::queue::QueueScheduler;
use paradl_store::{QueueJob, SqliteStore, Store};

fn main() -> Result<(), paradl_core::Error> {
    let dir = tempfile::tempdir()?;

    // 1. Queue scheduling on the shared v4 `queue_jobs` table.
    let store = SqliteStore::open_at(dir.path().join("history.db"))?;
    let scheduler = QueueScheduler::new(store.open_queue()?);
    let mut job = QueueJob::new("https://example.com/file.zip", dir.path());
    job.priority = 3;
    job.when_fast_bps = 0.0; // skip the speed probe in this demo
    let added = scheduler.add_job(&job)?;
    println!("{}", added.to_jsonl());
    let (jobs, listing) = scheduler.list_jobs(None)?;
    println!("{}", listing.to_jsonl());
    let started = scheduler.start_job(&jobs[0], 16)?;
    println!("{}", started.to_jsonl());
    let recovered = scheduler.recover_crashed()?.unwrap();
    println!("{}", recovered.to_jsonl());

    // 2. Deterministic launchd plist (never loaded with launchctl).
    let plist = paradl_resilience::plist::generate_launchd_plist(
        &PlistConfig {
            label: "com.example.paradl.worker".into(),
            program_arguments: vec!["/usr/local/bin/paradl".into(), "queue".into(), "run".into()],
            working_directory: dir.path().to_path_buf(),
            environment_variables: BTreeMap::new(),
            logs_dir: dir.path().join("logs"),
            start: PlistStart::Interval(900),
        },
        Some(dir.path().to_path_buf()),
    )?;
    println!("{}", plist.emitted.to_jsonl());

    // 3. Hooks: isolated, 30 s timeout, JSON on stdin.
    let runner = HookRunner::new(dir.path().join("hooks"), Duration::from_secs(30));
    let events = runner.run_result_actions(&serde_json::json!({"target": "https://e.test"}), true)?;
    for event in &events {
        println!("{}", event.to_jsonl());
    }
    println!("# hook type discovered: {:?}", runner.find_executables(HookType::OnComplete)?);

    // 4. Resilience event source with an injected clock.
    let clock = ManualClock::new(1_700_000_000.0, 1000.0);
    let mut source = ResilienceEventSource::new(ResilienceConfig::default(), clock.clone());
    assert!(source.tick(0).is_none());
    clock.jump_wall(8.0);
    if let Some(event) = source.tick(0) {
        assert_eq!(event.event_type, ResilienceEventType::Wake);
        println!("{}", event.emitted.to_jsonl());
    }

    // 5. Auth: env lookup wins, platform keychain next, never in argv.
    let env = MapEnv::new([("HF_TOKEN".to_string(), "demo-token-1234".to_string())]);
    let found = lookup_token(Service::Hf, &env, &InMemorySecretStore::new()).unwrap();
    println!("# auth source: {:?} (exit code {})", found.source, ExitCode::Ok);
    let _ = OsEnv;

    Ok(())
}
