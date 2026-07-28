use kube::Client;
use kube::discovery::Scope;
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

/// Validate that a user-supplied path is safe (no `..`, no absolute paths).
fn validate_path(path: &str) -> Result<PathBuf, FileToolError> {
    if path.contains("..") {
        return Err(FileToolError::Other(
            "Path must not contain '..'".to_string(),
        ));
    }
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return Err(FileToolError::Other(
            "Path must be relative, not absolute".to_string(),
        ));
    }
    Ok(p)
}

/// Validate a glob pattern doesn't escape the working directory.
fn validate_glob_pattern(pattern: &str) -> Result<(), FileToolError> {
    if pattern.contains("..") {
        return Err(FileToolError::Other(
            "Pattern must not contain '..'".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum FileToolError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Deserialize)]
pub struct WriteArtifactArgs {
    pub path: String,
    pub content: String,
}

pub struct WriteArtifact;

impl Tool for WriteArtifact {
    const NAME: &'static str = "write_artifact";

    type Error = FileToolError;
    type Args = WriteArtifactArgs;
    type Output = String;

    fn description(&self) -> String {
        "Write content to a file in the current working directory. Path is relative to the current directory.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
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
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = validate_path(&args.path)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &args.content).await?;
        Ok(format!(
            "Written {} bytes to {}",
            args.content.len(),
            args.path
        ))
    }
}

#[derive(Deserialize)]
pub struct ReadArtifactArgs {
    pub path: String,
}

pub struct ReadArtifact;

impl Tool for ReadArtifact {
    const NAME: &'static str = "read_artifact";

    type Error = FileToolError;
    type Args = ReadArtifactArgs;
    type Output = String;

    fn description(&self) -> String {
        "Read the contents of a file in the current working directory.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path (e.g. 'manifests/nginx.yaml')"
                }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let _path = validate_path(&args.path)?;
        let content = tokio::fs::read_to_string(&_path).await?;
        Ok(content)
    }
}

#[derive(Deserialize)]
pub struct ListArtifactsArgs {
    pub pattern: String,
}

pub struct ListArtifacts;

impl Tool for ListArtifacts {
    const NAME: &'static str = "list_artifacts";

    type Error = FileToolError;
    type Args = ListArtifactsArgs;
    type Output = String;

    fn description(&self) -> String {
        "List files in the current directory matching a glob pattern.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (e.g. '**/*.yaml', 'manifests/*.yml')"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        validate_glob_pattern(&args.pattern)?;
        let entries = glob::glob(&args.pattern)
            .map_err(|e| FileToolError::Other(format!("Invalid glob pattern: {e}")))?;
        let paths: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|p| p.display().to_string())
            .collect();
        if paths.is_empty() {
            Ok("No files found matching pattern.".to_string())
        } else {
            Ok(format!(
                "Files matching '{}' ({}):\n{}",
                args.pattern,
                paths.len(),
                paths.join("\n")
            ))
        }
    }
}

// --- GenerateManifest ---

#[derive(Deserialize)]
pub struct GenerateManifestArgs {
    pub kind: String,
    pub spec: String,
    pub name: String,
    pub namespace: Option<String>,
}

