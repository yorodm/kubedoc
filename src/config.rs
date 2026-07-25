use std::path::PathBuf;

use serde::Deserialize;

pub fn kubedoc_home(cli_override: Option<&str>) -> PathBuf {
    cli_override.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".kubedoc")
    })
}

#[derive(Debug, Deserialize, Default)]
pub struct KubedocConfig {
    pub llm: LlmConfig,
    pub kube: KubeConfig,
    pub mcp: Option<McpConfig>,
}

#[derive(Debug, Deserialize, Default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct KubeConfig {
    pub kubeconfig_path: Option<String>,
    pub context: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct McpConfig {
    pub servers: Option<Vec<McpServerConfig>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpServerConfig {
    pub name: String,
    pub command: Option<Vec<String>>,
    pub url: Option<String>,
}

impl KubedocConfig {
    /// Load config from file, then apply env var overrides, then CLI arg overrides.
    pub fn load(config_path: Option<&str>, cli: &crate::cli::Cli) -> anyhow::Result<Self> {
        let path = config_path
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| kubedoc_home(cli.data_dir.as_deref()).join("config.toml"));

        let mut config: Self = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            toml::from_str(&content)?
        } else {
            Self::default()
        };

        config.apply_env_overrides();
        config.apply_cli_overrides(cli);
        if config.llm.provider.is_empty() || config.llm.model.is_empty() {
            anyhow::bail!(
                "LLM provider and model must be configured (via config file, env vars, or CLI args)"
            );
        }
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("KUBEDOC_LLM_PROVIDER") {
            self.llm.provider = val;
        }
        if let Ok(val) = std::env::var("KUBEDOC_LLM_MODEL") {
            self.llm.model = val;
        }
        if let Ok(val) = std::env::var("KUBEDOC_KUBECONFIG") {
            self.kube.kubeconfig_path = Some(val);
        }
        if let Ok(val) = std::env::var("KUBEDOC_KUBE_CONTEXT") {
            self.kube.context = Some(val);
        }
    }

    fn apply_cli_overrides(&mut self, cli: &crate::cli::Cli) {
        if let Some(ref provider) = cli.provider {
            self.llm.provider = provider.clone();
        }
        if let Some(ref model) = cli.model {
            self.llm.model = model.clone();
        }
        if let Some(ref kubeconfig) = cli.kubeconfig {
            self.kube.kubeconfig_path = Some(kubeconfig.clone());
        }
        if let Some(ref context) = cli.context {
            self.kube.context = Some(context.clone());
        }
    }
}
