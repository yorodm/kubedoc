use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Event, Namespace, Node, Pod, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, ListParams, LogParams};
use kube::{
    Client,
    config::{Config, KubeConfigOptions, Kubeconfig},
};
use rig_core::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

// --- Structured state types ---

#[derive(Debug, Serialize)]
pub struct NodeCondition {
    pub type_: String,
    pub status: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct NodeAddress {
    pub type_: String,
    pub address: String,
}

#[derive(Debug, Serialize)]
pub struct ResourceCapacity {
    pub cpu: String,
    pub memory: String,
    pub pods: String,
}

#[derive(Debug, Serialize)]
pub struct NodeState {
    pub name: String,
    pub conditions: Vec<NodeCondition>,
    pub capacity: Option<ResourceCapacity>,
    pub addresses: Vec<NodeAddress>,
    pub container_images_count: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PodState {
    pub name: String,
    pub namespace: String,
    pub phase: String,
    pub host_ip: String,
    pub pod_ip: String,
    pub restarts: i32,
    pub containers: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EventState {
    pub name: String,
    pub namespace: String,
    pub kind: String,
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i32,
}

#[derive(Debug, Serialize)]
pub struct DeploymentState {
    pub name: String,
    pub namespace: String,
    pub desired: i32,
    pub ready: i32,
    pub available: i32,
}

#[derive(Debug, Serialize)]
pub struct ServicePort {
    pub port: i32,
    pub protocol: String,
    pub target_port: String,
}

#[derive(Debug, Serialize)]
pub struct ServiceState {
    pub name: String,
    pub namespace: String,
    pub type_: String,
    pub cluster_ip: String,
    pub ports: Vec<ServicePort>,
}

#[derive(Debug, Serialize)]
pub struct ConfigMapState {
    pub name: String,
    pub namespace: String,
    pub data_keys: usize,
}

#[derive(Debug, Serialize)]
pub struct NamespaceState {
    pub name: String,
    pub phase: String,
}

// --- List result wrappers ---

#[derive(Debug, Serialize)]
pub struct NamespaceListResult {
    pub count: usize,
    pub namespaces: Vec<NamespaceState>,
}

#[derive(Debug, Serialize)]
pub struct NodeListResult {
    pub count: usize,
    pub nodes: Vec<NodeState>,
}

#[derive(Debug, Serialize)]
pub struct PodListResult {
    pub count: usize,
    pub pods: Vec<PodState>,
}

#[derive(Debug, Serialize)]
pub struct EventListResult {
    pub count: usize,
    pub events: Vec<EventState>,
}

#[derive(Debug, Serialize)]
pub struct DeploymentListResult {
    pub count: usize,
    pub deployments: Vec<DeploymentState>,
}

#[derive(Debug, Serialize)]
pub struct ServiceListResult {
    pub count: usize,
    pub services: Vec<ServiceState>,
}

#[derive(Debug, Serialize)]
pub struct ConfigMapListResult {
    pub count: usize,
    pub configmaps: Vec<ConfigMapState>,
}

// --- Helper: wrap result with LLM-friendly summary ---

fn with_summary<T: Serialize>(summary: String, data: T) -> serde_json::Value {
    json!({ "summary": summary, "data": data })
}

// --- Error type ---

#[derive(Debug, thiserror::Error)]
pub enum KubeToolError {
    #[error("Kubernetes API error: {0}")]
    ApiError(#[from] kube::Error),
    #[error("Kubeconfig error: {0}")]
    KubeconfigError(#[from] kube::config::KubeconfigError),
    #[error("{0}")]
    Other(String),
}

// --- Client ---

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
            let kube_config = Kubeconfig::read_from(std::path::Path::new(path)).map_err(|e| {
                KubeToolError::Other(format!("Failed to read kubeconfig from {path}: {e}"))
            })?;
            Config::from_custom_kubeconfig(kube_config, &options)
                .await
                .map_err(|e| {
                    KubeToolError::Other(format!(
                        "Failed to build config from kubeconfig at {path}: {e}"
                    ))
                })?
        } else {
            Config::from_kubeconfig(&options).await.map_err(|e| {
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

// --- Summary functions returning structured state ---

pub fn namespace_state(ns: &Namespace) -> NamespaceState {
    let name = ns.metadata.name.as_deref().unwrap_or("unknown").to_string();
    let phase = ns
        .status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown")
        .to_string();
    NamespaceState { name, phase }
}

pub fn node_state(node: &Node) -> NodeState {
    let name = node
        .metadata
        .name
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let status = node.status.as_ref();

    let conditions = status
        .and_then(|s| s.conditions.as_ref())
        .map(|c| {
            c.iter()
                .map(|cond| NodeCondition {
                    type_: cond.type_.clone(),
                    status: cond.status.clone(),
                    reason: cond.reason.clone().unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    let capacity = status
        .and_then(|s| s.capacity.as_ref())
        .map(|c| ResourceCapacity {
            cpu: c.get("cpu").map(|v| v.0.clone()).unwrap_or_default(),
            memory: c.get("memory").map(|v| v.0.clone()).unwrap_or_default(),
            pods: c.get("pods").map(|v| v.0.clone()).unwrap_or_default(),
        });

    let addresses = status
        .and_then(|s| s.addresses.as_ref())
        .map(|a| {
            a.iter()
                .map(|addr| NodeAddress {
                    type_: addr.type_.clone(),
                    address: addr.address.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let container_images_count = status.and_then(|s| s.images.as_ref()).map(|i| i.len());

    NodeState {
        name,
        conditions,
        capacity,
        addresses,
        container_images_count,
    }
}

pub fn pod_state(pod: &Pod) -> PodState {
    let name = pod
        .metadata
        .name
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let namespace = pod
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
        .to_string();
    let status = pod.status.as_ref();
    let phase = status
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown")
        .to_string();
    let host_ip = status
        .and_then(|s| s.host_ip.as_deref())
        .unwrap_or("N/A")
        .to_string();
    let pod_ip = status
        .and_then(|s| s.pod_ip.as_deref())
        .unwrap_or("N/A")
        .to_string();
    let restarts: i32 = status
        .and_then(|s| s.container_statuses.as_ref())
        .map(|cs| cs.iter().map(|c| c.restart_count).sum())
        .unwrap_or(0);
    let containers: Vec<String> = pod
        .spec
        .as_ref()
        .map(|s| s.containers.iter())
        .map(|c| c.map(|c| c.name.clone()).collect())
        .unwrap_or_default();

    PodState {
        name,
        namespace,
        phase,
        host_ip,
        pod_ip,
        restarts,
        containers,
    }
}

pub fn event_state(event: &Event) -> EventState {
    let name = event
        .metadata
        .name
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let namespace = event
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
        .to_string();
    let kind = event
        .involved_object
        .kind
        .as_deref()
        .unwrap_or("Unknown")
        .to_string();
    let type_ = event.type_.as_deref().unwrap_or("Unknown").to_string();
    let reason = event.reason.as_deref().unwrap_or("").to_string();
    let message = event.message.as_deref().unwrap_or("").to_string();
    let count = event.count.unwrap_or(0);

    EventState {
        name,
        namespace,
        kind,
        type_,
        reason,
        message,
        count,
    }
}

pub fn deployment_state(deploy: &Deployment) -> DeploymentState {
    let name = deploy
        .metadata
        .name
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let namespace = deploy
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
        .to_string();
    let desired = deploy.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
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

    DeploymentState {
        name,
        namespace,
        desired,
        ready,
        available,
    }
}

fn int_or_string_display(v: &IntOrString) -> String {
    match v {
        IntOrString::Int(i) => i.to_string(),
        IntOrString::String(s) => s.clone(),
    }
}

pub fn service_state(svc: &Service) -> ServiceState {
    let name = svc
        .metadata
        .name
        .as_deref()
        .unwrap_or("unknown")
        .to_string();
    let namespace = svc
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
        .to_string();
    let type_ = svc
        .spec
        .as_ref()
        .and_then(|s| s.type_.as_deref())
        .unwrap_or("ClusterIP")
        .to_string();
    let cluster_ip = svc
        .spec
        .as_ref()
        .and_then(|s| s.cluster_ip.as_deref())
        .unwrap_or("None")
        .to_string();
    let ports: Vec<ServicePort> = svc
        .spec
        .as_ref()
        .and_then(|s| s.ports.as_ref())
        .map(|p| {
            p.iter()
                .map(|p| ServicePort {
                    port: p.port,
                    protocol: p.protocol.clone().unwrap_or_default(),
                    target_port: p
                        .target_port
                        .as_ref()
                        .map(int_or_string_display)
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    ServiceState {
        name,
        namespace,
        type_,
        cluster_ip,
        ports,
    }
}

pub fn configmap_state(cm: &ConfigMap) -> ConfigMapState {
    let name = cm.metadata.name.as_deref().unwrap_or("unknown").to_string();
    let namespace = cm
        .metadata
        .namespace
        .as_deref()
        .unwrap_or("default")
        .to_string();
    let data_keys = cm.data.as_ref().map(|d| d.len()).unwrap_or(0);

    ConfigMapState {
        name,
        namespace,
        data_keys,
    }
}

// --- GatherClusterState (batched parallel query) ---

#[derive(Serialize)]
pub struct ClusterState {
    pub nodes: NodeListResult,
    pub pods: PodListResult,
    pub events: EventListResult,
    pub deployments: DeploymentListResult,
}

pub struct GatherClusterState {
    pub client: Client,
}

impl Tool for GatherClusterState {
    const NAME: &'static str = "gather_cluster_state";

    type Error = KubeToolError;
    type Args = serde_json::Value;
    type Output = serde_json::Value;

    fn description(&self) -> String {
        "Query nodes, pods, events, and deployments in parallel and return them as a single result. Use this instead of calling get_nodes + get_pods + get_events + get_deployments separately.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = self.client.clone();
        let (nodes_res, pods_res, events_res, deployments_res) = tokio::join!(
            async {
                let api: Api<Node> = Api::all(client.clone());
                let list = api.list(&ListParams::default().limit(500)).await?;
                let nodes: Vec<NodeState> = list.items.iter().map(node_state).collect();
                Ok::<_, KubeToolError>(NodeListResult {
                    count: nodes.len(),
                    nodes,
                })
            },
            async {
                let api: Api<Pod> = Api::all(client.clone());
                let list = api.list(&ListParams::default().limit(500)).await?;
                let pods: Vec<PodState> = list.items.iter().map(pod_state).collect();
                Ok::<_, KubeToolError>(PodListResult {
                    count: pods.len(),
                    pods,
                })
            },
            async {
                let api: Api<Event> = Api::all(client.clone());
                let list = api.list(&ListParams::default().limit(500)).await?;
                let events: Vec<EventState> = list.items.iter().map(event_state).collect();
                Ok::<_, KubeToolError>(EventListResult {
                    count: events.len(),
                    events,
                })
            },
            async {
                let api: Api<Deployment> = Api::all(client.clone());
                let list = api.list(&ListParams::default().limit(500)).await?;
                let deployments: Vec<DeploymentState> =
                    list.items.iter().map(deployment_state).collect();
                Ok::<_, KubeToolError>(DeploymentListResult {
                    count: deployments.len(),
                    deployments,
                })
            },
        );
        let state = ClusterState {
            nodes: nodes_res?,
            pods: pods_res?,
            events: events_res?,
            deployments: deployments_res?,
        };
        let summary = format!(
            "Cluster state: {} nodes, {} pods, {} events, {} deployments",
            state.nodes.count, state.pods.count, state.events.count, state.deployments.count
        );
        Ok(with_summary(summary, state))
    }
}

// --- Args types ---

#[derive(Deserialize)]
pub struct NamespaceArgs {
    pub namespace: Option<String>,
}

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
    type Args = serde_json::Value;
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let namespaces: Vec<NamespaceState> = list.items.iter().map(namespace_state).collect();
        let data = NamespaceListResult {
            count: namespaces.len(),
            namespaces,
        };
        Ok(with_summary(
            format!("{} namespaces found", data.count),
            data,
        ))
    }
}

// --- GetNodes ---

pub struct GetNodes {
    pub client: Client,
}

impl Tool for GetNodes {
    const NAME: &'static str = "get_nodes";

    type Error = KubeToolError;
    type Args = serde_json::Value;
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let nodes: Vec<NodeState> = list.items.iter().map(node_state).collect();
        let data = NodeListResult {
            count: nodes.len(),
            nodes,
        };
        Ok(with_summary(format!("{} nodes found", data.count), data))
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
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let pods: Vec<PodState> = list.items.iter().map(pod_state).collect();
        let data = PodListResult {
            count: pods.len(),
            pods,
        };
        let ns_label = args.namespace.unwrap_or_default();
        let summary = if ns_label.is_empty() {
            format!("{} pods found across all namespaces", data.count)
        } else {
            format!("{} pods found in namespace {}", data.count, ns_label)
        };
        Ok(with_summary(summary, data))
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
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let events: Vec<EventState> = list.items.iter().map(event_state).collect();
        let data = EventListResult {
            count: events.len(),
            events,
        };
        let ns_label = args.namespace.unwrap_or_default();
        let summary = if ns_label.is_empty() {
            format!("{} events found across all namespaces", data.count)
        } else {
            format!("{} events found in namespace {}", data.count, ns_label)
        };
        Ok(with_summary(summary, data))
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
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let deployments: Vec<DeploymentState> = list.items.iter().map(deployment_state).collect();
        let data = DeploymentListResult {
            count: deployments.len(),
            deployments,
        };
        let ns_label = args.namespace.unwrap_or_default();
        let summary = if ns_label.is_empty() {
            format!("{} deployments found across all namespaces", data.count)
        } else {
            format!("{} deployments found in namespace {}", data.count, ns_label)
        };
        Ok(with_summary(summary, data))
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
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let services: Vec<ServiceState> = list.items.iter().map(service_state).collect();
        let data = ServiceListResult {
            count: services.len(),
            services,
        };
        let ns_label = args.namespace.unwrap_or_default();
        let summary = if ns_label.is_empty() {
            format!("{} services found across all namespaces", data.count)
        } else {
            format!("{} services found in namespace {}", data.count, ns_label)
        };
        Ok(with_summary(summary, data))
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
    type Output = serde_json::Value;

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
        let list = api.list(&ListParams::default().limit(500)).await?;
        let configmaps: Vec<ConfigMapState> = list.items.iter().map(configmap_state).collect();
        let data = ConfigMapListResult {
            count: configmaps.len(),
            configmaps,
        };
        let ns_label = args.namespace.unwrap_or_default();
        let summary = if ns_label.is_empty() {
            format!("{} configmaps found across all namespaces", data.count)
        } else {
            format!("{} configmaps found in namespace {}", data.count, ns_label)
        };
        Ok(with_summary(summary, data))
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
    type Output = serde_json::Value;

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
        let state = node_state(&node);
        Ok(with_summary(
            format!(
                "Node: {} ({})",
                state.name,
                state
                    .conditions
                    .first()
                    .map(|c| c.status.as_str())
                    .unwrap_or("unknown")
            ),
            state,
        ))
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
