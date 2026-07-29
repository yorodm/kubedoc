use std::sync::Arc;

use kube::Client;
use rig_core::{
    agent::AgentBuilder,
    completion::{CompletionModel, Message, Prompt},
    memory::ConversationMemory,
    one_or_many::OneOrMany,
    tool::server::ToolServer,
};

use tokio::sync::mpsc;

use crate::{
    agents::{
        SubAgentTool, artifacts::ArtifactAgent, diagnose::DiagnoseAgent, review::ReviewAgent,
    },
    tools::{dispatch::DispatchParallel, kube_client},
    tui::progress::{ProgressEvent, ProgressHook},
};

const COORDINATOR_PREAMBLE: &str = r#"
You are kubedoc, a Kubernetes cluster assistant with direct Kubernetes access.
You have K8s tools for querying the cluster, plus specialist sub-agents for
deeper investigation, performance review, and manifest generation.

YOUR DIRECT TOOLS (use these first for quick answers):
- get_nodes: List nodes with status, capacity, and conditions
- get_pods: List pods with status, restarts, and node assignment
- get_events: List recent cluster events
- get_deployments: List deployments with replica status
- get_services: List services with cluster IPs and ports
- get_configmaps: List configmaps with data keys
- get_node_details: Get detailed info for a specific node
- get_pod_logs: Get logs for a specific pod
- gather_cluster_state: Query nodes, pods, events, and deployments in
  parallel. Use this instead of calling 4 separate tools.

SUB-AGENTS (use these for complex multi-step work):
- diagnose: Deep investigation. Receives K8s data from you and uses MCP
  metrics tools (Prometheus) for root-cause analysis. Call this when your
  initial queries reveal issues that need deeper investigation.
- review: Performance analysis. Receives cluster data from you and produces
  a structured review. Has no tools. Call this for performance questions.
- artifacts: Kubernetes YAML manifest generation. Has file I/O + limited
  K8s tools. Call this when the user wants to create or modify resources.
  If a prior diagnosis is available, pass it in the diagnosis field so
  artifacts targets the right fixes.
- dispatch_parallel: Run multiple sub-agent tasks concurrently (e.g.
  diagnose a node issue + generate a sidecar manifest at the same time).
  Pass a list of tasks, each specifying agent, task, context, and optional
  diagnosis. All tasks run in parallel and results are returned together.

PROTOCOL:
1. For simple questions, use your K8s tools directly — no sub-agents needed.
2. For a broad overview, call gather_cluster_state (parallel query).
3. If you find issues that need root-cause analysis, call diagnose with
   the relevant K8s data as context. Pass the full diagnose result
   (summary + root_causes) as context.
4. For performance questions, call review with cluster data as context.
   If a diagnosis exists, pass it in the diagnosis field.
5. For manifest generation, call artifacts. If a diagnosis exists, pass
   it in the diagnosis field so the fixes target the right issues.
6. For independent multi-agent work, call dispatch_parallel once instead
   of calling diagnose + review + artifacts sequentially.
7. Always summarize the final result for the user.
"#;

pub struct Coordinator<M: CompletionModel> {
    agent: rig_core::agent::Agent<M>,
    /// Hold MCP server connections alive for the lifetime of the coordinator.
    #[allow(dead_code)]
    mcp_connections: Arc<Vec<crate::mcp::client::McpConnection>>,
}

impl<M: CompletionModel + Clone> Clone for Coordinator<M> {
    fn clone(&self) -> Self {
        Self {
            agent: self.agent.clone(),
            mcp_connections: Arc::clone(&self.mcp_connections),
        }
    }
}