fn kind_to_api_version(kind: &str) -> Result<&'static str, FileToolError> {
    let v = match kind {
        "Pod"
        | "Service"
        | "ConfigMap"
        | "Secret"
        | "Endpoints"
        | "LimitRange"
        | "ResourceQuota"
        | "ReplicationController"
        | "ServiceAccount"
        | "Event"
        | "Namespace"
        | "Node"
        | "PersistentVolume"
        | "PersistentVolumeClaim"
        | "ComponentStatus" => "v1",
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "ControllerRevision" => {
            "apps/v1"
        }
        "Ingress" | "IngressClass" | "NetworkPolicy" => "networking.k8s.io/v1",
        "Job" | "CronJob" => "batch/v1",
        "Role" | "ClusterRole" | "RoleBinding" | "ClusterRoleBinding" => {
            "rbac.authorization.k8s.io/v1"
        }
        "HorizontalPodAutoscaler" => "autoscaling/v2",
        "PodDisruptionBudget" | "Eviction" => "policy/v1",
        "CustomResourceDefinition" => "apiextensions.k8s.io/v1",
        "MutatingWebhookConfiguration" | "ValidatingWebhookConfiguration" => {
            "admissionregistration.k8s.io/v1"
        }
        "APIService" => "apiregistration.k8s.io/v1",
        "PriorityClass" | "RuntimeClass" => "scheduling.k8s.io/v1",
        "CSIDriver" | "CSINode" | "StorageClass" | "VolumeAttachment" => "storage.k8s.io/v1",
        "SelfSubjectReview"
        | "TokenReview"
        | "SubjectAccessReview"
        | "SelfSubjectAccessReview"
        | "SelfSubjectRulesReview"
        | "LocalSubjectAccessReview" => "authorization.k8s.io/v1",
        "CertificateSigningRequest" => "certificates.k8s.io/v1",
        "FlowSchema" | "PriorityLevelConfiguration" => "flowcontrol.apiserver.k8s.io/v1",
        "ClusterCIDR" | "IPAddress" => "networking.k8s.io/v1alpha1",
        "ValidatingAdmissionPolicy" | "ValidatingAdmissionPolicyBinding" => {
            "admissionregistration.k8s.io/v1"
        }
        _ => {
            return Err(FileToolError::Other(format!(
                "Unknown resource kind: {kind}. Provide a valid Kubernetes resource kind."
            )));
        }
    };
    Ok(v)
}

pub struct GenerateManifest;

impl Tool for GenerateManifest {
    const NAME: &'static str = "generate_manifest";

    type Error = FileToolError;
    type Args = GenerateManifestArgs;
    type Output = String;

    fn description(&self) -> String {
        "Generate a valid Kubernetes YAML manifest for a given resource kind. Provide the kind (e.g. 'Deployment'), a name, and a JSON spec string with the desired fields.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "description": "Kubernetes resource kind (e.g. 'Deployment', 'Service', 'ConfigMap')"
                },
                "spec": {
                    "type": "string",
                    "description": "JSON string representing the resource spec and other fields. Example: '{\"replicas\": 3, \"selector\": {\"matchLabels\": {\"app\": \"nginx\"}}, \"template\": {\"metadata\": {\"labels\": {\"app\": \"nginx\"}}, \"spec\": {\"containers\": [{\"name\": \"nginx\", \"image\": \"nginx:1.25\"}]}}}'"
                },
                "name": {
                    "type": "string",
                    "description": "The metadata.name for the resource"
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional namespace for the resource"
                }
            },
            "required": ["kind", "spec", "name"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let api_version = kind_to_api_version(&args.kind)?;
        let spec_value: serde_json::Value = serde_json::from_str(&args.spec)
            .map_err(|e| FileToolError::Other(format!("Invalid spec JSON: {e}")))?;

        let mut metadata = serde_json::Map::new();
        metadata.insert("name".into(), json!(args.name));
        if let Some(ns) = &args.namespace {
            metadata.insert("namespace".into(), json!(ns));
        }

        let mut doc = serde_json::Map::new();
        doc.insert("apiVersion".into(), json!(api_version));
        doc.insert("kind".into(), json!(&args.kind));
        doc.insert("metadata".into(), json!(metadata));
        if let Some(obj) = spec_value.as_object() {
            let mut spec_map = serde_json::Map::new();
            for (k, v) in obj {
                spec_map.insert(k.clone(), v.clone());
            }
            doc.insert("spec".into(), json!(spec_map));
        } else if !spec_value.is_null() {
            doc.insert("spec".into(), spec_value);
        }

        let yaml = serde_yaml::to_string(&doc)
            .map_err(|e| FileToolError::Other(format!("Failed to serialize to YAML: {e}")))?;
        Ok(yaml)
    }
}

