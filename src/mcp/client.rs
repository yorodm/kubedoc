use rig_core::tool::rmcp::McpClientHandler;
use rig_core::tool::server::ToolServerHandle;
use rmcp::ServiceError;
use rmcp::model::ClientInfo;
use rmcp::transport::{TokioChildProcess, streamable_http_client::StreamableHttpClientWorker};

use crate::config::McpServerConfig;

#[derive(Debug, thiserror::Error)]
pub enum McpClientError {
    #[error("Failed to spawn MCP server process: {0}")]
    SpawnProcess(std::io::Error),
    #[error("MCP connection error: {0}")]
    ConnectionError(String),
    #[error("Tool fetch error: {0}")]
    ToolFetch(#[from] ServiceError),
}

pub struct McpConnection {
    /// Keep the running service alive for the lifetime of the agent.
    #[allow(dead_code)]
    running_service: Box<dyn std::any::Any + Send + Sync>,
}

/// Connect to an MCP server and register its tools with the shared handle.
pub async fn connect_and_register(
    config: &McpServerConfig,
    tool_handle: &ToolServerHandle,
) -> Result<McpConnection, McpClientError> {
    let client_info = ClientInfo::default();

    let handler = McpClientHandler::new(client_info, tool_handle.clone());

    let running_service = if let Some(ref url) = config.url {
        let worker = StreamableHttpClientWorker::<reqwest::Client>::new_simple(url.clone());
        handler
            .connect(worker)
            .await
            .map_err(|e| McpClientError::ConnectionError(e.to_string()))?
    } else if let Some(ref cmd) = config.command {
        let mut parts = cmd.iter();
        let program = parts
            .next()
            .ok_or_else(|| McpClientError::ConnectionError("empty command".into()))?;
        let args: Vec<&str> = parts.map(|s| s.as_str()).collect();

        let mut command = tokio::process::Command::new(program);
        command.args(&args);
        let child = TokioChildProcess::new(command).map_err(McpClientError::SpawnProcess)?;

        #[allow(deprecated)]
        let (reader, writer) = child.split();
        handler
            .connect((reader, writer))
            .await
            .map_err(|e| McpClientError::ConnectionError(e.to_string()))?
    } else {
        return Err(McpClientError::ConnectionError(
            "MCP server config must have either 'command' or 'url'".into(),
        ));
    };

    Ok(McpConnection {
        running_service: Box::new(running_service),
    })
}

/// Connect to ALL configured MCP servers and register their tools.
pub async fn connect_all(
    servers: &[McpServerConfig],
    tool_handle: &ToolServerHandle,
) -> Vec<McpConnection> {
    let mut connections = Vec::new();
    for server in servers {
        match connect_and_register(server, tool_handle).await {
            Ok(conn) => {
                tracing::info!("Connected to MCP server: {}", server.name);
                connections.push(conn);
            }
            Err(e) => {
                tracing::warn!("Failed to connect to MCP server '{}': {e}", server.name);
            }
        }
    }
    connections
}
