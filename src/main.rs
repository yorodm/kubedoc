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

use anyhow::Context;
use clap::Parser;
use cli::{Cli, Commands, ConfigAction, SessionsAction};
use config::kubedoc_home;
use rig_core::{completion::CompletionModel, memory::InMemoryConversationMemory};
use tracing::info;

struct RunContext {
    session_manager: session::SessionManager,
    session_data: session::SessionData,
    conversation_id: String,
    memory: Arc<InMemoryConversationMemory>,
    audit_log: Arc<audit::AuditLog>,
    mcp_servers: Vec<config::McpServerConfig>,
}

impl RunContext {
    fn new(data_dir: Option<&str>) -> anyhow::Result<Self> {
        let home = kubedoc_home(data_dir);
        let session_manager = session::SessionManager::new(Some(home.clone()))?;
        let session_data = session_manager.create();
        let conversation_id = session_data.session_id.clone();
        let memory = Arc::new(InMemoryConversationMemory::new());
        let audit_log = Arc::new(audit::AuditLog::new(
            &conversation_id,
            Some(&home.to_string_lossy()),
        )?);
        Ok(Self {
            session_manager,
            session_data,
            conversation_id,
            memory,
            audit_log,
            mcp_servers: Vec::new(),
        })
    }
}

async fn run_interactive_tui<M: CompletionModel + 'static>(
    model: M,
    kube_client: kube::Client,
    ctx: &mut RunContext,
) -> anyhow::Result<()> {
    let coordinator = agents::coordinator::Coordinator::new(
        kube_client,
        model,
        ctx.mcp_servers.clone(),
        Some(ctx.audit_log.clone()),
        Some(ctx.memory.clone() as Arc<dyn rig_core::memory::ConversationMemory>),
        Some(ctx.conversation_id.clone()),
    )
    .await?;
    tui::run(
        coordinator,
        &ctx.conversation_id,
        Some(&ctx.session_manager),
        Some(ctx.session_data.clone()),
        Some(ctx.audit_log.clone()),
    )
    .await
}

macro_rules! dispatch_provider {
    ($config:expr, $kube:expr, $ctx:expr, $provider:expr) => {
        match $provider.as_str() {
            "openai" => {
                run_interactive_tui(
                    providers::openai_completion($config)?,
                    $kube.clone(),
                    $ctx,
                )
                .await
            }
            "anthropic" => {
                run_interactive_tui(
                    providers::anthropic_completion($config)?,
                    $kube.clone(),
                    $ctx,
                )
                .await
            }
            "groq" => {
                run_interactive_tui(
                    providers::groq_completion($config)?,
                    $kube.clone(),
                    $ctx,
                )
                .await
            }
            "ollama" => {
                run_interactive_tui(
                    providers::ollama_completion($config)?,
                    $kube.clone(),
                    $ctx,
                )
                .await
            }
            other => Err(anyhow::anyhow!("Unsupported provider: {other}")),
        }
    };
}

fn handle_audit(session_id: &str, data_dir: Option<&str>) -> anyhow::Result<()> {
    let path = kubedoc_home(data_dir)
        .join("audit")
        .join(format!("{session_id}.jsonl"));

    if !path.exists() {
        eprintln!("No audit log found for session: {session_id}");
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
                    .map(|a| format!(" agent={a}"))
                    .unwrap_or_default(),
                tool = entry["tool_name"]
                    .as_str()
                    .map(|t| format!(" tool={t}"))
                    .unwrap_or_default(),
                detail = entry["result_summary"]
                    .as_str()
                    .or_else(|| entry["args"].as_str())
                    .map(|d| format!(" {d}"))
                    .unwrap_or_default(),
            );
        }
    }
    Ok(())
}

fn handle_config(
    action: ConfigAction,
    data_dir: Option<&str>,
    config: &config::KubedocConfig,
) -> anyhow::Result<()> {
    match action {
        ConfigAction::Init => {
            let dir = kubedoc_home(data_dir);
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
                kubedoc_home(data_dir).display()
            );
        }
    }
    Ok(())
}