// --- ValidateManifest ---

#[derive(Deserialize)]
pub struct ValidateManifestArgs {
    pub yaml: String,
}

#[derive(serde::Serialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub issues: Vec<String>,
}

pub struct ValidateManifest;

impl Tool for ValidateManifest {
    const NAME: &'static str = "validate_manifest";

    type Error = FileToolError;
    type Args = ValidateManifestArgs;
    type Output = String;

    fn description(&self) -> String {
        "Validate a Kubernetes YAML manifest. Checks that it has the required fields (apiVersion, kind, metadata.name) and is valid YAML.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "yaml": {
                    "type": "string",
                    "description": "The YAML manifest content to validate"
                }
            },
            "required": ["yaml"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let mut issues: Vec<String> = Vec::new();

        let value: serde_yaml::Value = match serde_yaml::from_str(&args.yaml) {
            Ok(v) => v,
            Err(e) => {
                return Ok(serde_json::to_string_pretty(&ValidationResult {
                    valid: false,
                    issues: vec![format!("Invalid YAML: {e}")],
                })
                .unwrap_or_default());
            }
        };

        let mapping = match value.as_mapping() {
            Some(m) => m,
            None => {
                return Ok(serde_json::to_string_pretty(&ValidationResult {
                    valid: false,
                    issues: vec!["Root is not a YAML mapping (expected a dict)".into()],
                })
                .unwrap_or_default());
            }
        };

        if !mapping.contains_key(serde_yaml::Value::String("apiVersion".into())) {
            issues.push("Missing required field: apiVersion".into());
        }
        if !mapping.contains_key(serde_yaml::Value::String("kind".into())) {
            issues.push("Missing required field: kind".into());
        }

        let metadata = mapping.get(serde_yaml::Value::String("metadata".into()));
        match metadata {
            Some(serde_yaml::Value::Mapping(m)) => {
                if !m.contains_key(serde_yaml::Value::String("name".into())) {
                    issues.push("Missing required field: metadata.name".into());
                }
            }
            Some(_) => {
                issues.push("metadata must be a mapping".into());
            }
            None => {
                issues.push("Missing required field: metadata".into());
            }
        }

        let valid = issues.is_empty();
        Ok(serde_json::to_string_pretty(&ValidationResult { valid, issues }).unwrap_or_default())
    }
}

// --- EditArtifact ---

#[derive(Deserialize)]
pub struct EditArtifactArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}

pub struct EditArtifact;

impl Tool for EditArtifact {
    const NAME: &'static str = "edit_artifact";

    type Error = FileToolError;
    type Args = EditArtifactArgs;
    type Output = String;

    fn description(&self) -> String {
        "Edit a file by finding a unique string and replacing it with new content. The old_string must match exactly one location in the file.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative file path to edit (e.g. 'manifests/nginx.yaml')"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to find and replace (must appear exactly once in the file)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement text"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = validate_path(&args.path)?;
        let content = tokio::fs::read_to_string(&path).await?;

        if !content.contains(&args.old_string) {
            return Err(FileToolError::Other(format!(
                "old_string not found in file {}: matching text does not appear in the file content",
                args.path
            )));
        }

        let count = content.matches(&args.old_string).count();
        if count > 1 {
            return Err(FileToolError::Other(format!(
                "Found {} matches for old_string in file {}. Provide more surrounding context to make the match unique.",
                count, args.path
            )));
        }

        let new_content = content.replace(&args.old_string, &args.new_string);
        tokio::fs::write(&path, &new_content).await?;

        let old_len = args.old_string.len();
        let new_len = args.new_string.len();
        let diff = if new_len >= old_len {
            format!("+{} bytes", new_len - old_len)
        } else {
            format!("-{} bytes", old_len - new_len)
        };

        Ok(format!(
            "Edited {} — replaced 1 occurrence (original: {old_len} chars, replacement: {new_len} chars, {diff})",
            args.path
        ))
    }
}

