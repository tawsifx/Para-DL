//! HTTP error type bridging FastAPI `HTTPException` shapes and `paradl_core`.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use paradl_core::ExitCode;
use serde_json::json;

use crate::models::ValidationIssue;

/// Every rejection that can leave an API handler.
///
/// `Validation` mirrors FastAPI's `RequestValidationError` (HTTP 422 with an
/// array `detail`); `Http` mirrors `HTTPException` (HTTP 404/400 with a string
/// `detail`). Both shapes are byte-identical to the Python oracle.
#[derive(Debug)]
pub enum ApiError {
    /// Pydantic-style validation failure.
    Validation(Vec<ValidationIssue>),
    /// FastAPI-style `HTTPException` with a string detail.
    Http {
        /// HTTP status code.
        status: StatusCode,
        /// The exact `detail` string.
        detail: String,
    },
    /// Internal failure (HTTP 500).
    Internal(String),
}

impl ApiError {
    /// Build a 422 validation rejection.
    pub fn validation(issues: Vec<ValidationIssue>) -> Self {
        Self::Validation(issues)
    }

    /// Build an `HTTPException`-shaped rejection.
    pub fn http(status: StatusCode, detail: impl Into<String>) -> Self {
        Self::Http { status, detail: detail.into() }
    }

    /// Build the exact `Task {id} not found` 404 used by the oracle.
    pub fn not_found(task_id: &str) -> Self {
        Self::http(StatusCode::NOT_FOUND, format!("Task {task_id} not found"))
    }

    /// Convert a framework error while preserving its exit-code family.
    pub fn from_core(error: paradl_core::Error) -> Self {
        let status = match error.exit_code() {
            ExitCode::Ok => StatusCode::OK,
            ExitCode::Usage | ExitCode::VerificationFailed | ExitCode::MissingDependency => {
                StatusCode::BAD_REQUEST
            }
            ExitCode::Failure | ExitCode::Interrupted => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::http(status, error.to_string())
    }

    /// Return the process exit code this rejection corresponds to.
    ///
    /// Validation and 4xx client errors map to [`ExitCode::Usage`]; server
    /// errors map to [`ExitCode::Failure`].
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Validation(_) => ExitCode::Usage,
            Self::Http { status, .. } if status.is_client_error() => ExitCode::Usage,
            Self::Http { .. } | Self::Internal(_) => ExitCode::Failure,
        }
    }
}

impl From<paradl_core::Error> for ApiError {
    fn from(error: paradl_core::Error) -> Self {
        Self::from_core(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Validation(issues) => {
                (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({ "detail": issues }))).into_response()
            }
            Self::Http { status, detail } => {
                (status, Json(json!({ "detail": detail }))).into_response()
            }
            Self::Internal(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "detail": message }))).into_response()
            }
        }
    }
}
