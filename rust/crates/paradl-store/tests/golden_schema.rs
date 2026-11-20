//! Golden contract tests for `paradl-store`, captured from the Python oracle.
//!
//! Every literal asserted here was produced by running
//! `tests/integration/test_sqlite_guard.py` and by dumping the schema of a
//! freshly-migrated `$PARADL_HOME/history.db` with the oracle's
//! `HistoryManager` / `ThrottleStore`. The captured dump lives in
//! `tests/fixtures/golden_schema.json`.

use std::path::Path;

use paradl_store::{HISTORY_TABLES, SCHEMA_VERSION, SqliteStore, Store, THROTTLE_TABLES, TransferRecord};
use rusqlite::Connection;
use serde_json::{Value, json};

fn golden() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden_schema.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing golden fixture {}: {e}", path.display()));
    serde_json::from_str(&text).expect("invalid golden fixture JSON")
}

/// Mirror the oracle's `_dump_schema`: sqlite_master, table_info and pragmas.
fn dump_schema(db: &Path) -> Value {
    let conn = Connection::open(db).expect("open dump connection");
    let master: Vec<Value> = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .expect("prepare sqlite_master")
        .query_map([], |row| {
            Ok(json!([
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ]))
        })
        .expect("query sqlite_master")
        .map(|r| r.expect("master row"))
        .collect();

    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .expect("prepare tables")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query tables")
        .map(|r| r.expect("table row"))
        .collect();

    let mut table_info = serde_json::Map::new();
    for table in &tables {
        let sql = format!("PRAGMA table_info(\"{table}\")");
        let cols: Vec<Value> = conn
            .prepare(&sql)
            .unwrap_or_else(|e| panic!("pragma table_info({table}): {e}"))
            .query_map([], |row| {
                Ok(json!([
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                ]))
            })
            .expect("query table_info")
            .map(|r| r.expect("column row"))
            .collect();
        table_info.insert(table.clone(), Value::Array(cols));
    }

    let mut pragmas = serde_json::Map::new();
    for key in ["journal_mode", "synchronous", "user_version"] {
        let sql = format!("PRAGMA {key}");
        let value: Value = conn
            .query_row(&sql, [], |row| match row.get_ref(0)? {
                rusqlite::types::ValueRef::Integer(v) => Ok(json!(v)),
                rusqlite::types::ValueRef::Text(v) => {
                    Ok(json!(std::str::from_utf8(v).expect("utf8")))
                }
                other => panic!("unexpected pragma {key}: {other:?}"),
            })
            .unwrap_or_else(|e| panic!("pragma {key}: {e}"));
        pragmas.insert(key.to_string(), value);
    }

    let schema_meta: Vec<Value> = conn
        .prepare("SELECT key, value FROM schema_meta ORDER BY key")
        .expect("prepare schema_meta")
        .query_map([], |row| {
            Ok(json!([
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ]))
        })
        .expect("query schema_meta")
        .map(|r| r.expect("meta row"))
        .collect();

    json!({
        "master": master,
        "tables": tables,
        "table_info": Value::Object(table_info),
        "pragmas": Value::Object(pragmas),
        "schema_meta": schema_meta,
    })
}

/// D01/D02/D03/D06/D16: schema, WAL, pragmas and all 10 tables are byte-exact.
#[test]
fn schema_matches_python_oracle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("history.db");
    let store = SqliteStore::open_at(&db).expect("open store");
    drop(store.open_queue().expect("open queue"));

    let actual = dump_schema(&db);
    let expected = golden();
    assert_eq!(SCHEMA_VERSION, "4");
    assert_eq!(actual["master"], expected["master"]);
    assert_eq!(actual["table_info"], expected["table_info"]);
    assert_eq!(actual["schema_meta"], expected["schema_meta"]);
    assert_eq!(actual["pragmas"], expected["pragmas"]);
    assert_eq!(actual["pragmas"]["journal_mode"], json!("wal"));
    assert_eq!(actual["pragmas"]["synchronous"], json!(2));
    assert_eq!(actual["pragmas"]["user_version"], json!(0));

    for table in HISTORY_TABLES.iter().chain(THROTTLE_TABLES.iter()) {
        assert!(
            actual["tables"]
                .as_array()
                .expect("tables array")
                .iter()
                .any(|t| t.as_str() == Some(*table)),
            "missing table {table}"
        );
    }

    // WAL/synchronous are per-connection: a managed connection reapplies them.
    let (journal, sync, version): (String, i64, String) = {
        let store = SqliteStore::open_at(&db).expect("reopen");
        store
            .with_connection(|c| {
                Ok((
                    c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
                    c.query_row("PRAGMA synchronous", [], |r| r.get(0))?,
                    c.query_row(
                        "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                        [],
                        |r| r.get(0),
                    )?,
                ))
            })
            .expect("managed pragmas")
    };
    assert_eq!(journal, "wal");
    assert_eq!(sync, 1);
    assert_eq!(version, "4");
}

