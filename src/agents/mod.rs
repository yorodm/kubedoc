use std::marker::PhantomData;

use rig_core::{agent::Agent, completion::CompletionModel};
use rig_core::completion::Prompt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub mod artifacts;
pub mod coordinator;
pub mod diagnose;
pub mod review;

pub trait SubAgentKind {
    type Output: Serialize + DeserializeOwned + JsonSchema + Default + Send;
}

use serde::de::DeserializeOwned;

#[derive(Debug, Deserialize)]
pub struct SubAgentArgs {
    pub task: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub diagnosis: Option<serde_json::Value>,
}

impl SubAgentArgs {
    pub fn as_parameters() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "Full task description for the subagent. Be explicit: \
                        include file paths, goals, and any constraints. The subagent has \
                        no memory of the current conversation."
                },
                "context": {
                    "type": "string",
                    "description": "Optional extra context or constraints prepended to \
                        the task (e.g. 'The current status of the cluster is ')."
                },
                "diagnosis": {
                    "type": "object",
                    "description": "Optional structured diagnosis from the diagnose agent. \
                        Pass this when calling artifacts to fix diagnosed issues, or review \
                        to analyze diagnosed problems. Contains: summary, root_causes, recommendations."
                }
            },
            "required": ["task"]
        })
    }
}

#[derive(Serialize)]
pub struct SubAgentOutput<O> {
    pub summary: String,
    pub result: String,
    pub data: O,
}

pub struct SubAgentTool<M: CompletionModel, S: SubAgentKind> {
    agent: Agent<M>,
    p: PhantomData<S>,
}

#[derive(Debug, thiserror::Error)]
#[error("spawn_agent error: {0}")]
pub struct SubAgentError(String);

impl<M: CompletionModel + 'static, S: SubAgentKind> SubAgentTool<M, S> {
    pub fn new(agent: Agent<M>) -> Self {
        Self {
            agent,
            p: PhantomData,
        }
    }

    pub async fn call_agent(&self, args: SubAgentArgs) -> Result<SubAgentOutput<S::Output>, SubAgentError> {
        let mut parts = Vec::new();
        if let Some(diag) = args.diagnosis {
            if !diag.is_null() {
                parts.push(format!(
                    "PREVIOUS DIAGNOSIS:\n{}",
                    serde_json::to_string_pretty(&diag).unwrap_or_default()
                ));
            }
        }
        if let Some(ctx) = args.context {
            let trimmed = ctx.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
        }
        parts.push(args.task);
        let full_task = parts.join("\n\n");
        let result = self
            .agent
            .prompt(full_task)
            .await
            .map_err(|e| SubAgentError(e.to_string()))?;
        let data: S::Output = serde_json::from_str(&result).unwrap_or_default();
        let summary = serde_json::from_str::<serde_json::Value>(&result)
            .ok()
            .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(String::from)))
            .unwrap_or_default();
        Ok(SubAgentOutput { summary, result, data })
    }
}
