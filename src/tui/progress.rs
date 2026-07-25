use rig_core::agent::hook::{AgentHook, Flow, HookContext, StepEvent, StepEventKind};
use rig_core::completion::CompletionModel;
use rig_core::completion::message::AssistantContent;
use rig_core::wasm_compat::WasmCompatSend;
use tokio::sync::mpsc;

/// Events sent from agent hooks to the TUI during execution.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum ProgressEvent {
    ToolCall {
        agent: String,
        tool_name: String,
        args: String,
    },
    ToolResult {
        agent: String,
        tool_name: String,
        result: String,
        duration_ms: u64,
    },
    LlmTurn {
        agent: String,
        turn: usize,
    },
    ModelTurnFinished {
        agent: String,
        turn: usize,
        text: String,
    },
    AgentStart {
        name: String,
    },
}

pub struct ProgressHook {
    tx: mpsc::UnboundedSender<ProgressEvent>,
}

impl ProgressHook {
    pub fn new(tx: mpsc::UnboundedSender<ProgressEvent>) -> Self {
        Self { tx }
    }
}

impl<M: CompletionModel + 'static> AgentHook<M> for ProgressHook {
    fn observes(&self, kind: StepEventKind) -> bool {
        matches!(
            kind,
            StepEventKind::ToolCall
                | StepEventKind::ToolResult
                | StepEventKind::CompletionCall
                | StepEventKind::ModelTurnFinished
        )
    }

    fn on_event(
        &self,
        ctx: &HookContext,
        event: StepEvent<'_, M>,
    ) -> impl Future<Output = Flow> + WasmCompatSend {
        let agent = ctx.agent_name().unwrap_or("agent").to_string();
        let tx = self.tx.clone();

        match event {
            StepEvent::ToolCall {
                tool_name, args, ..
            } => {
                let _ = tx.send(ProgressEvent::ToolCall {
                    agent,
                    tool_name: tool_name.to_string(),
                    args: args.chars().take(150).collect(),
                });
            }
            StepEvent::ToolResult {
                tool_name, result, ..
            } => {
                let _ = tx.send(ProgressEvent::ToolResult {
                    agent,
                    tool_name: tool_name.to_string(),
                    result: result.chars().take(200).collect(),
                    duration_ms: 0,
                });
            }
            StepEvent::CompletionCall { turn, .. } => {
                let _ = tx.send(ProgressEvent::LlmTurn { agent, turn });
            }
            StepEvent::ModelTurnFinished { turn, content, .. } => {
                let text: String = content
                    .iter()
                    .filter_map(|c| match c {
                        AssistantContent::Text(t) => Some(t.text().to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = tx.send(ProgressEvent::ModelTurnFinished {
                    agent,
                    turn,
                    text: text.chars().take(300).collect(),
                });
            }
            _ => {}
        }

        async move { Flow::Continue }
    }
}
