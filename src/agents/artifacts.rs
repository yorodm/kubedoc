use std::sync::Arc;

use kube::Client;
use rig_core::{
    agent::{Agent, AgentBuilder},
    completion::{CompletionModel, Prompt},
    tool::Tool,
};
use tokio::sync::mpsc;

use crate::tools::artifacts as file_tools;
use crate::{
    agents::{SubAgentArgs, SubAgentError, SubAgentOutput, SubAgentTool},
    tui::progress::{ProgressEvent, ProgressHook},
};
use crate::{
    audit::{AuditHook, AuditLog},
    tools::kube_client,
};

const ARTIFACTS_PREAMBLE: &str = r#"
You are a Kubernetes manifest generator. When asked to create or modify resources:

1. First inspect the current cluster state using available tools to understand
   existing resources, naming conventions, and configurations.
2. Generate the appropriate YAML manifests based on the request.
3. Write the generated manifests to files in the current directory using the
   write_artifact tool. Use descriptive filenames (e.g., "nginx-deployment.yaml").
4. You can also read existing artifact files and list files matching patterns.

Important rules:
- You ONLY generate YAML manifests — you do NOT apply them.
- The user will apply manifests using kubectl, GitOps, or their preferred method.
- Generate complete, valid Kubernetes YAML.
- Prefer standard resource types (Deployment, Service, ConfigMap, etc.).
- Include necessary metadata like labels and selectors.
- Use the namespace from context or default to "default" if not specified.
- For modifications, inspect the existing resource first and only show the diff or updated manifest.
- Write multi-resource manifests to separate files or a single file as appropriate.

When the user asks you to save or write manifests to disk, use write_artifact.
"#;

pub fn build<M: CompletionModel + 'static>(
    client: Client,
    model: M,
    progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    audit_log: Option<Arc<AuditLog>>,
) -> anyhow::Result<Agent<M>> {
    let mut builder = AgentBuilder::new(model)
        .preamble(ARTIFACTS_PREAMBLE)
        .temperature(0.2)
        .tool(kube_client::ListNamespaces {
            client: client.clone(),
        })
        .tool(file_tools::WriteArtifact)
        .tool(file_tools::ReadArtifact)
        .tool(file_tools::ListArtifacts)
        .tool(file_tools::GenerateManifest)
        .tool(file_tools::ValidateManifest)
        .tool(file_tools::ListAvailableApiResources {
            client: client.clone(),
        })
        .default_max_turns(20);

    if let Some(log) = audit_log {
        builder = builder.add_hook(AuditHook::new(log));
    }

    if let Some(tx) = progress_tx {
        builder = builder.add_hook(ProgressHook::new(tx));
    }

    Ok(builder.build())
}

pub struct ArtifactAgent {}

impl<M: CompletionModel + 'static> Tool for SubAgentTool<M, ArtifactAgent> {
    const NAME: &'static str = "artifacts";

    type Error = SubAgentError;

    type Args = SubAgentArgs;

    type Output = SubAgentOutput;

    fn description(&self) -> String {
        "Generate Kubernetes YAML manifests for deployments, services, configmaps, and other resources.
            Use this when the user wants to create or modify resources.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        SubAgentArgs::as_parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let full_task = match args.context {
            Some(ctx) if !ctx.trim().is_empty() => format!("{}\n\n{}", ctx.trim(), args.task),
            _ => args.task,
        };
        let result = self
            .agent
            .prompt(full_task)
            .await
            .map_err(|e| SubAgentError(e.to_string()))?;
        Ok(SubAgentOutput { result })
    }
}
