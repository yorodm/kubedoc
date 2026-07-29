use std::fmt;

use rig_core::{agent::Agent, completion::CompletionModel, completion::Prompt, tool::Tool};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug)]
pub struct DispatchError(pub String);

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dispatch error: {}", self.0)
    }
}

impl std::error::Error for DispatchError {}

#[derive(Deserialize)]
pub struct DispatchTask {
    pub agent: String,
    pub task: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub diagnosis: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub struct DispatchArgs {
    pub tasks: Vec<DispatchTask>,
}

pub struct DispatchParallel<M: CompletionModel> {
    pub diagnose: Agent<M>,
    pub review: Agent<M>,
    pub artifacts: Agent<M>,
}

impl<M: CompletionModel + 'static + Send + Sync> Tool for DispatchParallel<M> {
    const NAME: &'static str = "dispatch_parallel";

    type Error = DispatchError;
    type Args = DispatchArgs;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Run multiple sub-agent tasks in parallel. Each task specifies which agent to use \
         ('diagnose', 'review', or 'artifacts'), a task description, and optional context \
         and diagnosis. All tasks run concurrently and results are returned together."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent": {
                                "type": "string",
                                "enum": ["diagnose", "review", "artifacts"],
                                "description": "Which sub-agent to run"
                            },
                            "task": {
                                "type": "string",
                                "description": "Full task description for the sub-agent"
                            },
                            "context": {
                                "type": "string",
                                "description": "Optional context"
                            },
                            "diagnosis": {
                                "type": "object",
                                "description": "Optional structured diagnosis"
                            }
                        },
                        "required": ["agent", "task"]
                    },
                    "description": "List of tasks to run in parallel"
                }
            },
            "required": ["tasks"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut handles = Vec::with_capacity(args.tasks.len());
        for task in args.tasks {
            let parts = compose_parts(&task);
            let full_task = parts.join("\n\n");

            let agent: &Agent<M> = match task.agent.as_str() {
                "diagnose" => &self.diagnose,
                "review" => &self.review,
                "artifacts" => &self.artifacts,
                other => {
                    return Err(DispatchError(format!("Unknown agent: {other}. Use diagnose, review, or artifacts.")));
                }
            };
            let agent = agent.clone();

            handles.push(tokio::spawn(async move {
                let result = agent.prompt(full_task).await;
                (task.agent, result)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            let (agent_name, result) = handle.await.map_err(|e| DispatchError(format!("Join error: {e}")))?;
            match result {
                Ok(text) => results.push(json!({
                    "agent": agent_name,
                    "success": true,
                    "result": text,
                })),
                Err(e) => results.push(json!({
                    "agent": agent_name,
                    "success": false,
                    "error": e.to_string(),
                })),
            }
        }

        Ok(json!({ "results": results }))
    }
}

fn compose_parts(task: &DispatchTask) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(diag) = &task.diagnosis {
        if !diag.is_null() {
            parts.push(format!(
                "PREVIOUS DIAGNOSIS:\n{}",
                serde_json::to_string_pretty(diag).unwrap_or_default()
            ));
        }
    }
    if let Some(ctx) = &task.context {
        let trimmed = ctx.trim().to_string();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts.push(task.task.clone());
    parts
}
