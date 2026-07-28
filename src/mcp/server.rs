use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Namespace, Node, Pod, Service};
use kube::Client;
use kube::api::{Api, ListParams, LogParams};
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, Content, ErrorCode,
    InitializeResult, ListToolsResult, PaginatedRequestParams, RawContent, ServerCapabilities,
    ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServerHandler, ServiceExt};
use std::path::Path;
use std::sync::Arc;

use crate::tools::kube_client;

#[derive(Clone)]
pub struct KubedocMcpServer {
    client: Client,
}

impl KubedocMcpServer {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn tool_definitions() -> Vec<Tool> {
        fn tool(name: &'static str, desc: &'static str, schema: serde_json::Value) -> Tool {
            let mut t = Tool::default();
            t.name = name.into();
            t.description = Some(desc.into());
            t.input_schema = Arc::new(schema.as_object().unwrap().clone());
            t
        }

        vec![
            tool(
                "list_namespaces",
                "List all namespaces in the cluster",
                serde_json::json!({"type": "object", "properties": {}}),
            ),
            tool(
                "get_nodes",
                "List all nodes in the cluster with their status and capacity",
                serde_json::json!({"type": "object", "properties": {}}),
            ),
            tool(
                "get_pods",
                "List pods. Optionally filter by namespace (omit for all namespaces).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Namespace to filter pods by (omit for all namespaces)"
                        }
                    }
                }),
            ),
            tool(
                "get_events",
                "List recent events. Optionally filter by namespace (omit for all namespaces).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Namespace to filter events by (omit for all namespaces)"
                        }
                    }
                }),
            ),
            tool(
                "get_deployments",
                "List deployments. Optionally filter by namespace (omit for all namespaces).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Namespace to filter deployments by (omit for all namespaces)"
                        }
                    }
                }),
            ),
            tool(
                "get_services",
                "List services. Optionally filter by namespace (omit for all namespaces).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Namespace to filter services by (omit for all namespaces)"
                        }
                    }
                }),
            ),
            tool(
                "get_configmaps",
                "List configmaps. Optionally filter by namespace (omit for all namespaces).",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "Namespace to filter configmaps by (omit for all namespaces)"
                        }
                    }
                }),
            ),
            tool(
                "get_node_details",
                "Get detailed information about a specific node by name.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The name of the node"
                        }
                    },
                    "required": ["name"]
                }),
            ),
            tool(
                "get_pod_logs",
                "Get recent logs from a pod. Optionally specify a container name.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "namespace": {
                            "type": "string",
                            "description": "The namespace of the pod"
                        },
                        "pod": {
                            "type": "string",
                            "description": "The name of the pod"
                        },
                        "container": {
                            "type": "string",
                            "description": "Optional container name within the pod"
                        }
                    },
                    "required": ["namespace", "pod"]
                }),
            ),
            tool(
                "write_artifact",
                "Write content to a file in the current working directory.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative file path (e.g. 'manifests/nginx.yaml')"
                        },
                        "content": {
                            "type": "string",
                            "description": "File content to write"
                        }
                    },
                    "required": ["path", "content"]
                }),
            ),
            tool(
                "read_artifact",
                "Read the contents of a file in the current working directory.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Relative file path (e.g. 'manifests/nginx.yaml')"
                        }
                    },
                    "required": ["path"]
                }),
            ),
            tool(
                "list_artifacts",
                "List files in the current directory matching a glob pattern.",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern (e.g. '**/*.yaml', 'manifests/*.yml')"
                        }
                    },
                    "required": ["pattern"]
                }),
            ),
        ]
    }

    async fn handle_tool_call(
        &self,
        name: &str,
        mut args: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, ErrorData> {
        let result = match name {
            "list_namespaces" => {
                let api: Api<Namespace> = Api::all(self.client.clone());
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let namespaces: Vec<_> = list.items.iter().map(kube_client::namespace_state).collect();
                serde_json::to_string_pretty(&kube_client::NamespaceListResult {
                    count: namespaces.len(),
                    namespaces,
                })
                .unwrap_or_default()
            }
            "get_nodes" => {
                let api: Api<Node> = Api::all(self.client.clone());
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let nodes: Vec<_> = list.items.iter().map(kube_client::node_state).collect();
                serde_json::to_string_pretty(&kube_client::NodeListResult {
                    count: nodes.len(),
                    nodes,
                })
                .unwrap_or_default()
            }
            "get_pods" => {
                let ns = extract_opt(&mut args, "namespace");
                let api: Api<Pod> = match ns {
                    Some(ref ns) => Api::namespaced(self.client.clone(), ns),
                    None => Api::all(self.client.clone()),
                };
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let pods: Vec<_> = list.items.iter().map(kube_client::pod_state).collect();
                serde_json::to_string_pretty(&kube_client::PodListResult {
                    count: pods.len(),
                    pods,
                })
                .unwrap_or_default()
            }
            "get_events" => {
                let ns = extract_opt(&mut args, "namespace");
                let api: Api<Event> = match ns {
                    Some(ref ns) => Api::namespaced(self.client.clone(), ns),
                    None => Api::all(self.client.clone()),
                };
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let events: Vec<_> = list.items.iter().map(kube_client::event_state).collect();
                serde_json::to_string_pretty(&kube_client::EventListResult {
                    count: events.len(),
                    events,
                })
                .unwrap_or_default()
            }
            "get_deployments" => {
                let ns = extract_opt(&mut args, "namespace");
                let api: Api<Deployment> = match ns {
                    Some(ref ns) => Api::namespaced(self.client.clone(), ns),
                    None => Api::all(self.client.clone()),
                };
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let deployments: Vec<_> = list
                    .items
                    .iter()
                    .map(kube_client::deployment_state)
                    .collect();
                serde_json::to_string_pretty(&kube_client::DeploymentListResult {
                    count: deployments.len(),
                    deployments,
                })
                .unwrap_or_default()
            }
            "get_services" => {
                let ns = extract_opt(&mut args, "namespace");
                let api: Api<Service> = match ns {
                    Some(ref ns) => Api::namespaced(self.client.clone(), ns),
                    None => Api::all(self.client.clone()),
                };
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let services: Vec<_> = list
                    .items
                    .iter()
                    .map(kube_client::service_state)
                    .collect();
                serde_json::to_string_pretty(&kube_client::ServiceListResult {
                    count: services.len(),
                    services,
                })
                .unwrap_or_default()
            }
            "get_configmaps" => {
                let ns = extract_opt(&mut args, "namespace");
                let api: Api<ConfigMap> = match ns {
                    Some(ref ns) => Api::namespaced(self.client.clone(), ns),
                    None => Api::all(self.client.clone()),
                };
                let list = api
                    .list(&ListParams::default().limit(500))
                    .await
                    .map_err(to_error)?;
                let configmaps: Vec<_> = list
                    .items
                    .iter()
                    .map(kube_client::configmap_state)
                    .collect();
                serde_json::to_string_pretty(&kube_client::ConfigMapListResult {
                    count: configmaps.len(),
                    configmaps,
                })
                .unwrap_or_default()
            }
            "get_node_details" => {
                let name = extract_req(&mut args, "name")?;
                let api: Api<Node> = Api::all(self.client.clone());
                let node = api.get(&name).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to get node {name}: {e}"),
                        None,
                    )
                })?;
                let state = kube_client::node_state(&node);
                serde_json::to_string_pretty(&state).unwrap_or_default()
            }
            "get_pod_logs" => {
                let namespace = extract_req(&mut args, "namespace")?;
                let pod = extract_req(&mut args, "pod")?;
                let container = extract_opt(&mut args, "container");
                let api: Api<Pod> = Api::namespaced(self.client.clone(), &namespace);
                let log_params = LogParams {
                    container,
                    tail_lines: Some(100),
                    ..Default::default()
                };
                api.logs(&pod, &log_params).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to get logs for pod {pod}: {e}"),
                        None,
                    )
                })?
            }
            "write_artifact" => {
                let path = extract_req(&mut args, "path")?;
                let content = extract_req(&mut args, "content")?;
                if path.contains("..") || Path::new(&path).is_absolute() {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Path must be relative and must not contain '..'".to_string(),
                        None,
                    ));
                }
                let p = Path::new(&path);
                if let Some(parent) = p.parent()
                    && !parent.as_os_str().is_empty()
                {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ErrorData::new(
                            ErrorCode::INTERNAL_ERROR,
                            format!("Failed to create directory: {e}"),
                            None,
                        )
                    })?;
                }
                tokio::fs::write(p, &content).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to write file: {e}"),
                        None,
                    )
                })?;
                format!("Written {} bytes to {}", content.len(), path)
            }
            "read_artifact" => {
                let path = extract_req(&mut args, "path")?;
                if path.contains("..") || Path::new(&path).is_absolute() {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Path must be relative and must not contain '..'".to_string(),
                        None,
                    ));
                }
                tokio::fs::read_to_string(&path).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to read file: {e}"),
                        None,
                    )
                })?
            }
            "list_artifacts" => {
                let pattern = extract_req(&mut args, "pattern")?;
                if pattern.contains("..") {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Pattern must be relative and must not contain '..'".to_string(),
                        None,
                    ));
                }
                let entries = glob::glob(&pattern).map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        format!("Invalid glob pattern: {e}"),
                        None,
                    )
                })?;
                let paths: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|p| p.display().to_string())
                    .collect();
                if paths.is_empty() {
                    "No files found matching pattern.".to_string()
                } else {
                    format!(
                        "Files matching '{}' ({}):\n{}",
                        pattern,
                        paths.len(),
                        paths.join("\n")
                    )
                }
            }
            _ => {
                return Err(ErrorData::method_not_found::<CallToolRequestMethod>());
            }
        };
        Ok(CallToolResult::success(vec![Content::new(
            RawContent::text(result),
            None,
        )]))
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let service = self.serve(stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}

impl ServerHandler for KubedocMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.instructions =
            Some("Kubernetes cluster diagnostics and manifest generation tools.".to_string());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: Self::tool_definitions(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.handle_tool_call(&request.name, request.arguments)
            .await
    }
}

fn to_error(e: kube::Error) -> ErrorData {
    ErrorData::new(
        ErrorCode::INTERNAL_ERROR,
        format!("Kubernetes API error: {e}"),
        None,
    )
}

fn extract_opt(
    args: &mut Option<serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Option<String> {
    args.as_mut()
        .and_then(|m| m.remove(key))
        .and_then(|v| v.as_str().map(String::from))
}

fn extract_req(
    args: &mut Option<serde_json::Map<String, serde_json::Value>>,
    key: &str,
) -> Result<String, ErrorData> {
    extract_opt(args, key).ok_or_else(|| {
        ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            format!("Missing required argument: {key}"),
            None,
        )
    })
}
