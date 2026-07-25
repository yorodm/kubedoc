mod agents;
mod audit;
mod cli;
mod config;
mod mcp;
mod providers;
mod session;
mod tools;
mod trace;
mod tui;

use std::sync::Arc;

use clap::Parser;
use cli::{Cli, Commands, ConfigAction};
use config::kubedoc_home;
use kube::Client;
use rig_core::{
    completion::CompletionModel,
    memory::InMemoryConversationMemory,
};
use tracing::info;

async fn run_interactive_tui<M: CompletionModel + 'static>(
    model: M,
    kube_client: Client,
    mcp_servers: Vec<config::McpServerConfig>,
    memory: &Arc<InMemoryConversationMemory>,
    conversation_id: &str,
    session_manager: &session::SessionManager,
    session_data: &session::SessionData,
) -> Result<(), Box<dyn std::error::Error>> {
    let coordinator = agents::coordinator::Coordinator::new(
        kube_client,
        model,
        mcp_servers,
        None,
        Some(memory.clone() as Arc<dyn rig_core::memory::ConversationMemory>),
        Some(conversation_id.to_string()),
    )
    .await?;
    tui::run(coordinator, conversation_id, Some(session_manager), Some(session_data.clone())).await
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    trace::init(cli.verbose);

    let config = config::KubedocConfig::load(cli.config.as_deref(), &cli)?;

    match cli.command {
        Some(Commands::Audit { session_id }) => {
            let audit_path = kubedoc_home(cli.data_dir.as_deref()).join("audit");

            let path = audit_path.join(format!("{}.jsonl", session_id));
            if !path.exists() {
                eprintln!("No audit log found for session: {}", session_id);
                std::process::exit(1);
            }

            let content = std::fs::read_to_string(&path)?;
            for line in content.lines() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
                    println!(
                        "[{timestamp}] {event_type} {agent}{tool}{detail}",
                        timestamp = entry["timestamp"].as_str().unwrap_or("?"),
                        event_type = entry["event_type"].as_str().unwrap_or("?"),
                        agent = entry["agent"]
                            .as_str()
                            .map(|a| format!(" agent={}", a))
                            .unwrap_or_default(),
                        tool = entry["tool_name"]
                            .as_str()
                            .map(|t| format!(" tool={}", t))
                            .unwrap_or_default(),
                        detail = entry["result_summary"]
                            .as_str()
                            .or_else(|| entry["args"].as_str())
                            .map(|d| format!(" {}", d))
                            .unwrap_or_default(),
                    );
                }
            }
        }

        Some(Commands::Config { action }) => match action {
            ConfigAction::Init => {
                let dir = kubedoc_home(cli.data_dir.as_deref());
                std::fs::create_dir_all(&dir)?;

                let path = dir.join("config.toml");
                if path.exists() {
                    println!("Config already exists at: {}", path.display());
                } else {
                    std::fs::write(
                        &path,
                        r#"# kubedoc configuration file

[llm]
provider = "openai"              # openai | anthropic | groq | ollama
model = "gpt-4o"                 # model name for the chosen provider
api_key_env = "OPENAI_API_KEY"   # env var holding the API key
# base_url = ""                  # optional: custom endpoint for proxies / local

[kube]
# kubeconfig_path = ""           # defaults to ~/.kube/config
# context = ""                   # override kubeconfig's current-context

# [[mcp_servers]]
# name = "prometheus"
# command = ["prometheus-mcp-server"]
"#,
                    )?;
                    println!("Config template written to: {}", path.display());
                }
            }

            ConfigAction::Show => {
                println!("Resolved configuration (secrets redacted):");
                println!("  LLM provider: {}", config.llm.provider);
                println!("  LLM model:    {}", config.llm.model);
                println!(
                    "  Kubeconfig:   {}",
                    config
                        .kube
                        .kubeconfig_path
                        .as_deref()
                        .unwrap_or("~/.kube/config (default)")
                );
                println!(
                    "  Context:      {}",
                    config
                        .kube
                        .context
                        .as_deref()
                        .unwrap_or("(current-context from kubeconfig)")
                );
                println!(
                    "  Audit dir:    {}/audit",
                    kubedoc_home(cli.data_dir.as_deref()).display()
                );
            }
        },

        Some(Commands::Mcp { transport, bind }) => {
            info!(
                "Starting MCP server (transport={}, bind={})",
                transport, bind
            );
            let kube_client = tools::kube_client::KubeClient::new(
                config.kube.kubeconfig_path.as_deref(),
                config.kube.context.clone(),
            )
            .await?
            .into_client();

            let server = mcp::server::KubedocMcpServer::new(kube_client);

            match transport.as_str() {
                "stdio" => {
                    server.run().await?;
                }
                "tcp" => {
                    eprintln!("TCP transport not yet implemented; use 'stdio'");
                    std::process::exit(1);
                }
                other => {
                    eprintln!("Unsupported transport: {other} (use 'stdio' or 'tcp')");
                    std::process::exit(1);
                }
            }
        }

        None => {
            let kube_client = tools::kube_client::KubeClient::new(
                config.kube.kubeconfig_path.as_deref(),
                config.kube.context.clone(),
            )
            .await?
            .into_client();

            let mcp_servers = config
                .mcp
                .as_ref()
                .and_then(|m| m.servers.clone())
                .unwrap_or_default();

            let session_manager = session::SessionManager::new(Some(kubedoc_home(cli.data_dir.as_deref())))?;
            let session_data = session_manager.create();
            let conversation_id = session_data.session_id.clone();
            let memory = Arc::new(InMemoryConversationMemory::new());

            match config.llm.provider.as_str() {
                "openai" => run_interactive_tui(
                    providers::openai_completion(&config)?,
                    kube_client.clone(),
                    mcp_servers.clone(),
                    &memory,
                    &conversation_id,
                    &session_manager,
                    &session_data,
                )
                .await?,
                "anthropic" => run_interactive_tui(
                    providers::anthropic_completion(&config)?,
                    kube_client.clone(),
                    mcp_servers.clone(),
                    &memory,
                    &conversation_id,
                    &session_manager,
                    &session_data,
                )
                .await?,
                "groq" => run_interactive_tui(
                    providers::groq_completion(&config)?,
                    kube_client.clone(),
                    mcp_servers.clone(),
                    &memory,
                    &conversation_id,
                    &session_manager,
                    &session_data,
                )
                .await?,
                "ollama" => run_interactive_tui(
                    providers::ollama_completion(&config)?,
                    kube_client.clone(),
                    mcp_servers.clone(),
                    &memory,
                    &conversation_id,
                    &session_manager,
                    &session_data,
                )
                .await?,
                other => return Err(format!("Unsupported provider: {other}").into()),
            }
        }
    }

    Ok(())
}
