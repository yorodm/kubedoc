use rig_core::agent::hook::{AgentHook, Flow, HookContext, StepEvent, StepEventKind};
use rig_core::completion::CompletionModel;
use rig_core::completion::message::AssistantContent;
use rig_core::wasm_compat::WasmCompatSend;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
    session_id: String,
}

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub session_id: String,
    pub event_type: String,
    pub agent: Option<String>,
    pub tool_name: Option<String>,
    pub args: Option<String>,
    pub result_summary: Option<String>,
    pub tokens: Option<u64>,
}

impl AuditLog {
    pub fn new(
        session_id: &str,
        data_dir: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = crate::config::kubedoc_home(data_dir).join("audit");

        std::fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}.jsonl", session_id));

        Ok(Self {
            path,
            session_id: session_id.to_string(),
        })
    }

    pub fn log_event(
        &self,
        event_type: &str,
        agent: Option<&str>,
        tool_name: Option<&str>,
        args: Option<&str>,
        result_summary: Option<&str>,
        tokens: Option<u64>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let entry = AuditEntry {
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_id: self.session_id.clone(),
            event_type: event_type.to_string(),
            agent: agent.map(String::from),
            tool_name: tool_name.map(String::from),
            args: args.map(String::from),
            result_summary: result_summary.map(String::from),
            tokens,
        };

        let json = serde_json::to_string(&entry)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        writeln!(file, "{}", json)?;
        Ok(())
    }

    pub fn session_start(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event("session_start", None, None, None, None, None)
    }

    pub fn session_end(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event("session_end", None, None, None, None, None)
    }

    pub fn user_prompt(&self, prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event("user_prompt", None, None, Some(prompt), None, None)
    }

    pub fn tool_call(
        &self,
        agent: &str,
        tool_name: &str,
        args: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event(
            "tool_call",
            Some(agent),
            Some(tool_name),
            Some(args),
            None,
            None,
        )
    }

    pub fn tool_result(
        &self,
        agent: &str,
        tool_name: &str,
        result_summary: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event(
            "tool_result",
            Some(agent),
            Some(tool_name),
            None,
            Some(result_summary),
            None,
        )
    }

    pub fn agent_response(
        &self,
        agent: &str,
        response: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event(
            "agent_response",
            Some(agent),
            None,
            None,
            Some(response),
            None,
        )
    }

    pub fn agent_thinking(&self, agent: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.log_event("agent_thinking", Some(agent), None, None, None, None)
    }
}

pub struct AuditHook {
    log: Arc<AuditLog>,
}

impl AuditHook {
    pub fn new(log: Arc<AuditLog>) -> Self {
        Self { log }
    }
}

impl<M: CompletionModel + 'static> AgentHook<M> for AuditHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(
            kind,
            StepEventKind::ToolCall | StepEventKind::ToolResult | StepEventKind::ModelTurnFinished
        )
    }

    fn on_event(
        &self,
        _ctx: &HookContext,
        event: StepEvent<'_, M>,
    ) -> impl Future<Output = Flow> + WasmCompatSend {
        let log = self.log.clone();
        match event {
            StepEvent::ToolCall {
                tool_name, args, ..
            } => {
                if let Err(e) = log.tool_call("coordinator", tool_name, args) {
                    tracing::warn!("audit: tool_call failed: {e}");
                }
            }
            StepEvent::ToolResult {
                tool_name, result, ..
            } => {
                let summary = if result.len() > 200 {
                    format!("{}...", &result[..200])
                } else {
                    result.to_string()
                };
                if let Err(e) = log.tool_result("coordinator", tool_name, &summary) {
                    tracing::warn!("audit: tool_result failed: {e}");
                }
            }
            StepEvent::ModelTurnFinished {
                turn: _,
                content,
                usage,
            } => {
                if let Err(e) = log.agent_thinking("coordinator") {
                    tracing::warn!("audit: agent_thinking failed: {e}");
                }
                {
                    let text: String = content
                        .iter()
                        .filter_map(|c| match c {
                            AssistantContent::Text(t) => Some(t.text().to_string()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let summary = if text.len() > 500 {
                        format!("{}...", &text[..500])
                    } else {
                        text
                    };
                    if let Err(e) = log.agent_response("coordinator", &summary) {
                        tracing::warn!("audit: agent_response failed: {e}");
                    }
                }
                let total = usage.total_tokens;
                if total > 0 {
                    if let Err(e) = log.log_event(
                        "model_turn",
                        Some("coordinator"),
                        None,
                        None,
                        None,
                        Some(total),
                    ) {
                        tracing::warn!("audit: model_turn failed: {e}");
                    }
                }
            }
            _ => {}
        }
        async move { Flow::Continue }
    }
}
