//! Runnable smoke test: open a fresh v4 database, record a transfer and a
//! queue job, then print the resulting schema state.
//!
//! Run with `cargo run -p paradl-store --example sqlite_roundtrip`.

use paradl_store::{QueueJob, SqliteStore, Store, TransferRecord};

fn main() -> Result<(), paradl_core::Error> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("history.db");
    let store = SqliteStore::open_at(&db)?;

    store.record_transfer(&TransferRecord::new(
        "https://example.com/archive.zip",
        "direct",
        "/tmp/archive.zip",
        1_024,
        1.25,
        true,
        false,
        Some("deadbeef".to_string()),
    ))?;

    let recent = store.recent(5)?;
    println!("recent transfers: {}", recent.len());
    for record in &recent {
        println!(
            "  {} {} {} bytes {:.2}s sha256={:?}",
            record.target, record.filename, record.size, record.duration, record.sha256
        );
    }

    let queue = store.open_queue()?;
    let job = queue.add_job(&QueueJob::new("https://example.com/archive.zip", "/tmp"))?;
    println!("queue job #{}: {} {}", job.id, job.target, job.status);

    let (version, tables): (String, i64) = store.with_connection(|conn| {
        Ok((
            conn.query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )?,
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )?,
        ))
    })?;
    println!("schema v{version} with {tables} tables at {}", store.path().display());
    Ok(())
}
