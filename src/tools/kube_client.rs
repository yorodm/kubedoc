use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Namespace, Node, Pod, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, ListParams, LogParams};
use kube::{
    Client,
    config::{Config, KubeConfigOptions, Kubeconfig},
};
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum KubeToolError {
    #[error("Kubernetes API error: {0}")]
    ApiError(#[from] kube::Error),
    #[error("Kubeconfig error: {0}")]
    KubeconfigError(#[from] kube::config::KubeconfigError),
    #[error("{0}")]
    Other(String),
}

pub struct KubeClient {
    client: Client,
}

impl KubeClient {
    pub async fn new(
        kubeconfig_path: Option<&str>,
        context: Option<String>,
    ) -> Result<Self, KubeToolError> {
        let options = KubeConfigOptions {
            context: context.clone(),
            ..Default::default()
        };

        let config = if let Some(path) = kubeconfig_path {
            let kube_config = Kubeconfig::read_from(std::path::Path::new(path))
                .map_err(|e| {
                    KubeToolError::Other(format!(
                        "Failed to read kubeconfig from {path}: {e}"
                    ))
                })?;
            Config::from_custom_kubeconfig(kube_config, &options)
                .await
                .map_err(|e| {
                    KubeToolError::Other(format!(
                        "Failed to build config from kubeconfig at {path}: {e}"
                    ))
                })?
        } else {
            Config::from_kubeconfig(&options)
                .await
                .map_err(|e| {
                    let ctx_display = context.as_deref().unwrap_or("(default)");
                    KubeToolError::Other(format!(
                        "Failed to load kubeconfig (context={ctx_display}): {e}"
                    ))
                })?
        };

        let server = config.cluster_url.clone();
        let client = Client::try_from(config).map_err(|e| {
            KubeToolError::Other(format!(
                "Failed to create Kubernetes client for {server}: {e}"
            ))
        })?;
        Ok(Self { client })
    }

    pub fn into_client(self) -> Client {
        self.client
    }
}

pub fn node_summary(node: &Node) -> String {
    let name = node.metadata.name.as_deref().unwrap_or("unknown");
    let status = node.status.as_ref();
    let conditions = status
        .and_then(|s| s.conditions.as_ref())
        .map(|c| {
            c.iter()
                .map(|cond| {
                    format!(
                        "  type={} status={} reason={}",
                        cond.type_.as_str(),
                        cond.status.as_str(),
                        cond.reason.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let capacity = status
        .and_then(|s| s.capacity.as_ref())
        .map(|c| {
            format!(
                "  capacity: cpu={} memory={} pods={}",
                c.get("cpu").map(|v| v.0.as_str()).unwrap_or("?"),
                c.get("memory").map(|v| v.0.as_str()).unwrap_or("?"),
                c.get("pods").map(|v| v.0.as_str()).unwrap_or("?"),
            )
        })
        .unwrap_or_default();
    format!("Node: {name}\n{conditions}\n{capacity}")
}

pub fn pod_summary(pod: &Pod) -> String {
    let name = pod.metadata.name.as_deref().unwrap_or("unknown");
    let ns = pod.metadata.namespace.as_deref().unwrap_or("default");
    let status = pod.status.as_ref();
    let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or("Unknown");
    let host_ip = status.and_then(|s| s.host_ip.as_deref()).unwrap_or("N/A");
    let pod_ip = status.and_then(|s| s.pod_ip.as_deref()).unwrap_or("N/A");
    let restarts: i32 = status
        .and_then(|s| s.container_statuses.as_ref())
        .map(|cs| cs.iter().map(|c| c.restart_count).sum())
        .unwrap_or(0);
    let containers: Vec<String> = pod
        .spec
        .as_ref().map(|s| s.containers.iter())
        .map(|c| c.map(|c| c.name.clone()).collect())
        .unwrap_or_default();
    format!(
        "Pod: {name} namespace={ns} phase={phase} node_ip={host_ip} pod_ip={pod_ip} restarts={restarts} containers={}",
        containers.join(", ")
    )
}

pub fn event_summary(event: &Event) -> String {
    let name = event.metadata.name.as_deref().unwrap_or("unknown");
    let ns = event.metadata.namespace.as_deref().unwrap_or("default");
    let kind = event.involved_object.kind.as_deref().unwrap_or("Unknown");
    let type_ = event.type_.as_deref().unwrap_or("Unknown");
    let reason = event.reason.as_deref().unwrap_or("");
    let message = event.message.as_deref().unwrap_or("");
    let count = event.count.unwrap_or(0);
    format!(
        "Event: {name} namespace={ns} kind={kind} type={type_} reason={reason} count={count} message={message}"
    )
}

pub fn deployment_summary(deploy: &Deployment) -> String {
    let name = deploy.metadata.name.as_deref().unwrap_or("unknown");
    let ns = deploy.metadata.namespace.as_deref().unwrap_or("default");
    let replicas = deploy.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    let ready = deploy
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let available = deploy
        .status
        .as_ref()
        .and_then(|s| s.available_replicas)
        .unwrap_or(0);
    format!(
        "Deployment: {name} namespace={ns} desired={replicas} ready={ready} available={available}"
    )
}

fn int_or_string_display(v: &IntOrString) -> String {
    match v {
        IntOrString::Int(i) => i.to_string(),
        IntOrString::String(s) => s.clone(),
    }
}

pub fn service_summary(svc: &Service) -> String {
    let name = svc.metadata.name.as_deref().unwrap_or("unknown");
    let ns = svc.metadata.namespace.as_deref().unwrap_or("default");
    let type_ = svc
        .spec
        .as_ref()
        .and_then(|s| s.type_.as_deref())
        .unwrap_or("ClusterIP");
    let cluster_ip = svc
        .spec
        .as_ref()
        .and_then(|s| s.cluster_ip.as_deref())
        .unwrap_or("None");
    let ports: Vec<String> = svc
        .spec
        .as_ref()
        .and_then(|s| s.ports.as_ref())
        .map(|p| {
            p.iter()
                .map(|p| {
                    format!(
                        "{}/{}->{}",
                        p.port,
                        p.protocol.as_deref().unwrap_or("TCP"),
                        p.target_port
                            .as_ref()
                            .map(int_or_string_display)
                            .unwrap_or_default()
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    format!(
        "Service: {name} namespace={ns} type={type_} cluster_ip={cluster_ip} ports=[{}]",
        ports.join(", ")
    )
}

pub fn configmap_summary(cm: &ConfigMap) -> String {
    let name = cm.metadata.name.as_deref().unwrap_or("unknown");
    let ns = cm.metadata.namespace.as_deref().unwrap_or("default");
    let data_count = cm.data.as_ref().map(|d| d.len()).unwrap_or(0);
    format!("ConfigMap: {name} namespace={ns} data_keys={data_count}")
}

// --- Args types ---

#[derive(Deserialize)]
pub struct NamespaceArgs {
    pub namespace: Option<String>,
}

#[derive(Deserialize)]
pub struct NoArgs;

#[derive(Deserialize)]
pub struct NodeNameArgs {
    pub name: String,
}

#[derive(Deserialize)]
pub struct PodLogArgs {
    pub namespace: String,
    pub pod: String,
    pub container: Option<String>,
}

// --- ListNamespaces ---

pub struct ListNamespaces {
    pub client: Client,
}

impl Tool for ListNamespaces {
    const NAME: &'static str = "list_namespaces";

    type Error = KubeToolError;
    type Args = NoArgs;
    type Output = String;

    fn description(&self) -> String {
        "List all namespaces in the cluster".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Namespace> = Api::all(self.client.clone());
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list
            .items
            .iter()
            .map(|ns| {
                let name = ns.metadata.name.as_deref().unwrap_or("unknown");
                let status = ns.status.as_ref();
                let phase = status.and_then(|s| s.phase.as_deref()).unwrap_or("Unknown");
                format!("{name} (phase: {phase})")
            })
            .collect();
        if items.is_empty() {
            Ok("No namespaces found.".to_string())
        } else {
            Ok(format!(
                "Namespaces ({}):\n{}",
                items.len(),
                items.join("\n")
            ))
        }
    }
}

// --- GetNodes ---

pub struct GetNodes {
    pub client: Client,
}

impl Tool for GetNodes {
    const NAME: &'static str = "get_nodes";

    type Error = KubeToolError;
    type Args = NoArgs;
    type Output = String;

    fn description(&self) -> String {
        "List all nodes in the cluster with their status and capacity".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Node> = Api::all(self.client.clone());
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(node_summary).collect();
        if items.is_empty() {
            Ok("No nodes found.".to_string())
        } else {
            Ok(format!(
                "Nodes ({}):\n{}",
                items.len(),
                items.join("\n---\n")
            ))
        }
    }
}

// --- GetPods ---

pub struct GetPods {
    pub client: Client,
}

impl Tool for GetPods {
    const NAME: &'static str = "get_pods";

    type Error = KubeToolError;
    type Args = NamespaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "List pods. Optionally filter by namespace (omit for all namespaces).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "Namespace to filter pods by (omit for all namespaces)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Pod> = match args.namespace {
            Some(ref ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(pod_summary).collect();
        if items.is_empty() {
            Ok("No pods found.".to_string())
        } else {
            Ok(format!("Pods ({}):\n{}", items.len(), items.join("\n")))
        }
    }
}

// --- GetEvents ---

pub struct GetEvents {
    pub client: Client,
}

impl Tool for GetEvents {
    const NAME: &'static str = "get_events";

    type Error = KubeToolError;
    type Args = NamespaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "List recent events. Optionally filter by namespace (omit for all namespaces).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "Namespace to filter events by (omit for all namespaces)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Event> = match args.namespace {
            Some(ref ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(event_summary).collect();
        if items.is_empty() {
            Ok("No events found.".to_string())
        } else {
            Ok(format!("Events ({}):\n{}", items.len(), items.join("\n")))
        }
    }
}

// --- GetDeployments ---

pub struct GetDeployments {
    pub client: Client,
}

impl Tool for GetDeployments {
    const NAME: &'static str = "get_deployments";

    type Error = KubeToolError;
    type Args = NamespaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "List deployments. Optionally filter by namespace (omit for all namespaces).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "Namespace to filter deployments by (omit for all namespaces)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Deployment> = match args.namespace {
            Some(ref ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(deployment_summary).collect();
        if items.is_empty() {
            Ok("No deployments found.".to_string())
        } else {
            Ok(format!(
                "Deployments ({}):\n{}",
                items.len(),
                items.join("\n")
            ))
        }
    }
}

// --- GetServices ---

pub struct GetServices {
    pub client: Client,
}

impl Tool for GetServices {
    const NAME: &'static str = "get_services";

    type Error = KubeToolError;
    type Args = NamespaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "List services. Optionally filter by namespace (omit for all namespaces).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "Namespace to filter services by (omit for all namespaces)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Service> = match args.namespace {
            Some(ref ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(service_summary).collect();
        if items.is_empty() {
            Ok("No services found.".to_string())
        } else {
            Ok(format!("Services ({}):\n{}", items.len(), items.join("\n")))
        }
    }
}

// --- GetConfigMaps ---

pub struct GetConfigMaps {
    pub client: Client,
}

impl Tool for GetConfigMaps {
    const NAME: &'static str = "get_configmaps";

    type Error = KubeToolError;
    type Args = NamespaceArgs;
    type Output = String;

    fn description(&self) -> String {
        "List configmaps. Optionally filter by namespace (omit for all namespaces).".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "namespace": {
                    "type": "string",
                    "description": "Namespace to filter configmaps by (omit for all namespaces)"
                }
            }
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<ConfigMap> = match args.namespace {
            Some(ref ns) => Api::namespaced(self.client.clone(), ns),
            None => Api::all(self.client.clone()),
        };
        let list = api.list(&ListParams::default()).await?;
        let items: Vec<String> = list.items.iter().map(configmap_summary).collect();
        if items.is_empty() {
            Ok("No configmaps found.".to_string())
        } else {
            Ok(format!(
                "ConfigMaps ({}):\n{}",
                items.len(),
                items.join("\n")
            ))
        }
    }
}

// --- GetNodeDetails ---

pub struct GetNodeDetails {
    pub client: Client,
}

impl Tool for GetNodeDetails {
    const NAME: &'static str = "get_node_details";

    type Error = KubeToolError;
    type Args = NodeNameArgs;
    type Output = String;

    fn description(&self) -> String {
        "Get detailed information about a specific node by name.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the node"
                }
            },
            "required": ["name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Node> = Api::all(self.client.clone());
        let node = api.get(&args.name).await.map_err(|e| {
            KubeToolError::Other(format!("Failed to get node {}: {}", args.name, e))
        })?;
        let summary = node_summary(&node);
        let extra = node
            .status
            .as_ref()
            .map(|s| {
                let mut parts = Vec::new();
                if let Some(addrs) = &s.addresses {
                    for addr in addrs {
                        parts.push(format!(
                            "  address: type={} address={}",
                            addr.type_.as_str(),
                            addr.address.as_str()
                        ));
                    }
                }
                if let Some(images) = &s.images {
                    parts.push(format!("  container_images_count: {}", images.len()));
                }
                parts.join("\n")
            })
            .unwrap_or_default();
        Ok(format!("{summary}\n{extra}"))
    }
}

// --- GetPodLogs ---

pub struct GetPodLogs {
    pub client: Client,
}

impl Tool for GetPodLogs {
    const NAME: &'static str = "get_pod_logs";

    type Error = KubeToolError;
    type Args = PodLogArgs;
    type Output = String;

    fn description(&self) -> String {
        "Get recent logs from a pod. Optionally specify a container name.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api: Api<Pod> = Api::namespaced(self.client.clone(), &args.namespace);
        let log_params = LogParams {
            container: args.container,
            tail_lines: Some(100),
            ..Default::default()
        };
        let logs = api.logs(&args.pod, &log_params).await.map_err(|e| {
            KubeToolError::Other(format!("Failed to get logs for pod {}: {}", args.pod, e))
        })?;
        Ok(logs)
    }
}
