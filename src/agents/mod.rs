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
                }
            },
            "required": ["task"]
        })
    }
}

#[derive(Serialize)]
pub struct SubAgentOutput<O> {
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
        let full_task = match args.context {
            Some(ctx) if !ctx.trim().is_empty() => format!("{}\n\n{}", ctx.trim(), args.task),
            _ => args.task,
        };
        let result = self
            .agent
            .prompt(full_task)
            .await
            .map_err(|e| SubAgentError(e.to_string()))?;
        let data = serde_json::from_str(&result).unwrap_or_default();
        Ok(SubAgentOutput { result, data })
    }
}
