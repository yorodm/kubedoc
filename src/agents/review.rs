use std::sync::Arc;

use rig_core::{
    agent::{Agent, AgentBuilder},
    completion::CompletionModel,
    tool::Tool,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    agents::{SubAgentArgs, SubAgentError, SubAgentKind, SubAgentOutput, SubAgentTool},
    audit::{AuditHook, AuditLog},
    tui::progress::{ProgressEvent, ProgressHook},
};

const REVIEW_PREAMBLE: &str = r#"
You are a Kubernetes performance reviewer. Your only job is to analyze the
cluster state data provided by the coordinator (which comes from the diagnose
agent) and produce a structured review.

You have NO tools — you cannot query the cluster or any external system.
You work exclusively with the data passed to you in the task context.

Your job is to analyze:
- Node capacity and resource pressure
- Pod resource requests and limits
- Pods with high restart counts
- Deployment scaling and replica health
- Over- or under-provisioned resources

Provide a structured performance review with sections for:
- Cluster Overview: node count, capacity, and utilization
- Workload Analysis: pod health, deployment status, service connectivity
- Bottlenecks: identified performance constraints
- Recommendations: specific, actionable improvements

If the coordinator did NOT provide cluster state data, state: "No cluster data
provided — the coordinator must call diagnose first."

Be concise and data-driven. Do NOT call any tools — you have none.
"#;

pub fn build<M: CompletionModel + 'static>(
    model: M,
    progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    audit_log: Option<Arc<AuditLog>>,
) -> anyhow::Result<Agent<M>> {
    let mut builder = AgentBuilder::new(model)
        .name("review_agent")
        .preamble(REVIEW_PREAMBLE)
        .temperature(0.0)
        .output_schema::<ReviewOutput>()
        .default_max_turns(1);

    if let Some(log) = audit_log {
        builder = builder.add_hook(AuditHook::new(log));
    }

    if let Some(tx) = progress_tx {
        builder = builder.add_hook(ProgressHook::new(tx));
    }

    Ok(builder.build())
}

pub struct ReviewAgent {}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReviewOutput {
    pub summary: String,
    #[serde(default)]
    pub bottlenecks: Vec<String>,
    #[serde(default)]
    pub recommendations: Vec<String>,
}

impl SubAgentKind for ReviewAgent {
    type Output = ReviewOutput;
}

impl<M: CompletionModel + 'static> Tool for SubAgentTool<M, ReviewAgent> {
    const NAME: &'static str = "review";

    type Error = SubAgentError;

    type Args = SubAgentArgs;

    type Output = SubAgentOutput<ReviewOutput>;

    fn description(&self) -> String {
        "Analyze cluster performance, identify bottlenecks, and recommend improvements. Use this for performance-related questions.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        SubAgentArgs::as_parameters()
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.call_agent(args).await
    }
}