/// D07: transfer records round-trip with the oracle's exact field values.
#[test]
fn record_transfer_and_recent_match_oracle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("history.db");
    let store = SqliteStore::open_at(&db)
        .expect("open store")
        .with_history_limit(2);

    let a = TransferRecord::new("https://a.example/x", "direct", "/tmp/a.bin", 10, 1.256, true, false, None);
    let b = TransferRecord::new("https://b.example/y", "stream", "/tmp/b.bin", 20, 2.5, false, true, Some("abc".into()));
    let c = TransferRecord::new("https://c.example/z", "direct", "/tmp/c.bin", 30, 3.0, true, false, None);
    store.record_transfer(&a).expect("record a");
    store.record_transfer(&b).expect("record b");
    store.record_transfer(&c).expect("record c");

    let recent = store.recent(10).expect("recent");
    assert_eq!(recent.len(), 2, "history_limit cap must keep newest 2");
    assert_eq!(recent[0].target, "https://c.example/z");
    assert_eq!(recent[1].target, "https://b.example/y");
    assert_eq!(recent[1].filename, "b.bin");
    assert_eq!(recent[1].path, "/tmp/b.bin");
    assert_eq!(recent[1].size, 20);
    assert_eq!(recent[1].duration, 2.5);
    assert!(!recent[1].success);
    assert!(recent[1].adb);
    assert_eq!(recent[1].sha256.as_deref(), Some("abc"));

    // Python rounds duration to two decimals when recording.
    let one = store.recent(1).expect("recent 1");
    assert_eq!(one[0].duration, 3.0);
    store.record_transfer(&a).expect("record a again");
    let again = store.recent(1).expect("recent 1");
    assert_eq!(again[0].duration, 1.26);
    assert_eq!(again[0].filename, "a.bin");
}

/// D20: migrations add columns without dropping a single row.
#[test]
fn migration_from_v3_preserves_data_and_matches_golden_schema() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("history.db");
    {
        let conn = Connection::open(&db).expect("create v3 db");
        conn.execute_batch(include_str!("fixtures/v3_schema.sql"))
            .expect("seed v3 schema");
    }

    SqliteStore::open_at(&db).expect("migrate v3 -> v4");

    let check = Connection::open(&db).expect("reopen migrated");
    let rows = |sql: &str| -> Vec<Value> {
        check
            .prepare(sql)
            .expect("prepare")
            .query_map([], |row| {
                let count = row.as_ref().column_count();
                let mut values = Vec::new();
                for i in 0..count {
                    let value = match row.get_ref(i)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(v) => json!(v),
                        rusqlite::types::ValueRef::Real(v) => json!(v),
                        rusqlite::types::ValueRef::Text(v) => {
                            json!(std::str::from_utf8(v).expect("utf8"))
                        }
                        other => panic!("unexpected value: {other:?}"),
                    };
                    values.push(value);
                }
                Ok(Value::Array(values))
            })
            .expect("query")
            .map(|r| r.expect("row"))
            .collect()
    };
    let expected = golden();
    assert_eq!(json!(rows("SELECT * FROM downloads ORDER BY id")), expected["v3_migration_downloads"]);
    assert_eq!(json!(rows("SELECT * FROM host_strategies ORDER BY host")), expected["v3_migration_host_strategies"]);
    assert_eq!(json!(rows("SELECT * FROM learned_profiles ORDER BY host")), expected["v3_migration_learned_profiles"]);
    assert_eq!(json!(rows("SELECT * FROM queue_jobs ORDER BY id")), expected["v3_migration_queue_jobs"]);
    let version: String = check
        .query_row("SELECT value FROM schema_meta WHERE key = 'schema_version'", [], |r| r.get(0))
        .expect("schema_version");
    assert_eq!(version, "4");
    drop(check);

    // The migration DDL itself is byte-exact against the oracle's v3 -> v4 path
    // (SQLite's ALTER TABLE rewriting is deliberately preserved, not normalized).
    let actual = dump_schema(&db);
    assert_eq!(actual["master"], golden()["v3_migration_master"]);
    assert_eq!(actual["table_info"], golden()["v3_migration_table_info"]);
}

