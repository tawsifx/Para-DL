//! # paradl-api
//!
//! Opt-in HTTP surface for paraDL: an axum server exposing the seven endpoints
//! of `src/paradl/api/server.py`, pydantic-compatible serde validation, and a
//! task store/runner abstraction. The actual download bodies are **not**
//! implemented here — they are typed commands delegated to a
//! `CommandExecutor` that W4-1 wires to the rest of the framework.
//!
//! Everything except [`API_VERSION`] is behind the `api` feature, so a default
//! build pulls in no axum dependency at all:
//!
//! ```toml
//! [dependencies]
//! paradl-api = { path = "crates/paradl-api", features = ["api"] }
//! ```
//!
//! ```
//! use paradl_api::API_VERSION;
//! assert_eq!(API_VERSION, "1.0.0");
//! ```
//!
//! With the feature enabled, build a router and serve it:
//!
//! ```no_run
//! # #[cfg(feature = "api")]
//! # fn main() -> Result<(), paradl_core::Error> {
//! # use std::sync::Arc;
//! # use paradl_api::{AppState, NoopExecutor, TaskStore, build_router};
//! let state = AppState::new(Arc::new(TaskStore::new()), Arc::new(NoopExecutor::new()), ".");
//! let _router = build_router(state);
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "api"))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]

/// Version of the REST API contract, mirrored from `server.py:656`.
pub const API_VERSION: &str = "1.0.0";

#[cfg(feature = "api")]
mod error;
#[cfg(feature = "api")]
mod models;
#[cfg(feature = "api")]
mod runner;
#[cfg(feature = "api")]
mod server;
#[cfg(feature = "api")]
mod store;

#[cfg(feature = "api")]
pub use error::ApiError;
#[cfg(feature = "api")]
pub use models::{
    CreateTaskRequest, CreateTaskResponse, ResolveRequest, ResolveResponse, TaskActionResponse,
    TaskInfo, TaskListResponse, ValidatedRequest, ValidationIssue,
};
#[cfg(feature = "api")]
pub use runner::{
    CommandExecutor, DownloadOutcome, NoopExecutor, Result as CommandResult, RunnerHandle,
    TaskRunner,
};
#[cfg(feature = "api")]
pub use server::{
    AppState, StatusQuery, ValidatedJson, api_server_started_line, build_router, error_response,
    get_task, list_tasks, resolve, run_server, serve,
};
#[cfg(feature = "api")]
pub use store::{ApiTask, TaskStore};
