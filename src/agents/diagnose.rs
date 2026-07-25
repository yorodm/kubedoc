use kube::Client;
use rig_core::{
    agent::{Agent, AgentBuilder},
    completion::CompletionModel,
};

use crate::tools::kube_client;

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

Provide a structured diagnosis with sections for:
- Nodes: summary of node health
- Pods: list of unhealthy or unusual pods
- Events: significant events
- Issues Found: clear description of each problem
- Recommendations: actionable steps to resolve issues

Be thorough but concise. If you find no issues, state that clearly.
"#;

pub fn build<M: CompletionModel + 'static>(
    client: Client,
    model: M,
) -> anyhow::Result<Agent<M>> {
    let agent = AgentBuilder::new(model)
        .name("diagnose")
        .description("Inspect the cluster for issues, misconfigurations, and unhealthy resources. Use this for in-depth cluster health diagnosis.")
        .preamble(DIAGNOSE_PREAMBLE)
        .temperature(0.0)
        .tool(kube_client::GetNodes { client: client.clone() })
        .tool(kube_client::GetPods { client: client.clone() })
        .tool(kube_client::GetEvents { client: client.clone() })
        .tool(kube_client::GetDeployments { client: client.clone() })
        .tool(kube_client::GetServices { client: client.clone() })
        .tool(kube_client::GetConfigMaps { client: client.clone() })
        .tool(kube_client::GetNodeDetails { client: client.clone() })
        .tool(kube_client::GetPodLogs { client })
        .build();
    Ok(agent)
}
