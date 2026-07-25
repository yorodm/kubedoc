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

use crate::tui::progress::{ProgressEvent, ProgressHook};

const COORDINATOR_PREAMBLE: &str = r#"
You are kubedoc, a Kubernetes cluster assistant coordinating specialist agents.

You do NOT access the cluster directly. Your tools are specialist sub-agents,
each with their own capabilities. You delegate tasks to the appropriate
sub-agent and summarize their results.

Agent capabilities:
- diagnose: Cluster inspection and issue diagnosis. Has direct K8s tools (nodes,
  pods, events, deployments, services, configmaps, node details, pod logs).
  Call this first to gather cluster state data.
- review: Performance analysis. Has direct access to MCP metrics tools
  (Prometheus) for querying CPU/memory utilization and trends. Call this for
  performance-related questions — it can gather metrics itself.
- artifacts: Kubernetes YAML manifest generation. Has limited K8s inspection
  tools + file I/O tools (write/read/list). Call this when the user wants to
  create or modify resources.

PROTOCOL for multi-step tasks:
1. Call diagnose first to gather cluster state data.
2. If this is a diagnostics session, call review next for performance analysis or recommendations.
3. If the user just want to generate artifacts, call artifacts if the user wants to create or modify resources.
4. Always summarize the final result for the user.
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
        memory: Option<Arc<dyn rig_core::memory::ConversationMemory>>,
        conversation_id: Option<String>,
        progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
    ) -> anyhow::Result<Self> {
        let diagnose = crate::agents::diagnose::build(
            client.clone(),
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

        // Coordinator's tool server: sub-agents + MCP tools
        let tool_handle = ToolServer::new().run();

        // Build review with a clone of the handle so it can access MCP tools (Prometheus)
        let review = crate::agents::review::build(
            model.clone(),
            tool_handle.clone(),
            progress_tx.clone(),
            audit_log.clone(),
        )?;

        tool_handle.add_tool(diagnose).await?;
        tool_handle.add_tool(review).await?;
        tool_handle.add_tool(artifacts).await?;

        // Connect to MCP servers — their tools are registered on the same handle,
        // accessible by both the coordinator and the review agent
        let mcp_connections = crate::mcp::client::connect_all(&mcp_servers, &tool_handle).await;

        let mut agent_builder = AgentBuilder::new(model)
            .preamble(COORDINATOR_PREAMBLE)
            .temperature(0.1)
            .default_max_turns(5)
            .tool_server_handle(tool_handle);

        if let Some(log) = audit_log {
            agent_builder = agent_builder.add_hook(crate::audit::AuditHook::new(log));
        }

        if let Some(tx) = progress_tx {
            agent_builder = agent_builder.add_hook(ProgressHook::new(tx));
        }

        let mut agent = agent_builder.build();
        agent.memory = memory;
        agent.default_conversation_id = conversation_id;

        Ok(Self {
            agent,
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
