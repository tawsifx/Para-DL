//! End-to-end HTTP tests for the seven endpoints of `src/paradl/api/server.py`.
//!
//! Status codes and `detail` strings are taken verbatim from the Python oracle;
//! the 422 `detail` array shape matches pydantic v2's `RequestValidationError`.

#![cfg(feature = "api")]

use std::sync::Arc;

use axum::Router;
use paradl_api::{
    AppState, NoopExecutor, TaskStore, api_server_started_line, build_router,
};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct TestServer {
    base: String,
    store: Arc<TaskStore>,
    handle: JoinHandle<()>,
}

impl TestServer {
    async fn new() -> Self {
        let store = Arc::new(TaskStore::new());
        let state = AppState::new(store.clone(), Arc::new(NoopExecutor::new()), "/tmp/out");
        Self::with_router(store, build_router(state)).await
    }

    async fn with_router(store: Arc<TaskStore>, router: Router) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { base, store, handle }
    }

    fn client(&self) -> Client {
        Client::new()
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn create_task(server: &TestServer, target: &str) -> String {
    let resp = server
        .client()
        .post(server.url("/tasks"))
        .json(&json!({"target": target}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body: Value = resp.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn post_resolve_returns_full_response_shape() {
    let server = TestServer::new().await;
    let resp = server
        .client()
        .post(server.url("/resolve"))
        .json(&json!({"target": "https://example.test/f"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.json::<Value>().await.unwrap(),
        json!({
            "target": "https://example.test/f",
            "target_type": "direct",
            "clean_url": null,
            "filename_hint": null,
            "content_length": null,
            "mime_type": null,
            "accept_ranges": null,
            "etag_sha256": null,
            "expected_digest": null,
            "status_code": null,
            "probe_error": null
        })
    );
}

#[tokio::test]
async fn validation_errors_use_fastapi_422_shape() {
    let server = TestServer::new().await;

    let low = server
        .client()
        .post(server.url("/tasks"))
        .json(&json!({"target": "t", "connections": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(low.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        low.json::<Value>().await.unwrap(),
        json!({"detail": [{
            "type": "greater_than_equal",
            "loc": ["body", "connections"],
            "msg": "Input should be greater than or equal to 1",
            "input": 0,
            "ctx": {"ge": 1}
        }]})
    );

    let high = server
        .client()
        .post(server.url("/tasks"))
        .json(&json!({"target": "t", "connections": 17}))
        .send()
        .await
        .unwrap();
    assert_eq!(high.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        high.json::<Value>().await.unwrap(),
        json!({"detail": [{
            "type": "less_than_equal",
            "loc": ["body", "connections"],
            "msg": "Input should be less than or equal to 16",
            "input": 17,
            "ctx": {"le": 16}
        }]})
    );

    let missing = server
        .client()
        .post(server.url("/resolve"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        missing.json::<Value>().await.unwrap(),
        json!({"detail": [{
            "type": "missing",
            "loc": ["body", "target"],
            "msg": "Field required",
            "input": {}
        }]})
    );
}

#[tokio::test]
async fn create_and_get_task() {
    let server = TestServer::new().await;
    let create_resp = server
        .client()
        .post(server.url("/tasks"))
        .json(&json!({
            "target": "https://example.test/f",
            "out_dir": "/tmp/custom",
            "connections": 8,
            "priority": 3,
            "filename": "f.bin"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::ACCEPTED);
    let created: Value = create_resp.json().await.unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["status"], "queued");
    assert_eq!(created["message"], "Task created and queued.");
    assert_eq!(id.len(), 36, "uuid v4 id");

    let got: Value = server
        .client()
        .get(server.url(&format!("/tasks/{id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["id"], id);
    assert_eq!(got["target"], "https://example.test/f");
    assert_eq!(got["target_type"], "direct");
    assert_eq!(got["status"], "queued");
    assert_eq!(got["out_dir"], "/tmp/custom");
    assert_eq!(got["filename"], "f.bin");
    assert_eq!(got["connections"], 8);
    assert_eq!(got["priority"], 3);
    assert_eq!(got["bytes_downloaded"], 0);
    assert_eq!(got["total_bytes"], Value::Null);
    assert_eq!(got["speed_bps"], 0.0);
    assert_eq!(got["file_path"], Value::Null);
    assert_eq!(got["error"], Value::Null);
    assert_eq!(got["created_at"].as_str().unwrap().len(), 19);
    assert_eq!(got["updated_at"].as_str().unwrap().len(), 19);
}

#[tokio::test]
async fn list_tasks_filters_by_status() {
    let server = TestServer::new().await;
    let a = create_task(&server, "https://example.test/a").await;
    let b = create_task(&server, "https://example.test/b").await;
    assert!(server.store.set_status(&b, "completed"));

    let all: Value = server
        .client()
        .get(server.url("/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all["count"], 2);
    assert_eq!(all["tasks"].as_array().unwrap().len(), 2);

    let queued: Value = server
        .client()
        .get(server.url("/tasks?status=queued"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queued["count"], 1);
    assert_eq!(queued["tasks"][0]["id"], a);

    let done: Value = server
        .client()
        .get(server.url("/tasks?status=completed"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(done["count"], 1);
    assert_eq!(done["tasks"][0]["id"], b);
}

#[tokio::test]
async fn get_task_not_found_is_404() {
    let server = TestServer::new().await;
    let resp = server
        .client()
        .get(server.url("/tasks/missing"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp.json::<Value>().await.unwrap(), json!({"detail": "Task missing not found"}));
}

#[tokio::test]
async fn pause_resume_delete_lifecycle() {
    let server = TestServer::new().await;
    let id = create_task(&server, "https://example.test/f").await;

    let paused = server
        .client()
        .post(server.url(&format!("/tasks/{id}/pause")))
        .send()
        .await
        .unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    assert_eq!(
        paused.json::<Value>().await.unwrap(),
        json!({"id": id, "status": "paused", "message": "Task paused."})
    );
    assert_eq!(server.store.get(&id).unwrap().status, "paused");

    let resumed = server
        .client()
        .post(server.url(&format!("/tasks/{id}/resume")))
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    assert_eq!(
        resumed.json::<Value>().await.unwrap(),
        json!({"id": id, "status": "queued", "message": "Task queued for resume."})
    );

    let deleted = server
        .client()
        .delete(server.url(&format!("/tasks/{id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(
        deleted.json::<Value>().await.unwrap(),
        json!({"id": id, "status": "deleted", "message": "Task cancelled and removed."})
    );
    assert!(server.store.get(&id).is_none());
}

#[tokio::test]
async fn invalid_state_transitions_are_400() {
    let server = TestServer::new().await;
    let id = create_task(&server, "https://example.test/f").await;

    assert!(server.store.set_status(&id, "completed"));
    let pause = server
        .client()
        .post(server.url(&format!("/tasks/{id}/pause")))
        .send()
        .await
        .unwrap();
    assert_eq!(pause.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        pause.json::<Value>().await.unwrap(),
        json!({"detail": "Cannot pause a completed task"})
    );

    assert!(server.store.set_status(&id, "running"));
    let resume = server
        .client()
        .post(server.url(&format!("/tasks/{id}/resume")))
        .send()
        .await
        .unwrap();
    assert_eq!(resume.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        resume.json::<Value>().await.unwrap(),
        json!({"detail": "Only paused or failed tasks can be resumed (current: running)"})
    );
}

#[tokio::test]
async fn action_on_missing_task_is_404() {
    let server = TestServer::new().await;
    for (method, path) in [
        ("post", "/tasks/nope/pause"),
        ("post", "/tasks/nope/resume"),
        ("delete", "/tasks/nope"),
    ] {
        let req = if method == "post" {
            server.client().post(server.url(path))
        } else {
            server.client().delete(server.url(path))
        };
        let resp = req.send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            resp.json::<Value>().await.unwrap(),
            json!({"detail": "Task nope not found"})
        );
    }
}

#[test]
fn api_server_started_event_is_byte_exact() {
    assert_eq!(
        api_server_started_line("127.0.0.1", 8000, 1.234),
        "{\"event\": \"api_server_started\", \"timestamp\": 1.234, \"host\": \"127.0.0.1\", \"port\": 8000}\n"
    );
}
