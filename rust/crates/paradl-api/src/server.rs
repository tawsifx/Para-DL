//! Axum router, handlers and server entrypoint (`server.py:652-912`).

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{FromRequest, Path, Query, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use paradl_events::{ApiServerStartedEvent, Event, emit_jsonl, now_timestamp};
use serde::Deserialize;
use serde_json::{Value, from_slice};
use tokio::net::TcpListener;

use crate::error::ApiError;
use crate::models::{
    CreateTaskRequest, CreateTaskResponse, ResolveRequest, ResolveResponse, TaskActionResponse,
    TaskInfo, TaskListResponse, ValidatedRequest, ValidationIssue,
};
use crate::runner::{CommandExecutor, NoopExecutor, Result as CommandResult, TaskRunner};
use crate::store::{ApiTask, TaskStore, now_string};

/// Dependencies shared by every handler.
#[derive(Clone)]
pub struct AppState {
    /// Shared task registry.
    pub store: Arc<TaskStore>,
    /// Typed command boundary; W4-1 wires the production implementation.
    pub executor: Arc<dyn CommandExecutor>,
    /// Fallback output directory when a request omits `out_dir`.
    pub out_dir: String,
}

impl AppState {
    /// Build server state from its three collaborators.
    pub fn new(
        store: Arc<TaskStore>,
        executor: Arc<dyn CommandExecutor>,
        out_dir: impl Into<String>,
    ) -> Self {
        Self { store, executor, out_dir: out_dir.into() }
    }
}

/// Extractor that applies pydantic-style validation before a handler runs.
pub struct ValidatedJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidatedJson<T>
where
    T: ValidatedRequest,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state)
            .await
            .map_err(|err| ApiError::http(StatusCode::BAD_REQUEST, err.to_string()))?;
        let value: Value = from_slice(&bytes).map_err(|err| {
            ApiError::validation(vec![ValidationIssue::value_error(
                vec!["body".into()],
                Value::Null,
                format!("JSON decode error: {err}"),
            )])
        })?;
        let parsed = T::validate(&value).map_err(ApiError::validation)?;
        Ok(Self(parsed))
    }
}

/// `GET /tasks` optional status filter.
#[derive(Debug, Default, Deserialize)]
pub struct StatusQuery {
    /// Only return tasks with this status.
    pub status: Option<String>,
}

/// `POST /resolve` handler.
pub async fn resolve(
    State(state): State<AppState>,
    ValidatedJson(req): ValidatedJson<ResolveRequest>,
) -> Result<Json<ResolveResponse>, ApiError> {
    state.executor.resolve(&req.target).await.map(Json).map_err(ApiError::from_core)
}

