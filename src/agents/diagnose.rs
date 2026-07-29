use std::sync::Arc;

use rig_core::{
    agent::{Agent, AgentBuilder},
    completion::CompletionModel,
    tool::{Tool, server::ToolServerHandle},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::tui::progress::{ProgressEvent, ProgressHook};
use crate::{
    agents::{SubAgentArgs, SubAgentError, SubAgentKind, SubAgentOutput, SubAgentTool},
    audit::{AuditHook, AuditLog},
};

const DIAGNOSE_PREAMBLE: &str = r#"
You are a deep-investigation agent. You do NOT have direct Kubernetes access.
The coordinator queries the cluster and passes you the relevant K8s data as
context. You have MCP tools (Prometheus metrics, etc.) for querying cluster
resource utilization.

Your job is to analyze the provided K8s data and MCP metrics to identify
root causes of issues. You are called when the coordinator's initial K8s
queries reveal something worth investigating further.

Focus on:
- CrashLoopBackOff or OOMKilled pods — check resource usage trends
- Node pressure (CPU/memory/disk) — correlate with pod scheduling issues
- Deployment replica mismatches — check rollout history
- Service endpoint failures — check selector label mismatches

Provide a structured diagnosis with sections for:
- Summary: what you investigated and what you found
- Root Causes: identified root causes with evidence
- Recommendations: actionable steps to resolve issues

If the coordinator did NOT provide cluster state data, state: "No cluster data
provided — the coordinator must query K8s first."
"#;

const DIAGNOSE_PREAMBLE_NO_MCP: &str = r#"
You are a deep-investigation agent. You do NOT have direct Kubernetes access
and no MCP tools are available. The coordinator queries the cluster and passes
you the relevant K8s data as context.

Your job is to analyze the provided K8s data to identify root causes of issues
using only what the coordinator has provided — you have no additional data
sources.

Provide a structured diagnosis:
- Summary: what you investigated and what you found
- Root Causes: identified root causes with evidence
- Recommendations: actionable steps to resolve issues

If the coordinator did NOT provide cluster state data, state: "No cluster data
provided — the coordinator must query K8s first."
"#;

pub async fn build<M: CompletionModel + 'static>(
    model: M,
    tool_handle: ToolServerHandle,
    progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    audit_log: Option<Arc<AuditLog>>,
    has_mcp: bool,
) -> anyhow::Result<Agent<M>> {
    let preamble = if has_mcp {
        DIAGNOSE_PREAMBLE
    } else {
        DIAGNOSE_PREAMBLE_NO_MCP
    };
    let mut builder = AgentBuilder::new(model)
        .name("diagnostics_agent")
        .preamble(preamble)
        .temperature(0.0)
        .tool_server_handle(tool_handle)
        .output_schema::<DiagnoseOutput>()
        .default_max_turns(if has_mcp { 10 } else { 1 });

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
    pub root_causes: Vec<RootCause>,
    #[serde(default)]
    pub recommendations: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RootCause {
    pub issue: String,
    pub evidence: String,
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
        "Deep-dive investigation into specific cluster issues. Use this when your initial K8s queries reveal problems that need root-cause analysis.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        SubAgentArgs::as_parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.call_agent(args).await
    }
}