impl<M: CompletionModel + 'static> Coordinator<M> {
    pub async fn new(
        client: Client,
        model: M,
        mcp_servers: Vec<crate::config::McpServerConfig>,
        audit_log: Option<Arc<crate::audit::AuditLog>>,
        memory: Arc<dyn rig_core::memory::ConversationMemory>,
        conversation_id: String,
        progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    ) -> anyhow::Result<Self> {
        // MCP tool handle for the diagnose agent
        let mcp_handle = ToolServer::new().run();

        let has_mcp = !mcp_servers.is_empty();
        let diagnose = crate::agents::diagnose::build(
            model.clone(),
            mcp_handle.clone(),
            progress_tx.clone(),
            audit_log.clone(),
            has_mcp,
        )
        .await?;
        let review = crate::agents::review::build(
            model.clone(),
            progress_tx.clone(),
            audit_log.clone(),
        )?;
        let artifacts = crate::agents::artifacts::build(
            client.clone(),
            model.clone(),
            progress_tx.clone(),
            audit_log.clone(),
        )?;

        // Connect MCP servers onto the handle (visible to diagnose)
        let mcp_connections = crate::mcp::client::connect_all(&mcp_servers, &mcp_handle).await;

        let dispatch_tool = DispatchParallel {
            diagnose: diagnose.clone(),
            review: review.clone(),
            artifacts: artifacts.clone(),
        };

        let mut agent_builder = AgentBuilder::new(model)
            .name("coordinator_agent")
            .preamble(COORDINATOR_PREAMBLE)
            .temperature(0.1)
            .default_max_turns(15)
            .memory(memory)
            .conversation(conversation_id)
            .tool(kube_client::GetNodes {
                client: client.clone(),
            })
            .tool(kube_client::GetPods {
                client: client.clone(),
            })
            .tool(kube_client::GetEvents {
                client: client.clone(),
            })
            .tool(kube_client::GetDeployments {
                client: client.clone(),
            })
            .tool(kube_client::GetServices {
                client: client.clone(),
            })
            .tool(kube_client::GetConfigMaps {
                client: client.clone(),
            })
            .tool(kube_client::GetNodeDetails {
                client: client.clone(),
            })
            .tool(kube_client::GetPodLogs { client: client.clone() })
            .tool(kube_client::GatherClusterState { client })
            .tool(SubAgentTool::<_, DiagnoseAgent>::new(diagnose))
            .tool(SubAgentTool::<_, ReviewAgent>::new(review))
            .tool(SubAgentTool::<_, ArtifactAgent>::new(artifacts))
            .tool(dispatch_tool);

        if let Some(log) = audit_log {
            agent_builder = agent_builder.add_hook(crate::audit::AuditHook::new(log));
        }

        if let Some(tx) = progress_tx {
            agent_builder = agent_builder.add_hook(ProgressHook::new(tx));
        }

        Ok(Self {
            agent: agent_builder.build(),
            mcp_connections: Arc::new(mcp_connections),
        })
    }

    pub async fn run(&self, prompt: &str) -> anyhow::Result<String> {
        let response = self.agent.prompt(prompt).await?;
        Ok(response)
    }

    pub async fn switch_session(
        &mut self,
        new_conversation_id: &str,
        entries: &[crate::session::Entry],
    ) -> anyhow::Result<()> {
        let old_id = self
            .agent
            .default_conversation_id
            .clone()
            .unwrap_or_default();

        if let Some(ref memory) = self.agent.memory {
            memory.clear(&old_id).await?;
        }

        let mut messages = Vec::with_capacity(entries.len());
        for entry in entries {
            let msg = match entry.role.as_str() {
                "user" => Message::from(entry.content.clone()),
                "assistant" => Message::Assistant {
                    id: None,
                    content: OneOrMany::one(rig_core::completion::AssistantContent::Text(
                        entry.content.clone().into(),
                    )),
                },
                _ => Message::from(entry.content.clone()),
            };
            messages.push(msg);
        }

        if let Some(ref memory) = self.agent.memory {
            memory.append(new_conversation_id, messages).await?;
        }

        self.agent.default_conversation_id = Some(new_conversation_id.to_string());
        Ok(())
    }
}
