use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "kubedoc",
    about = "Agentic Kubernetes cluster diagnostics, performance review, and manifest generation",
    version
)]
pub struct Cli {
    /// Path to config file
    #[arg(long, env = "KUBEDOC_CONFIG")]
    pub config: Option<String>,

    /// LLM provider override (openai, anthropic, groq, ollama)
    #[arg(long, env = "KUBEDOC_LLM_PROVIDER")]
    pub provider: Option<String>,

    /// Model override
    #[arg(long, env = "KUBEDOC_LLM_MODEL")]
    pub model: Option<String>,

    /// Data directory for config, sessions, and audit logs (default: ~/.kubedoc)
    #[arg(long, env = "KUBEDOC_DATA_DIR")]
    pub data_dir: Option<String>,

    /// Path to kubeconfig
    #[arg(long, env = "KUBEDOC_KUBECONFIG")]
    pub kubeconfig: Option<String>,

    /// Kubernetes context to use (overrides kubeconfig current-context)
    #[arg(long, short = 'c', env = "KUBEDOC_KUBE_CONTEXT")]
    pub context: Option<String>,

    /// Launch interactive TUI session
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Increase log verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Inspect a past audit session
    Audit {
        /// Session ID to replay
        session_id: String,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Run as an MCP server (stdio or TCP)
    Mcp {
        /// Transport mode
        #[arg(long, default_value = "stdio")]
        transport: String,

        /// TCP bind address (only used with tcp transport)
        #[arg(long, default_value = "127.0.0.1:3000")]
        bind: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Generate a config template at ~/.kubedoc/config.toml
    Init,
    /// Show current resolved config (secrets redacted)
    Show,
}