// --- ListAvailableApiResources ---

#[derive(Deserialize)]
pub struct ListAvailableApiResourcesArgs;

pub struct ListAvailableApiResources {
    pub client: Client,
}

impl Tool for ListAvailableApiResources {
    const NAME: &'static str = "list_available_api_resources";

    type Error = FileToolError;
    type Args = ListAvailableApiResourcesArgs;
    type Output = String;

    fn description(&self) -> String {
        "List all available Kubernetes API resource kinds in the target cluster with their API group, version, and namespaced scope.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, Self::Error> {
        let discovery = kube::discovery::Discovery::new(self.client.clone())
            .run()
            .await
            .map_err(|e| FileToolError::Other(format!("API discovery failed: {e}")))?;

        let mut lines: Vec<String> = Vec::new();
        for group in discovery.groups() {
            let group_name = group.name();
            for version in group.versions() {
                for (resource, caps) in group.versioned_resources(version) {
                    let namespaced = if caps.scope == Scope::Namespaced {
                        "namespaced"
                    } else {
                        "cluster"
                    };
                    lines.push(format!(
                        "{}.{}/{} ({})",
                        resource.kind, group_name, version, namespaced
                    ));
                }
            }
        }

        if lines.is_empty() {
            Ok("No API resources discovered.".to_string())
        } else {
            lines.sort();
            Ok(format!(
                "Available API resources ({}):\n{}",
                lines.len(),
                lines.join("\n")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    static FS_LOCK: Mutex<()> = Mutex::const_new(());

    #[tokio::test]
    async fn test_generate_manifest_basic() {
        let tool = GenerateManifest;
        let result = tool
            .call(GenerateManifestArgs {
                kind: "Deployment".into(),
                name: "nginx".into(),
                namespace: None,
                spec: r#"{"replicas": 3, "selector": {"matchLabels": {"app": "nginx"}}, "template": {"metadata": {"labels": {"app": "nginx"}}, "spec": {"containers": [{"name": "nginx", "image": "nginx:1.25"}]}}}"#.into(),
            })
            .await
            .unwrap();

        assert!(result.contains("apiVersion: apps/v1"));
        assert!(result.contains("kind: Deployment"));
        assert!(result.contains("name: nginx"));
        assert!(result.contains("replicas: 3"));
        assert!(result.contains("image: nginx:1.25"));
    }

    #[tokio::test]
    async fn test_generate_manifest_with_namespace() {
        let tool = GenerateManifest;
        let result = tool
            .call(GenerateManifestArgs {
                kind: "Service".into(),
                name: "my-svc".into(),
                namespace: Some("prod".into()),
                spec: r#"{"ports": [{"port": 80, "targetPort": 8080}]}"#.into(),
            })
            .await
            .unwrap();

        assert!(result.contains("apiVersion: v1"));
        assert!(result.contains("kind: Service"));
        assert!(result.contains("name: my-svc"));
        assert!(result.contains("namespace: prod"));
        assert!(result.contains("port: 80"));
    }

    #[tokio::test]
    async fn test_generate_manifest_invalid_spec() {
        let tool = GenerateManifest;
        let result = tool
            .call(GenerateManifestArgs {
                kind: "Pod".into(),
                name: "test".into(),
                namespace: None,
                spec: "not valid json".into(),
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_manifest_valid() {
        let tool = ValidateManifest;
        let yaml = r#"
apiVersion: v1
kind: Pod
metadata:
  name: my-pod
spec:
  containers:
    - name: nginx
      image: nginx:1.25
"#;
        let result = tool
            .call(ValidateManifestArgs {
                yaml: yaml.to_string(),
            })
            .await
            .unwrap();
        assert!(result.contains("\"valid\": true"));
    }

    #[tokio::test]
    async fn test_validate_manifest_missing_fields() {
        let tool = ValidateManifest;
        let yaml = r#"
kind: Pod
metadata:
  labels:
    app: test
"#;
        let result = tool
            .call(ValidateManifestArgs {
                yaml: yaml.to_string(),
            })
            .await
            .unwrap();
        assert!(result.contains("\"valid\": false"));
        assert!(result.contains("apiVersion"));
        assert!(result.contains("metadata.name"));
    }

    #[tokio::test]
    async fn test_validate_manifest_invalid_yaml() {
        let tool = ValidateManifest;
        let result = tool
            .call(ValidateManifestArgs {
                yaml: "{ invalid yaml content".into(),
            })
            .await
            .unwrap();
        assert!(result.contains("\"valid\": false"));
        assert!(result.contains("Invalid YAML"));
    }

    #[tokio::test]
    async fn test_write_and_read_artifact() {
        let _lock = FS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let write = WriteArtifact;
        let result = write
            .call(WriteArtifactArgs {
                path: "test-output.txt".into(),
                content: "hello world".into(),
            })
            .await
            .unwrap();
        assert!(result.contains("Written 11 bytes"));

        let read = ReadArtifact;
        let content = read
            .call(ReadArtifactArgs {
                path: "test-output.txt".into(),
            })
            .await
            .unwrap();
        assert_eq!(content, "hello world");

        let _ = std::env::set_current_dir(&orig_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_list_artifacts() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.yaml"), "a").unwrap();
        std::fs::write(dir.join("b.yaml"), "b").unwrap();
        std::fs::write(dir.join("c.json"), "c").unwrap();

        let pattern = format!("{}/*.yaml", dir.display());
        let tool = ListArtifacts;
        let result = tool.call(ListArtifactsArgs { pattern }).await.unwrap();
        assert!(result.contains("a.yaml"));
        assert!(result.contains("b.yaml"));
        assert!(!result.contains("c.json"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_artifact() {
        let _lock = FS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let write = WriteArtifact;
        write
            .call(WriteArtifactArgs {
                path: "test.yaml".into(),
                content: "foo: bar\nbaz: qux\n".into(),
            })
            .await
            .unwrap();

        let edit = EditArtifact;
        let result = edit
            .call(EditArtifactArgs {
                path: "test.yaml".into(),
                old_string: "baz: qux".into(),
                new_string: "baz: edited".into(),
            })
            .await
            .unwrap();
        assert!(result.contains("Edited"));

        let read = ReadArtifact;
        let content = read
            .call(ReadArtifactArgs {
                path: "test.yaml".into(),
            })
            .await
            .unwrap();
        assert_eq!(content, "foo: bar\nbaz: edited\n");

        let _ = std::env::set_current_dir(&orig_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_artifact_not_found() {
        let _lock = FS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let write = WriteArtifact;
        write
            .call(WriteArtifactArgs {
                path: "test.yaml".into(),
                content: "foo: bar".into(),
            })
            .await
            .unwrap();

        let edit = EditArtifact;
        let result = edit
            .call(EditArtifactArgs {
                path: "test.yaml".into(),
                old_string: "nonexistent".into(),
                new_string: "replacement".into(),
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));

        let _ = std::env::set_current_dir(&orig_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_edit_artifact_ambiguous() {
        let _lock = FS_LOCK.lock().await;
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let write = WriteArtifact;
        write
            .call(WriteArtifactArgs {
                path: "test.yaml".into(),
                content: "foo: bar\nfoo: bar\n".into(),
            })
            .await
            .unwrap();

        let edit = EditArtifact;
        let result = edit
            .call(EditArtifactArgs {
                path: "test.yaml".into(),
                old_string: "foo: bar".into(),
                new_string: "baz: qux".into(),
            })
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Found 2 matches"));

        let _ = std::env::set_current_dir(&orig_dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
