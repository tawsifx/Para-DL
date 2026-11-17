//! misterdownloadmanager-mcp: Mister Download Manager's MCP server. An AI assistant starts it and
//! speaks MCP with it on stdin and stdout; it hands what the assistant asks to the running app
//! (docs/ai-assistants.md).

use rmcp::ServiceExt;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Started by hand in a terminal: say what it is instead of waiting silently.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "--version" | "-V" => {
                println!("misterdownloadmanager-mcp {}", env!("CARGO_PKG_VERSION"))
            }
            _ => println!(
                "misterdownloadmanager-mcp: Mister Download Manager's MCP server, for AI \
                 assistants. The assistant starts it (no arguments) and talks to it on stdin and \
                 stdout. Setup: \
                 https://misterdownloadmanager.com/help/ai-assistants.html"
            ),
        }
        return Ok(());
    }
    let server = mdm_mcp::Server::new(std::sync::Arc::new(mdm_ipc::request));
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
