# paradl-api

Opt-in axum HTTP server for paraDL: the seven REST endpoints of
`src/paradl/api/server.py`, pydantic-compatible serde validation, and a task
store/runner abstraction. The Python application becomes Rust except media;
this crate is the API surface of that migration (W3-3).

## Public surface

- `AppState` / `build_router` / `serve` / `run_server` — axum wiring and entrypoints.
- `ValidatedJson<T>` + `ValidatedRequest` — pydantic-style request validation.
- `ApiError` — FastAPI-shaped rejections (422 issue array, 404/400 string detail) with `paradl_core::ExitCode` mapping.
- `TaskStore` / `ApiTask` — thread-safe in-memory task registry with per-task stop flags.
- `TaskRunner` / `RunnerHandle` — bounded-concurrency background worker loop.
- **`CommandExecutor`** (public trait) — the typed command boundary. W4-1 wires the real download body; `NoopExecutor` is the standalone default.
- `CreateTaskRequest`, `CreateTaskResponse`, `ResolveRequest`, `ResolveResponse`, `TaskInfo`, `TaskListResponse`, `TaskActionResponse`, `ValidationIssue`.

## Endpoints

| Method | Path | Success |
|---|---|---|
| POST | `/resolve` | 200 `ResolveResponse` |
| POST | `/tasks` | 202 `CreateTaskResponse` |
| GET | `/tasks` | 200 `TaskListResponse` (optional `?status=`) |
| GET | `/tasks/{id}` | 200 `TaskInfo` / 404 |
| POST | `/tasks/{id}/pause` | 200 `TaskActionResponse` / 400 / 404 |
| POST | `/tasks/{id}/resume` | 200 `TaskActionResponse` / 400 / 404 |
| DELETE | `/tasks/{id}` | 200 `TaskActionResponse` / 404 |

## Validation

`CreateTaskRequest.connections` enforces pydantic `Field(ge=1, le=16)`. An
out-of-range value returns HTTP 422 with the exact pydantic v2 shape, e.g.
`{"detail":[{"type":"less_than_equal","loc":["body","connections"],"msg":"Input should be less than or equal to 16","input":17,"ctx":{"le":16}}]}`.

## Feature flags

| Flag | Default | Effect |
|---|---|---|
| `api` | off | Enables the entire HTTP surface and the axum/tokio dependencies. |

A default `cargo build -p paradl-api` activates **no axum dependency**.

## Example

```bash
cargo run -p paradl-api --features api --example server
```

The example binds an ephemeral loopback port, exercises `/resolve`, `/tasks`
and `GET /tasks`, then shuts the server down.

## Verification

```bash
cargo test -p paradl-api --features api
cargo clippy -p paradl-api --all-features -- -D warnings
cargo doc -p paradl-api --features api --no-deps
```
