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

    fn tool_definitions(client: &Client) -> Vec<Tool> {
        fn mcp_tool<T: rig_core::tool::Tool>(tool: T) -> Tool {
            let mut t = Tool::default();
            t.name = T::NAME.into();
            t.description = Some(tool.description().into());
            t.input_schema = Arc::new(tool.parameters().as_object().unwrap().clone());
            t
        }

        let c = client.clone();
        vec![
            mcp_tool(kube_client::ListNamespaces { client: c.clone() }),
            mcp_tool(kube_client::GetNodes { client: c.clone() }),
            mcp_tool(kube_client::GetPods { client: c.clone() }),
            mcp_tool(kube_client::GetEvents { client: c.clone() }),
            mcp_tool(kube_client::GetDeployments { client: c.clone() }),
            mcp_tool(kube_client::GetServices { client: c.clone() }),
            mcp_tool(kube_client::GetConfigMaps { client: c.clone() }),
            mcp_tool(kube_client::GetNodeDetails { client: c.clone() }),
            mcp_tool(kube_client::GetPodLogs { client: c.clone() }),
            mcp_tool(crate::tools::artifacts::WriteArtifact),
            mcp_tool(crate::tools::artifacts::ReadArtifact),
            mcp_tool(crate::tools::artifacts::EditArtifact),
            mcp_tool(crate::tools::artifacts::ListArtifacts),
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
            "edit_artifact" => {
                let path = extract_req(&mut args, "path")?;
                let old_string = extract_req(&mut args, "old_string")?;
                let new_string = extract_req(&mut args, "new_string")?;
                if path.contains("..") || Path::new(&path).is_absolute() {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        "Path must be relative and must not contain '..'".to_string(),
                        None,
                    ));
                }
                let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to read file: {e}"),
                        None,
                    )
                })?;
                if !content.contains(&old_string) {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        format!("old_string not found in file {path}"),
                        None,
                    ));
                }
                let count = content.matches(&old_string).count();
                if count > 1 {
                    return Err(ErrorData::new(
                        ErrorCode::INVALID_PARAMS,
                        format!("Found {count} matches for old_string in file {path}. Provide more context to make the match unique."),
                        None,
                    ));
                }
                let new_content = content.replace(&old_string, &new_string);
                tokio::fs::write(&path, &new_content).await.map_err(|e| {
                    ErrorData::new(
                        ErrorCode::INTERNAL_ERROR,
                        format!("Failed to write file: {e}"),
                        None,
                    )
                })?;
                let old_len = old_string.len();
                let new_len = new_string.len();
                let diff = if new_len >= old_len {
                    format!("+{} bytes", new_len - old_len)
                } else {
                    format!("-{} bytes", old_len - new_len)
                };
                format!("Edited {path} — replaced 1 occurrence ({old_len} → {new_len} chars, {diff})")
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
            tools: Self::tool_definitions(&self.client),
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
