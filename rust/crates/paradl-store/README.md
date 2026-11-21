# paradl-store

SQLite persistence for the paraDL framework: the shipped `history.db` **schema
v4**, WAL mode, additive migrations, the process-wide device+inode schema
guard, the shared `queue_jobs` table, and the one-time read-only
`history.json` import.

All ten tables live in one file: seven history-owned (`schema_meta`,
`downloads`, `host_strategies`, `learned_profiles`, `http_cache`, `host_caps`,
`queue_jobs`) and three throttle-owned (`throttle_samples`, `throttle_runs`,
`asn_cache`). Every error is a `paradl_core::Error` and therefore carries the
full `ExitCode` mapping (`0/1/2/3/4/130`).

## Public API

- `trait Store` — `record_transfer`, `recent(limit)`, `open_queue`.
- `SqliteStore` — `open()` / `open_at(path)`, `with_history_limit(n)`,
  `with_connection(|conn| …)`, `path()`.
- `TransferRecord`, `QueueJob`, `Queue` (queue CRUD).

No feature flags.

## Example

```bash
cargo run -p paradl-store --example sqlite_roundtrip
```

The example opens a temporary `history.db`, records a transfer, lists it,
writes a queue job and prints the resulting schema metadata.

## Oracle parity

Every literal in `tests/golden_schema.rs` was captured from the Python oracle
(`tests/integration/test_sqlite_guard.py`, `src/paradl/storage/history.py`,
`src/paradl/throttle/store.py`, `src/paradl/resilience/queue.py`). The captured
schema dump lives in `tests/fixtures/golden_schema.json`; the v3-to-v4 fixture
is `tests/fixtures/v3_schema.sql`. Migrations never drop data: they are
`CREATE TABLE IF NOT EXISTS` plus `ALTER TABLE … ADD COLUMN`, preserving the
exact `sqlite_master.sql` text produced by the oracle.
