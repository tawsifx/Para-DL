//! Standalone `paradl-api` demo.
//!
//! Starts the seven-endpoint router on an ephemeral loopback port, exercises
//! `/resolve`, `/tasks` and `GET /tasks` with the no-op executor, then shuts
//! the server down. Run with:
//!
//! ```bash
//! cargo run -p paradl-api --features api --example server
//! ```

#![allow(dead_code)]

#[cfg(not(feature = "api"))]
fn main() {
    eprintln!("This example needs the `api` feature: cargo run -p paradl-api --features api --example server");
}

#[cfg(feature = "api")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use paradl_api::{AppState, NoopExecutor, TaskStore, api_server_started_line, build_router};
    use reqwest::Client;
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::sync::watch;

    let store = Arc::new(TaskStore::new());
    let state = AppState::new(Arc::clone(&store), Arc::new(NoopExecutor::new()), ".");
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let base = format!("http://127.0.0.1:{port}");

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let server = tokio::spawn(async move {
        axum::serve(listener, build_router(state))
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.changed().await;
            })
            .await
    });

    print!("{}", api_server_started_line("127.0.0.1", port, 1.0));

    let client = Client::new();
    let resolved: serde_json::Value = client
        .post(format!("{base}/resolve"))
        .json(&json!({"target": "https://example.com/file.zip"}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("POST /resolve -> {resolved}");

    let created: serde_json::Value = client
        .post(format!("{base}/tasks"))
        .json(&json!({"target": "https://example.com/file.zip", "connections": 8}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("POST /tasks   -> {created}");

    let listed: serde_json::Value = client
        .get(format!("{base}/tasks"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    println!("GET  /tasks   -> {listed}");

    let _ = shutdown_tx.send(true);
    server.await??;
    Ok(())
}
