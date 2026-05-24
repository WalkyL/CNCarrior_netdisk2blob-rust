mod browser_flow;
mod provider_bridge;

use std::{
    collections::BTreeMap,
    env, fs,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use futures_core::Stream;
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use browser_flow::{
    BROWSER_FLOW_SCHEMA_VERSION, BoundBrowserFlowPlan, BrowserFlow, BrowserFlowBindingContext,
    BrowserFlowCatalog, BrowserFlowCatalogCollection, BrowserFlowCatalogDirectoryEntry,
    BrowserFlowElement, BrowserFlowExecutionMode, BrowserFlowExecutionReport,
    BrowserFlowExecutionStepReport, BrowserFlowExecutionStepStatus, BrowserFlowExecutor,
    BrowserFlowHeaderMatcher, BrowserFlowInput, BrowserFlowInputKind, BrowserFlowOperation,
    BrowserFlowOperationKind, BrowserFlowOutput, BrowserFlowOutputKind, BrowserFlowPage,
    BrowserFlowRequest, BrowserFlowSelector, BrowserFlowSelectorEngine, BrowserFlowSession,
    BrowserFlowSessionExecutor, BrowserFlowStep, BrowserFlowVisualCaptchaRequest,
    BrowserFlowVisualLayoutTarget, BrowserFlowVisualLayoutValidationRequest,
    BrowserFlowVisualLayoutValidationTargetRequest, DryRunBrowserFlowExecutor,
};
pub use provider_bridge::{
    PROVIDER_BRIDGE_SCHEMA_VERSION, ProviderBridgeBrowserProfileBinding, ProviderBridgeCatalog,
    ProviderBridgeCatalogCollection, ProviderBridgeCatalogDirectoryEntry,
    ProviderBridgeCredentialMapping, ProviderBridgeLoggedInProbe,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
    pub multipart_upload: bool,
    #[serde(default)]
    pub streaming_get: bool,
    #[serde(default)]
    pub streaming_put: bool,
    #[serde(default)]
    pub max_single_upload_bytes: Option<u64>,
    #[serde(default)]
    pub max_single_download_bytes: Option<u64>,
    #[serde(default)]
    pub upload_part_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub backend: String,
    pub status: HealthStatus,
    pub capabilities: BackendCapabilities,
    #[serde(default)]
    pub scopes: Vec<StorageScopeHealth>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageScopeKind {
    Personal,
    Family,
    Shared,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageCapacity {
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageScopeHealth {
    pub id: String,
    pub label: String,
    pub kind: StorageScopeKind,
    pub writable: bool,
    pub root: Option<String>,
    pub container: Option<String>,
    pub object_count: Option<u64>,
    pub capacity: Option<StorageCapacity>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub object_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListObjectsRequest {
    pub container: Option<String>,
    pub prefix: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserRequestProfile {
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub captured_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl BrowserRequestProfile {
    pub fn normalize(mut self) -> Self {
        self.source_url = normalize_optional_string(self.source_url);
        self.user_agent = normalize_optional_string(self.user_agent);
        self.headers = self
            .headers
            .into_iter()
            .filter_map(|(name, value)| {
                let normalized_name = normalize_header_name(name.as_str())?;
                let normalized_value = normalize_optional_string(Some(value))?;
                Some((normalized_name, normalized_value))
            })
            .collect();
        if self.user_agent.is_none() {
            self.user_agent = self.header("user-agent").map(ToString::to_string);
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none() && self.headers.is_empty() && self.source_url.is_none()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let normalized = normalize_header_name(name)?;
        self.headers.get(normalized.as_str()).map(String::as_str)
    }

    pub fn effective_user_agent(&self) -> Option<&str> {
        self.user_agent
            .as_deref()
            .or_else(|| self.header("user-agent"))
    }

    pub fn forwarded_headers(&self, blocked_names: &[&str]) -> Vec<(String, String)> {
        self.headers
            .iter()
            .filter(|(name, _)| should_forward_browser_profile_header(name.as_str(), blocked_names))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_header_name(name: &str) -> Option<String> {
    let trimmed = name.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn should_forward_browser_profile_header(name: &str, blocked_names: &[&str]) -> bool {
    let Some(normalized) = normalize_header_name(name) else {
        return false;
    };
    let default_blocked = [
        "accept-encoding",
        "authorization",
        "browser-id",
        "connection",
        "content-length",
        "cookie",
        "host",
        "proxy-authorization",
        "signature",
        "timestamp",
        "transfer-encoding",
    ];
    if default_blocked.iter().any(|item| *item == normalized) {
        return false;
    }
    !blocked_names.iter().any(|item| {
        normalize_header_name(item)
            .as_deref()
            .is_some_and(|blocked| blocked == normalized)
    })
}

pub struct ObjectPayload {
    pub info: ObjectInfo,
    pub body: ObjectBody,
    pub first_response_latency_ms: Option<u64>,
}

pub trait BodySpoolLease: Send + Sync + std::fmt::Debug {
    fn update_tracked_bytes(&mut self, next_bytes: u64);
}

pub trait BodySpoolObserver: Send + Sync + std::fmt::Debug {
    fn start_tracking(&self) -> Box<dyn BodySpoolLease>;
}

pub type SharedBodySpoolObserver = Arc<dyn BodySpoolObserver>;

pub struct PutObjectRequest {
    pub container: String,
    pub key: String,
    pub body: ObjectBody,
    pub size: Option<u64>,
    pub content_type: Option<String>,
    pub preferred_upload_part_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PutObjectResult {
    pub etag: Option<String>,
    #[serde(default)]
    pub first_response_latency_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameObjectRequest {
    pub container: String,
    pub key: String,
    pub new_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyObjectRequest {
    pub source_container: String,
    pub source_key: String,
    pub destination_container: String,
    pub destination_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveObjectRequest {
    pub source_container: String,
    pub source_key: String,
    pub destination_container: String,
    pub destination_key: String,
}

#[derive(Debug, Error)]
pub enum BlobError {
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("interactive input required: {0}")]
    InteractiveInputRequired(String),
    #[error("feature not implemented: {0}")]
    NotImplemented(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("body stream error: {0}")]
    BodyStream(String),
}

pub type ObjectBodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, BlobError>> + Send + 'static>>;

pub struct ObjectBody {
    inner: ObjectBodyStream,
}

#[derive(Clone)]
pub struct StreamFirstProgressObserver {
    triggered: Arc<AtomicBool>,
    callback: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for ObjectBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObjectBody(<stream>)")
    }
}

impl std::fmt::Debug for StreamFirstProgressObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamFirstProgressObserver(<callback>)")
    }
}

impl ObjectBody {
    pub fn from_bytes(bytes: Bytes) -> Self {
        Self {
            inner: Box::pin(futures_util::stream::once(async move { Ok(bytes) })),
        }
    }

    pub fn from_stream<S>(stream: S) -> Self
    where
        S: Stream<Item = Result<Bytes, BlobError>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    pub async fn collect(self) -> Result<Bytes, BlobError> {
        let chunks = self.inner.try_collect::<Vec<_>>().await?;
        let total_len = chunks.iter().map(Bytes::len).sum();
        let mut buffer = Vec::with_capacity(total_len);
        for chunk in chunks {
            buffer.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(buffer))
    }

    pub fn into_stream(self) -> ObjectBodyStream {
        self.inner
    }

    pub fn observe_first_progress(self, observer: StreamFirstProgressObserver) -> Self {
        Self::from_stream(futures_util::stream::unfold(
            (self.into_stream(), observer),
            |(mut inner, observer)| async move {
                inner.next().await.map(|item| {
                    let item = item.map(|chunk| {
                        if !chunk.is_empty() {
                            observer.notify();
                        }
                        chunk
                    });
                    (item, (inner, observer))
                })
            },
        ))
    }
}

impl StreamFirstProgressObserver {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self {
            triggered: Arc::new(AtomicBool::new(false)),
            callback: Arc::new(callback),
        }
    }

    pub fn notify(&self) {
        if !self.triggered.swap(true, Ordering::SeqCst) {
            (self.callback)();
        }
    }
}

impl From<Bytes> for ObjectBody {
    fn from(value: Bytes) -> Self {
        Self::from_bytes(value)
    }
}

impl From<Vec<u8>> for ObjectBody {
    fn from(value: Vec<u8>) -> Self {
        Self::from_bytes(Bytes::from(value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenSource {
    EnvVar { key: String },
    File { path: String },
    Static { bearer: String },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundIpFamily {
    #[default]
    Auto,
    Ipv4,
    Ipv6,
}

impl TokenSource {
    pub fn describe(&self) -> &'static str {
        match self {
            Self::EnvVar { .. } => "env",
            Self::File { .. } => "file",
            Self::Static { .. } => "inline",
        }
    }

    pub fn load(&self) -> Result<String, BlobError> {
        match self {
            Self::EnvVar { key } => env::var(key).map_err(|_| {
                BlobError::Configuration(format!("missing environment variable: {key}"))
            }),
            Self::File { path } => fs::read_to_string(path)
                .map(|value| value.trim().to_string())
                .map_err(|error| {
                    BlobError::Configuration(format!("failed to read token file {path}: {error}"))
                }),
            Self::Static { bearer } => {
                if bearer.trim().is_empty() {
                    Err(BlobError::Configuration(
                        "inline token is empty".to_string(),
                    ))
                } else {
                    Ok(bearer.trim().to_string())
                }
            }
        }
    }
}

impl OutboundIpFamily {
    pub fn parse(raw: &str) -> Result<Self, BlobError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "ipv4" => Ok(Self::Ipv4),
            "ipv6" => Ok(Self::Ipv6),
            other => Err(BlobError::Configuration(format!(
                "unsupported outbound IP family: {other}; expected auto, ipv4, or ipv6"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }

    pub fn local_address(self) -> Option<IpAddr> {
        match self {
            Self::Auto => None,
            Self::Ipv4 => Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            Self::Ipv6 => Some(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        }
    }
}

#[async_trait]
pub trait BlobBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> BackendCapabilities;

    async fn health(&self) -> Result<ServiceHealth, BlobError>;
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError>;
    async fn list_objects(&self, request: ListObjectsRequest)
    -> Result<Vec<ObjectInfo>, BlobError>;

    async fn head_container(&self, name: &str) -> Result<ContainerInfo, BlobError> {
        self.list_containers()
            .await?
            .into_iter()
            .find(|container| container.name == name)
            .ok_or_else(|| BlobError::NotFound(format!("container not found: {name}")))
    }

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        self.list_objects(ListObjectsRequest {
            container: Some(container.to_string()),
            prefix: Some(key.to_string()),
            limit: None,
        })
        .await?
        .into_iter()
        .find(|object| object.key == key)
        .ok_or_else(|| BlobError::NotFound(format!("object not found: {container}/{key}")))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        Err(BlobError::NotImplemented(format!(
            "get_object not implemented for {container}/{key}"
        )))
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        Err(BlobError::NotImplemented(format!(
            "put_object not implemented for {}/{}",
            request.container, request.key
        )))
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        Err(BlobError::NotImplemented(format!(
            "delete_object not implemented for {container}/{key}"
        )))
    }

    async fn rename_object(&self, request: RenameObjectRequest) -> Result<(), BlobError> {
        Err(BlobError::NotImplemented(format!(
            "rename_object not implemented for {}/{} -> {}",
            request.container, request.key, request.new_key
        )))
    }

    async fn copy_object(&self, request: CopyObjectRequest) -> Result<(), BlobError> {
        Err(BlobError::NotImplemented(format!(
            "copy_object not implemented for {}/{} -> {}/{}",
            request.source_container,
            request.source_key,
            request.destination_container,
            request.destination_key
        )))
    }

    async fn move_object(&self, request: MoveObjectRequest) -> Result<(), BlobError> {
        Err(BlobError::NotImplemented(format!(
            "move_object not implemented for {}/{} -> {}/{}",
            request.source_container,
            request.source_key,
            request.destination_container,
            request.destination_key
        )))
    }
}

struct StubObject {
    body: Bytes,
    etag: String,
    content_type: Option<String>,
    last_modified: String,
}

pub struct StubBackend {
    objects: Mutex<BTreeMap<String, BTreeMap<String, StubObject>>>,
}

impl StubBackend {
    pub fn new() -> Self {
        let mut buckets = BTreeMap::new();
        let mut objects = BTreeMap::new();
        let body = Bytes::from_static(b"stub object from carrier-cloud-blob-gateway\n");
        objects.insert(
            "example.txt".to_string(),
            StubObject {
                etag: calculate_stub_etag(&body),
                body,
                content_type: Some("text/plain".to_string()),
                last_modified: "2026-01-01T00:00:00.000Z".to_string(),
            },
        );
        buckets.insert("placeholder".to_string(), objects);

        Self {
            objects: Mutex::new(buckets),
        }
    }
}

#[async_trait]
impl BlobBackend for StubBackend {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            read: true,
            write: true,
            delete: true,
            multipart_upload: false,
            streaming_get: false,
            streaming_put: false,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            upload_part_size_bytes: None,
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
        Ok(ServiceHealth {
            backend: self.name().to_string(),
            status: HealthStatus::Degraded,
            capabilities: self.capabilities(),
            scopes: vec![StorageScopeHealth {
                id: "local".to_string(),
                label: "Local Stub".to_string(),
                kind: StorageScopeKind::Unknown,
                writable: true,
                root: Some("placeholder".to_string()),
                container: Some("placeholder".to_string()),
                object_count: Some(1),
                capacity: None,
                notes: vec!["in-memory development backend".to_string()],
            }],
            notes: vec!["stub backend enabled; no upstream attached".to_string()],
        })
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
        let objects = self.objects.lock().expect("stub backend state poisoned");

        Ok(objects
            .iter()
            .map(|(name, entries)| ContainerInfo {
                name: name.clone(),
                object_count: Some(entries.len() as u64),
            })
            .collect())
    }

    async fn list_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        let Some(container) = request.container else {
            return Ok(Vec::new());
        };

        let objects = self.objects.lock().expect("stub backend state poisoned");
        let Some(entries) = objects.get(&container) else {
            return Err(BlobError::NotFound(format!(
                "container not found: {container}"
            )));
        };

        let prefix = request.prefix.unwrap_or_default();
        let limit = request.limit.unwrap_or(usize::MAX);

        Ok(entries
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .take(limit)
            .map(|(key, object)| ObjectInfo {
                key: key.clone(),
                size: object.body.len() as u64,
                etag: Some(object.etag.clone()),
                content_type: object.content_type.clone(),
                last_modified: Some(object.last_modified.clone()),
            })
            .collect())
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        let objects = self.objects.lock().expect("stub backend state poisoned");
        let Some(entries) = objects.get(container) else {
            return Err(BlobError::NotFound(format!(
                "container not found: {container}"
            )));
        };
        let Some(object) = entries.get(key) else {
            return Err(BlobError::NotFound(format!(
                "object not found: {container}/{key}"
            )));
        };

        Ok(ObjectPayload {
            info: ObjectInfo {
                key: key.to_string(),
                size: object.body.len() as u64,
                etag: Some(object.etag.clone()),
                content_type: object.content_type.clone(),
                last_modified: Some(object.last_modified.clone()),
            },
            body: ObjectBody::from_bytes(object.body.clone()),
            first_response_latency_ms: Some(0),
        })
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        let body = request.body.collect().await?;
        let mut objects = self.objects.lock().expect("stub backend state poisoned");
        let entries = objects.entry(request.container).or_default();
        let etag = calculate_stub_etag(&body);

        entries.insert(
            request.key,
            StubObject {
                etag: etag.clone(),
                body,
                content_type: request.content_type,
                last_modified: "2026-01-01T00:00:00.000Z".to_string(),
            },
        );

        Ok(PutObjectResult {
            etag: Some(etag),
            first_response_latency_ms: Some(0),
        })
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        let mut objects = self.objects.lock().expect("stub backend state poisoned");
        let Some(entries) = objects.get_mut(container) else {
            return Err(BlobError::NotFound(format!(
                "container not found: {container}"
            )));
        };

        if entries.remove(key).is_some() {
            Ok(())
        } else {
            Err(BlobError::NotFound(format!(
                "object not found: {container}/{key}"
            )))
        }
    }

    async fn rename_object(&self, request: RenameObjectRequest) -> Result<(), BlobError> {
        let mut objects = self.objects.lock().expect("stub backend state poisoned");
        let Some(entries) = objects.get_mut(&request.container) else {
            return Err(BlobError::NotFound(format!(
                "container not found: {}",
                request.container
            )));
        };
        let Some(object) = entries.remove(&request.key) else {
            return Err(BlobError::NotFound(format!(
                "object not found: {}/{}",
                request.container, request.key
            )));
        };
        entries.insert(request.new_key, object);
        Ok(())
    }

    async fn copy_object(&self, request: CopyObjectRequest) -> Result<(), BlobError> {
        let mut objects = self.objects.lock().expect("stub backend state poisoned");
        let source_entries = objects.get(&request.source_container).ok_or_else(|| {
            BlobError::NotFound(format!("container not found: {}", request.source_container))
        })?;
        let source = source_entries.get(&request.source_key).ok_or_else(|| {
            BlobError::NotFound(format!(
                "object not found: {}/{}",
                request.source_container, request.source_key
            ))
        })?;
        let cloned = StubObject {
            body: source.body.clone(),
            etag: source.etag.clone(),
            content_type: source.content_type.clone(),
            last_modified: source.last_modified.clone(),
        };
        let destination_entries = objects.entry(request.destination_container).or_default();
        destination_entries.insert(request.destination_key, cloned);
        Ok(())
    }

    async fn move_object(&self, request: MoveObjectRequest) -> Result<(), BlobError> {
        self.copy_object(CopyObjectRequest {
            source_container: request.source_container.clone(),
            source_key: request.source_key.clone(),
            destination_container: request.destination_container.clone(),
            destination_key: request.destination_key.clone(),
        })
        .await?;
        self.delete_object(&request.source_container, &request.source_key)
            .await
    }
}

fn calculate_stub_etag(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |acc, byte| {
        (acc ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });

    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::TokenSource;

    #[test]
    fn inline_token_is_trimmed() {
        let token = TokenSource::Static {
            bearer: "  token-value  ".to_string(),
        }
        .load()
        .expect("inline token should load");

        assert_eq!(token, "token-value");
    }
}