async fn handle_mcp(
    transport: &str,
    _bind: &str,
    config: &config::KubedocConfig,
) -> anyhow::Result<()> {
    info!("Starting MCP server (transport={transport})");

    let kube_client = tools::kube_client::KubeClient::new(
        config.kube.kubeconfig_path.as_deref(),
        config.kube.context.clone(),
    )
    .await
    .context("Failed to connect to Kubernetes cluster for MCP server")?
    .into_client();

    let server = mcp::server::KubedocMcpServer::new(kube_client);

    match transport {
        "stdio" => server.run().await?,
        "tcp" => {
            eprintln!("TCP transport not yet implemented; use 'stdio'");
            std::process::exit(1);
        }
        other => {
            eprintln!("Unsupported transport: {other} (use 'stdio' or 'tcp')");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn handle_sessions(
    action: SessionsAction,
    data_dir: Option<&str>,
) -> anyhow::Result<()> {
    let sm = session::SessionManager::new(Some(kubedoc_home(data_dir)))?;

    match action {
        SessionsAction::List => {
            let sessions = sm.list()?;
            if sessions.is_empty() {
                println!("No saved sessions.");
            } else {
                for s in &sessions {
                    println!(
                        "{id:<32}  {entries} entries  {updated}",
                        id = s.session_id,
                        entries = s.entries.len(),
                        updated = &s.updated_at[..19],
                    );
                }
            }
        }
        SessionsAction::Show { session_id } => match sm.load(&session_id)? {
            None => {
                eprintln!("Session not found: {session_id}");
                std::process::exit(1);
            }
            Some(data) => {
                println!(
                    "Session: {}  (created {})",
                    data.session_id,
                    &data.created_at[..19]
                );
                println!("---");
                for entry in &data.entries {
                    let prefix = match entry.role.as_str() {
                        "user" => "You",
                        "assistant" => "Agent",
                        other => other,
                    };
                    println!("[{prefix}]\n{}\n", entry.content);
                }
            }
        },
        SessionsAction::Delete { session_id } => match sm.load(&session_id)? {
            None => {
                eprintln!("Session not found: {session_id}");
                std::process::exit(1);
            }
            Some(_) => {
                sm.delete(&session_id)?;
                println!("Deleted session: {session_id}");
            }
        },
    }
    Ok(())
}

async fn handle_interactive(
    config: &config::KubedocConfig,
    data_dir: Option<&str>,
    mcp_servers: Vec<config::McpServerConfig>,
) -> anyhow::Result<()> {
    let kube_client = tools::kube_client::KubeClient::new(
        config.kube.kubeconfig_path.as_deref(),
        config.kube.context.clone(),
    )
    .await
    .context("Failed to connect to Kubernetes cluster — check your kubeconfig and cluster access")?
    .into_client();

    let mut ctx = RunContext::new(data_dir)?;
    ctx.mcp_servers = mcp_servers;

    dispatch_provider!(config, kube_client, &mut ctx, &config.llm.provider)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = Cli::parse();
    trace::init(cli.verbose);

    let config = config::KubedocConfig::load(cli.config.as_deref(), &cli)
        .context("Failed to load configuration")?;

    match cli.command {
        Some(Commands::Audit { session_id }) => {
            handle_audit(&session_id, cli.data_dir.as_deref())?;
        }
        Some(Commands::Config { action }) => {
            handle_config(action, cli.data_dir.as_deref(), &config)?;
        }
        Some(Commands::Mcp { transport, bind }) => {
            handle_mcp(&transport, &bind, &config).await?;
        }
        Some(Commands::Sessions { action }) => {
            handle_sessions(action, cli.data_dir.as_deref())?;
        }
        None => {
            let mcp_servers = config
                .mcp
                .as_ref()
                .and_then(|m| m.servers.clone())
                .unwrap_or_default();
            handle_interactive(&config, cli.data_dir.as_deref(), mcp_servers).await
                .context("Interactive session failed")?;
        }
    }

    Ok(())
}