/// `POST /tasks` handler: create and queue a task, answering HTTP 202.
pub async fn create_task(
    State(state): State<AppState>,
    ValidatedJson(req): ValidatedJson<CreateTaskRequest>,
) -> Result<(StatusCode, Json<CreateTaskResponse>), ApiError> {
    let target_type = state
        .executor
        .resolve(&req.target)
        .await
        .map(|resolved| resolved.target_type)
        .unwrap_or_else(|_| "direct".into());
    let now = now_string();
    let task = ApiTask {
        id: uuid::Uuid::new_v4().to_string(),
        target: req.target,
        target_type,
        status: "queued".into(),
        out_dir: req.out_dir.unwrap_or_else(|| state.out_dir.clone()),
        filename: req.filename,
        connections: req.connections,
        priority: req.priority,
        bytes_downloaded: 0,
        total_bytes: None,
        speed_bps: 0.0,
        file_path: None,
        error: None,
        created_at: now.clone(),
        updated_at: now,
        stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    state.store.create(task.clone());
    Ok((
        StatusCode::ACCEPTED,
        Json(CreateTaskResponse {
            id: task.id,
            status: "queued".into(),
            message: "Task created and queued.".into(),
        }),
    ))
}

/// `GET /tasks` handler.
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Json<TaskListResponse> {
    let mut tasks: Vec<TaskInfo> = state.store.list_all().iter().map(ApiTask::to_api).collect();
    if let Some(status) = &query.status {
        tasks.retain(|task| &task.status == status);
    }
    let count = tasks.len();
    Json(TaskListResponse { tasks, count })
}

/// `GET /tasks/{task_id}` handler.
pub async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskInfo>, ApiError> {
    state
        .store
        .get(&task_id)
        .map(|task| Json(task.to_api()))
        .ok_or_else(|| ApiError::not_found(&task_id))
}

/// `POST /tasks/{task_id}/pause` handler.
pub async fn pause_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskActionResponse>, ApiError> {
    let task = state.store.get(&task_id).ok_or_else(|| ApiError::not_found(&task_id))?;
    if matches!(task.status.as_str(), "completed" | "failed") {
        return Err(ApiError::http(
            StatusCode::BAD_REQUEST,
            format!("Cannot pause a {} task", task.status),
        ));
    }
    state.store.set_status(&task_id, "paused");
    state.store.request_stop(&task_id);
    Ok(Json(TaskActionResponse {
        id: task_id,
        status: "paused".into(),
        message: "Task paused.".into(),
    }))
}

/// `POST /tasks/{task_id}/resume` handler.
pub async fn resume_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskActionResponse>, ApiError> {
    let task = state.store.get(&task_id).ok_or_else(|| ApiError::not_found(&task_id))?;
    if !matches!(task.status.as_str(), "paused" | "failed") {
        return Err(ApiError::http(
            StatusCode::BAD_REQUEST,
            format!("Only paused or failed tasks can be resumed (current: {})", task.status),
        ));
    }
    state.store.set_status(&task_id, "queued");
    state.store.clear_stop(&task_id);
    Ok(Json(TaskActionResponse {
        id: task_id,
        status: "queued".into(),
        message: "Task queued for resume.".into(),
    }))
}

/// `DELETE /tasks/{task_id}` handler.
pub async fn delete_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskActionResponse>, ApiError> {
    let task = state.store.get(&task_id).ok_or_else(|| ApiError::not_found(&task_id))?;
    state.store.request_stop(&task.id);
    state.store.delete(&task_id);
    Ok(Json(TaskActionResponse {
        id: task_id,
        status: "deleted".into(),
        message: "Task cancelled and removed.".into(),
    }))
}

/// Build the seven-endpoint router with the given shared state.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/resolve", post(resolve))
        .route("/tasks", post(create_task).get(list_tasks))
        .route("/tasks/{task_id}", get(get_task).delete(delete_task))
        .route("/tasks/{task_id}/pause", post(pause_task))
        .route("/tasks/{task_id}/resume", post(resume_task))
        .with_state(state)
}

/// Serve an already-bound listener until the process exits.
pub async fn serve(listener: TcpListener, router: Router) -> CommandResult<()> {
    axum::serve(listener, router)
        .await
        .map_err(|err| paradl_core::Error::failure(format!("HTTP server error: {err}")))
}

/// Render the `api_server_started` event (E17) through the single emitter.
///
/// `timestamp` is injectable so golden tests can assert the exact line.
pub fn api_server_started_line(host: &str, port: u16, timestamp: f64) -> String {
    emit_jsonl(&Event::ApiServerStarted(ApiServerStartedEvent::new(
        timestamp,
        host.to_string(),
        port,
    )))
}

/// Run a standalone server with the no-op executor (blocking).
///
/// W4-1 calls [`build_router`] / [`serve`] directly with the production
/// executor; this convenience path keeps the crate runnable on its own.
pub fn run_server(host: &str, port: u16) -> CommandResult<()> {
    print!("{}", api_server_started_line(host, port, now_timestamp()));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let store = Arc::new(TaskStore::new());
        let runner = Arc::new(TaskRunner::new(
            Arc::clone(&store),
            Arc::new(NoopExecutor::new()),
            2,
        ))
        .spawn();
        let state = AppState::new(store, Arc::new(NoopExecutor::new()), ".");
        let listener = TcpListener::bind((host, port)).await?;
        let result = serve(listener, build_router(state)).await;
        runner.stop().await;
        result
    })
}

/// Build a JSON error response, exposed for consumers writing custom handlers.
pub fn error_response(error: ApiError) -> Response {
    error.into_response()
}