/// D19: legacy history.json is imported once, read-only, in reverse order.
#[test]
fn legacy_json_migration_matches_oracle() {
    let dir = tempfile::tempdir().expect("temp dir");
    let legacy = dir.path().join("history.json");
    std::fs::write(
        &legacy,
        r#"[
  {"timestamp":"2023-01-01 00:00:00","target":"https://old.example/a","type":"direct","filename":"a.bin","path":"/tmp/a.bin","size":10,"duration":1.25,"success":true,"adb":false,"sha256":null},
  {"timestamp":"2023-01-02 00:00:00","target":"https://old.example/b","type":"stream","filename":"b.bin","path":"/tmp/b.bin","size":20,"duration":2.5,"success":false,"adb":true,"sha256":"deadbeef"}
]"#,
    )
    .expect("write legacy history.json");

    let db = dir.path().join("history.db");
    SqliteStore::open_at(&db).expect("open store with legacy json");

    let conn = Connection::open(&db).expect("reopen");
    let rows: Vec<Value> = conn
        .prepare("SELECT * FROM downloads ORDER BY id")
        .expect("prepare downloads")
        .query_map([], |row| {
            Ok(json!([
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, f64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
            ]))
        })
        .expect("query downloads")
        .map(|r| r.expect("download row"))
        .collect();
    assert_eq!(json!(rows), golden()["legacy_downloads"]);
    let meta: Vec<Value> = conn
        .prepare("SELECT key, value FROM schema_meta ORDER BY key")
        .expect("prepare meta")
        .query_map([], |row| Ok(json!([row.get::<_, String>(0)?, row.get::<_, String>(1)?])))
        .expect("query meta")
        .map(|r| r.expect("meta row"))
        .collect();
    assert_eq!(json!(meta), golden()["legacy_meta"]);
    drop(conn);
    assert!(legacy.exists(), "history.json must never be deleted");
}

/// D18: a corrupt DB is rotated to `.bad` and sidecars are removed.
#[test]
fn corrupt_db_is_rotated_and_rebuilt() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("history.db");
    std::fs::write(&db, b"not a sqlite database at all").expect("write corrupt db");
    std::fs::write(
        format!("{}-wal", db.display()),
        b"stale wal",
    )
    .expect("write stale wal");

    let store = SqliteStore::open_at(&db).expect("recover from corruption");
    assert!(dir.path().join("history.db.bad").exists());
    assert!(!Path::new(&format!("{}-wal", db.display())).exists());
    assert_eq!(store.recent(5).expect("recent").len(), 0);
}

/// D04/D12: shared queue_jobs round-trips with the oracle's exact defaults.
#[test]
fn queue_round_trip_matches_oracle_defaults() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = SqliteStore::open_at(dir.path().join("history.db")).expect("open store");
    let queue = store.open_queue().expect("open queue");

    let mut job = paradl_store::QueueJob::new("  https://q.example/x  ", "/tmp/out");
    job.connections = 99; // must clamp to 16
    let saved = queue.add_job(&job).expect("add job");
    assert_eq!(saved.status, "queued");
    assert_eq!(saved.target_type, "direct");
    assert_eq!(saved.target, "https://q.example/x");
    assert_eq!(saved.connections, 16);
    assert!(saved.require_ac);
    assert!(!saved.pause_on_low_power);
    assert_eq!(saved.category, "");
    assert_eq!(saved.bytes_downloaded, 0);
    assert!(saved.error.is_none());
    assert!(saved.mirrors.is_empty());
    assert!(saved.options.as_object().expect("options object").is_empty());
    assert_eq!(saved.created_at, saved.updated_at);

    let row: (String, String) = {
        let conn = Connection::open(dir.path().join("history.db")).expect("reopen");
        conn.query_row(
            "SELECT mirrors_json, options_json FROM queue_jobs WHERE id = ?1",
            [saved.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("queue row")
    };
    assert_eq!(row, ("[]".to_string(), "{}".to_string()));

    queue
        .update_job_status(saved.id, "running", Some(123), None, Some("x.bin"))
        .expect("mark running");
    let running = queue.get_job(saved.id).expect("get job").expect("job exists");
    assert_eq!(running.status, "running");
    assert_eq!(running.bytes_downloaded, 123);
    assert_eq!(running.filename.as_deref(), Some("x.bin"));

    assert_eq!(queue.pause_jobs(&[], true).expect("pause all"), 1);
    assert_eq!(queue.get_job(saved.id).expect("get").expect("exists").status, "paused");
    assert_eq!(queue.resume_jobs(&[], true).expect("resume all"), 1);
    assert_eq!(queue.get_job(saved.id).expect("get").expect("exists").status, "queued");
    assert_eq!(queue.recover_crashed_jobs().expect("recover"), 0);
    assert_eq!(queue.list_jobs(None).expect("list").len(), 1);
    assert_eq!(queue.remove_jobs(&[], true).expect("remove all"), 1);
    assert!(queue.list_jobs(None).expect("list").is_empty());
}
