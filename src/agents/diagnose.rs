use std::sync::Arc;

use kube::Client;
use rig_core::{
    agent::{Agent, AgentBuilder},
    completion::CompletionModel,
    tool::{Tool, server::ToolServerHandle},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::tools::kube_client;
use crate::tui::progress::{ProgressEvent, ProgressHook};
use crate::{
    agents::{SubAgentArgs, SubAgentError, SubAgentKind, SubAgentOutput, SubAgentTool},
    audit::{AuditHook, AuditLog},
};

const DIAGNOSE_PREAMBLE: &str = r#"
You are the primary data-gathering agent. You have direct access to the Kubernetes
cluster and are the ONLY agent that can inspect it. Other agents (review, artifacts)
rely on the data you collect.

When asked to diagnose, gather information from multiple sources:
1. Check node status and capacity
2. List pods and check for crashes, restarts, or pending states
3. Review recent events for warnings or errors
4. Check deployments for replica mismatches
5. Inspect services for endpoint issues
6. Read pod logs when troubleshooting specific pods

You may also have optional MCP tools available for querying cluster metrics
(Prometheus, etc.) — use them when relevant.

Provide a structured diagnosis with sections for:
- Nodes: summary of node health
- Pods: list of unhealthy or unusual pods
- Events: significant events
- Issues Found: clear description of each problem
- Recommendations: actionable steps to resolve issues

Be thorough but concise. If you find no issues, state that clearly.
"#;

pub async fn build<M: CompletionModel + 'static>(
    client: Client,
    model: M,
    tool_handle: ToolServerHandle,
    progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    audit_log: Option<Arc<AuditLog>>,
) -> anyhow::Result<Agent<M>> {
    // Register K8s tools on the shared handle (alongside any MCP tools)
    tool_handle
        .add_tool(kube_client::GetNodes {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetPods {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetEvents {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetDeployments {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetServices {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetConfigMaps {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetNodeDetails {
            client: client.clone(),
        })
        .await?;
    tool_handle
        .add_tool(kube_client::GetPodLogs { client })
        .await?;

    let mut builder = AgentBuilder::new(model)
        .name("diagnostics_agent")
        .preamble(DIAGNOSE_PREAMBLE)
        .temperature(0.0)
        .tool_server_handle(tool_handle)
        .output_schema::<DiagnoseOutput>()
        .default_max_turns(10);

    if let Some(log) = audit_log {
        builder = builder.add_hook(AuditHook::new(log));
    }

    if let Some(tx) = progress_tx {
        builder = builder.add_hook(ProgressHook::new(tx));
    }

    Ok(builder.build())
}

pub struct DiagnoseAgent {}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DiagnoseOutput {
    pub summary: String,
    #[serde(default)]
    pub issues: Vec<DiagnosedIssue>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DiagnosedIssue {
    pub severity: String,
    pub resource: String,
    pub description: String,
}

impl SubAgentKind for DiagnoseAgent {
    type Output = DiagnoseOutput;
}

impl<M: CompletionModel + 'static> Tool for SubAgentTool<M, DiagnoseAgent> {
    const NAME: &'static str = "diagnose";

    type Error = SubAgentError;

    type Args = SubAgentArgs;

    type Output = SubAgentOutput<DiagnoseOutput>;

    fn description(&self) -> String {
        "Inspect the cluster for issues, misconfigurations, and unhealthy resources. Use this for in-depth cluster health diagnosis.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        SubAgentArgs::as_parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.call_agent(args).await
    }
}
