use std::marker::PhantomData;

use rig_core::{agent::Agent, completion::CompletionModel};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub mod artifacts;
pub mod coordinator;
pub mod diagnose;
pub mod review;

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
pub struct SubAgentOutput {
    pub result: String,
}
pub struct SubAgentTool<M: CompletionModel, SubAgent> {
    agent: Agent<M>,
    p: PhantomData<SubAgent>,
}

#[derive(Debug, thiserror::Error)]
#[error("spawn_agent error: {0}")]
pub struct SubAgentError(String);

impl<M: CompletionModel, SubAgent> SubAgentTool<M, SubAgent> {
    pub fn new(agent: Agent<M>) -> Self {
        Self {
            agent,
            p: PhantomData,
        }
    }
}
