use std::time::Duration;

use async_trait::async_trait;
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, ContainerInfo, HealthStatus, ListObjectsRequest,
    ObjectInfo, ObjectPayload, PutObjectRequest, PutObjectResult, ServiceHealth, TokenSource,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{
    Method, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, USER_AGENT},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneDriveConfig {
    pub enabled: bool,
    pub tenant: String,
    pub client_id: Option<String>,
    pub use_device_code: bool,
    pub redirect_url: Option<String>,
    pub drive_id: Option<String>,
    pub graph_base_url: String,
    pub token_source: TokenSource,
    pub root_prefix: Option<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
}

pub struct OneDriveBlobAdapter {
    config: OneDriveConfig,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
struct DriveItemResponse {
    id: String,
    name: Option<String>,
    size: Option<u64>,
    #[serde(rename = "eTag")]
    etag: Option<String>,
    #[serde(rename = "lastModifiedDateTime")]
    last_modified: Option<String>,
    file: Option<FileFacet>,
    folder: Option<FolderFacet>,
}

#[derive(Debug, Clone, Deserialize)]
struct FileFacet {
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FolderFacet {
    #[serde(rename = "childCount")]
    #[allow(dead_code)]
    child_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DriveItemCollection {
    value: Vec<DriveItemResponse>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

impl OneDriveBlobAdapter {
    pub fn new(config: OneDriveConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    fn ensure_enabled(&self) -> Result<(), BlobError> {
        if self.config.enabled {
            Ok(())
        } else {
            Err(BlobError::Configuration(
                "onedrive provider disabled".to_string(),
            ))
        }
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.request_timeout_secs.max(1))
    }

    fn drive_resource(&self) -> String {
        match self.config.drive_id.as_deref().map(str::trim) {
            Some(value) if !value.is_empty() => {
                format!("drives/{}", encode_segment(value))
            }
            _ => "me/drive".to_string(),
        }
    }

    fn graph_base_url(&self) -> &str {
        self.config.graph_base_url.trim_end_matches('/')
    }

    fn drive_root_url(&self) -> String {
        format!("{}/{}/root", self.graph_base_url(), self.drive_resource())
    }

    fn item_url(&self, item_id: &str) -> String {
        format!(
            "{}/{}/items/{}",
            self.graph_base_url(),
            self.drive_resource(),
            encode_segment(item_id)
        )
    }

    fn item_children_url(&self, item_id: &str) -> String {
        format!("{}/children", self.item_url(item_id))
    }

    fn item_content_url(&self, item_id: &str) -> String {
        format!("{}/content", self.item_url(item_id))
    }

    fn path_item_url(&self, path: &str) -> String {
        let normalized = normalize_path(path);
        if normalized.is_empty() {
            self.drive_root_url()
        } else {
            format!(
                "{}/{}/root:/{}:",
                self.graph_base_url(),
                self.drive_resource(),
                encode_path(&normalized)
            )
        }
    }

    fn path_upload_url(&self, path: &str) -> String {
        format!("{}/content", self.path_item_url(path))
    }

    fn normalized_root_prefix(&self) -> Option<String> {
        self.config
            .root_prefix
            .as_deref()
            .map(normalize_path)
            .filter(|value| !value.is_empty())
    }

    fn container_path(&self, container: &str) -> Result<String, BlobError> {
        ensure_non_empty(container, "container")?;

        let mut parts = Vec::new();
        if let Some(prefix) = self.normalized_root_prefix() {
            parts.push(prefix);
        }
        parts.push(normalize_path(container));

        Ok(parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/"))
    }

    fn object_path(&self, container: &str, key: &str) -> Result<String, BlobError> {
        ensure_non_empty(container, "container")?;
        ensure_non_empty(key, "object key")?;

        let mut parts = Vec::new();
        if let Some(prefix) = self.normalized_root_prefix() {
            parts.push(prefix);
        }
        parts.push(normalize_path(container));
        parts.push(normalize_path(key));

        Ok(parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/"))
    }

    fn request(&self, method: Method, url: &str) -> Result<reqwest::RequestBuilder, BlobError> {
        let token = self.config.token_source.load()?;

        Ok(self
            .client
            .request(method, url)
            .bearer_auth(token)
            .header(USER_AGENT, self.config.user_agent.as_str())
            .timeout(self.timeout()))
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str, action: &str) -> Result<T, BlobError> {
        let response = self
            .request(Method::GET, url)?
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response.json::<T>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn get_json_optional<T: DeserializeOwned>(
        &self,
        url: &str,
        action: &str,
    ) -> Result<Option<T>, BlobError> {
        let response = self
            .request(Method::GET, url)?
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response.json::<T>().await.map(Some).map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn put_bytes(
        &self,
        url: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
        action: &str,
    ) -> Result<DriveItemResponse, BlobError> {
        let response = self
            .request(Method::PUT, url)?
            .header(
                CONTENT_TYPE,
                content_type.unwrap_or("application/octet-stream"),
            )
            .body(body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response.json::<DriveItemResponse>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn get_bytes(&self, url: &str, action: &str) -> Result<Vec<u8>, BlobError> {
        let response = self
            .request(Method::GET, url)?
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response
            .bytes()
            .await
            .map(|body| body.to_vec())
            .map_err(|error| {
                BlobError::Upstream(format!("{action} returned invalid bytes: {error}"))
            })
    }

    async fn delete_item_by_id(&self, item_id: &str, action: &str) -> Result<(), BlobError> {
        let response = self
            .request(Method::DELETE, &self.item_url(item_id))?
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => Err(BlobError::NotFound(action.to_string())),
            _ => Err(response_to_error(response, action).await),
        }
    }

    async fn get_root_item(&self) -> Result<DriveItemResponse, BlobError> {
        self.get_json(&self.drive_root_url(), "probe OneDrive root")
            .await
    }

    async fn get_item_by_path(&self, path: &str) -> Result<Option<DriveItemResponse>, BlobError> {
        let normalized = normalize_path(path);
        let action = if normalized.is_empty() {
            "fetch OneDrive root".to_string()
        } else {
            format!("fetch OneDrive item {normalized}")
        };

        self.get_json_optional(&self.path_item_url(&normalized), &action)
            .await
    }

    async fn list_children_by_id(
        &self,
        item_id: &str,
    ) -> Result<Vec<DriveItemResponse>, BlobError> {
        let mut next_link = Some(self.item_children_url(item_id));
        let mut items = Vec::new();

        while let Some(url) = next_link.take() {
            let page: DriveItemCollection = self
                .get_json(&url, &format!("list OneDrive children for {item_id}"))
                .await?;
            items.extend(page.value);
            next_link = page.next_link;
        }

        Ok(items)
    }

    async fn resolve_container_folder(
        &self,
        container: &str,
    ) -> Result<DriveItemResponse, BlobError> {
        let path = self.container_path(container)?;
        let item = self
            .get_item_by_path(&path)
            .await?
            .ok_or_else(|| BlobError::NotFound(format!("container not found: {container}")))?;
        ensure_folder(&item, &format!("container {container}"))?;
        Ok(item)
    }

    async fn resolve_object(
        &self,
        container: &str,
        key: &str,
    ) -> Result<DriveItemResponse, BlobError> {
        let path = self.object_path(container, key)?;
        let item = self
            .get_item_by_path(&path)
            .await?
            .ok_or_else(|| BlobError::NotFound(format!("object not found: {container}/{key}")))?;
        ensure_file(&item, &format!("object {container}/{key}"))?;
        Ok(item)
    }

    async fn ensure_folder_tree(&self, folder_path: &str) -> Result<(), BlobError> {
        let normalized = normalize_path(folder_path);
        if normalized.is_empty() {
            return Ok(());
        }

        let mut parent_id = self.get_root_item().await?.id;
        let mut current_segments = Vec::new();

        for segment in normalized.split('/') {
            current_segments.push(segment);
            let current_path = current_segments.join("/");

            match self.get_item_by_path(&current_path).await? {
                Some(item) => {
                    ensure_folder(&item, &format!("folder path {current_path}"))?;
                    parent_id = item.id;
                }
                None => {
                    let created = self
                        .create_folder(&parent_id, segment, &current_path)
                        .await?;
                    parent_id = created.id;
                }
            }
        }

        Ok(())
    }

    async fn create_folder(
        &self,
        parent_id: &str,
        name: &str,
        full_path: &str,
    ) -> Result<DriveItemResponse, BlobError> {
        let action = format!("create OneDrive folder {full_path}");
        let url = self.item_children_url(parent_id);
        let body = json!({
            "name": name,
            "folder": {},
            "@microsoft.graph.conflictBehavior": "fail",
        });

        let response = self
            .request(Method::POST, &url)?
            .header(ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::CONFLICT {
            let item = self.get_item_by_path(full_path).await?.ok_or_else(|| {
                BlobError::Upstream(format!("{action} conflicted but no item was found"))
            })?;
            ensure_folder(&item, &format!("folder path {full_path}"))?;
            return Ok(item);
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, &action).await);
        }

        response.json::<DriveItemResponse>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn health_status(&self, notes: &mut Vec<String>) -> HealthStatus {
        if !self.config.enabled {
            notes.push("onedrive provider disabled".to_string());
            return HealthStatus::Unavailable;
        }

        if let Err(error) = self.config.token_source.load() {
            notes.push(error.to_string());
            return HealthStatus::Unavailable;
        }

        if let Err(error) = self.get_root_item().await {
            notes.push(error.to_string());
            return HealthStatus::Unavailable;
        }

        match self.normalized_root_prefix() {
            Some(prefix) => match self.get_item_by_path(&prefix).await {
                Ok(Some(item)) => {
                    if let Err(error) = ensure_folder(&item, &format!("root_prefix {prefix}")) {
                        notes.push(error.to_string());
                        HealthStatus::Unavailable
                    } else {
                        notes.push(format!("root_prefix_ready={prefix}"));
                        HealthStatus::Healthy
                    }
                }
                Ok(None) => {
                    notes.push(format!(
                        "root_prefix_missing={prefix}; it will be created on first write"
                    ));
                    HealthStatus::Degraded
                }
                Err(error) => {
                    notes.push(error.to_string());
                    HealthStatus::Unavailable
                }
            },
            None => HealthStatus::Healthy,
        }
    }
}

#[async_trait]
impl BlobBackend for OneDriveBlobAdapter {
    fn name(&self) -> &'static str {
        "onedrive"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            read: true,
            write: true,
            delete: true,
            multipart_upload: false,
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
        let mut notes = vec![
            format!("graph_base_url={}", self.config.graph_base_url),
            format!("auth_source={}", self.config.token_source.describe()),
            format!(
                "drive_selector={}",
                self.config
                    .drive_id
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| format!("drives/{value}"))
                    .unwrap_or_else(|| "me/drive".to_string())
            ),
            format!(
                "oauth_mode={}",
                if self.config.use_device_code {
                    "device_code"
                } else {
                    "web_callback"
                }
            ),
            format!(
                "root_prefix={}",
                self.normalized_root_prefix()
                    .unwrap_or_else(|| "<drive-root>".to_string())
            ),
        ];

        if let Some(client_id) = self
            .config
            .client_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            notes.push(format!("client_id={client_id}"));
        }

        if let Some(redirect_url) = self
            .config
            .redirect_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            notes.push(format!("redirect_url={redirect_url}"));
        }

        let status = self.health_status(&mut notes).await;

        Ok(ServiceHealth {
            backend: self.name().to_string(),
            status,
            capabilities: self.capabilities(),
            notes,
        })
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
        self.ensure_enabled()?;

        let base_item = match self.normalized_root_prefix() {
            Some(prefix) => match self.get_item_by_path(&prefix).await? {
                Some(item) => {
                    ensure_folder(&item, &format!("root_prefix {prefix}"))?;
                    item
                }
                None => return Ok(Vec::new()),
            },
            None => self.get_root_item().await?,
        };

        let mut containers = self
            .list_children_by_id(&base_item.id)
            .await?
            .into_iter()
            .filter(|item| item.folder.is_some())
            .filter_map(|item| {
                item.name.map(|name| ContainerInfo {
                    name,
                    object_count: None,
                })
            })
            .collect::<Vec<_>>();

        containers.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(containers)
    }

    async fn list_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        self.ensure_enabled()?;

        let Some(container) = request.container.as_deref() else {
            return Ok(Vec::new());
        };

        let prefix = request.prefix.as_deref().map(normalize_path);
        let mut objects = Vec::new();
        let container_folder = self.resolve_container_folder(container).await?;
        let mut stack = vec![(container_folder.id, String::new())];

        while let Some((folder_id, folder_prefix)) = stack.pop() {
            for item in self.list_children_by_id(&folder_id).await? {
                let child_name = item.name.clone().ok_or_else(|| {
                    BlobError::Upstream("OneDrive item is missing a name".to_string())
                })?;
                let object_key = join_relative_key(&folder_prefix, &child_name);

                if item.folder.is_some() {
                    stack.push((item.id, object_key));
                    continue;
                }

                if item.file.is_some()
                    && prefix
                        .as_deref()
                        .is_none_or(|value| object_key.starts_with(value))
                {
                    objects.push(item.into_object_info(object_key));
                }
            }
        }

        objects.sort_by(|left, right| left.key.cmp(&right.key));

        if let Some(limit) = request.limit {
            objects.truncate(limit);
        }

        Ok(objects)
    }

    async fn head_container(&self, name: &str) -> Result<ContainerInfo, BlobError> {
        self.ensure_enabled()?;
        self.resolve_container_folder(name).await?;
        Ok(ContainerInfo {
            name: name.to_string(),
            object_count: None,
        })
    }

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        self.ensure_enabled()?;
        Ok(self
            .resolve_object(container, key)
            .await?
            .into_object_info(normalize_path(key)))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        self.ensure_enabled()?;

        let item = self.resolve_object(container, key).await?;
        let body = self
            .get_bytes(
                &self.item_content_url(&item.id),
                &format!("download object {container}/{key}"),
            )
            .await?;

        Ok(ObjectPayload {
            info: item.into_object_info(normalize_path(key)),
            body,
        })
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        self.ensure_enabled()?;

        let object_path = self.object_path(&request.container, &request.key)?;
        if let Some(parent_path) = parent_path(&object_path) {
            self.ensure_folder_tree(&parent_path).await?;
        }

        let uploaded = self
            .put_bytes(
                &self.path_upload_url(&object_path),
                request.body,
                request.content_type.as_deref(),
                &format!("upload object {}/{}", request.container, request.key),
            )
            .await?;

        Ok(PutObjectResult {
            etag: uploaded.etag,
        })
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        self.ensure_enabled()?;

        let item = self.resolve_object(container, key).await?;
        self.delete_item_by_id(&item.id, &format!("delete object {container}/{key}"))
            .await
    }
}

impl DriveItemResponse {
    fn into_object_info(self, key: String) -> ObjectInfo {
        ObjectInfo {
            key,
            size: self.size.unwrap_or(0),
            etag: self.etag,
            content_type: self.file.and_then(|value| value.mime_type),
            last_modified: self.last_modified,
        }
    }
}

fn ensure_non_empty(value: &str, label: &str) -> Result<(), BlobError> {
    if value.trim().is_empty() {
        Err(BlobError::Configuration(format!("{label} cannot be empty")))
    } else {
        Ok(())
    }
}

fn ensure_folder(item: &DriveItemResponse, label: &str) -> Result<(), BlobError> {
    if item.folder.is_some() {
        Ok(())
    } else {
        Err(BlobError::Upstream(format!("{label} is not a folder")))
    }
}

fn ensure_file(item: &DriveItemResponse, label: &str) -> Result<(), BlobError> {
    if item.file.is_some() {
        Ok(())
    } else {
        Err(BlobError::NotFound(format!("{label} is not a file")))
    }
}

fn normalize_path(value: &str) -> String {
    value
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn parent_path(path: &str) -> Option<String> {
    let mut segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments.pop()?;

    if segments.is_empty() {
        None
    } else {
        Some(segments.join("/"))
    }
}

fn encode_segment(segment: &str) -> String {
    utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string()
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn join_relative_key(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn trim_response_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty response body".to_string()
    } else {
        let mut shortened = trimmed.chars().take(240).collect::<String>();
        if trimmed.chars().count() > 240 {
            shortened.push_str("...");
        }
        shortened
    }
}

async fn response_to_error(response: reqwest::Response, action: &str) -> BlobError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    BlobError::Upstream(format!(
        "{action} failed with {status}: {}",
        trim_response_body(&body)
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use axum::{
        Json, Router,
        body::Bytes,
        extract::{OriginalUri, State},
        http::{HeaderMap, Method, StatusCode},
        response::{IntoResponse, Response},
        routing::get,
    };
    use percent_encoding::percent_decode_str;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Clone)]
    struct MockGraphState {
        drive: Arc<Mutex<MockDrive>>,
    }

    struct MockDrive {
        items_by_id: BTreeMap<String, MockItem>,
        path_to_id: BTreeMap<String, String>,
        next_id: usize,
    }

    #[derive(Clone)]
    struct MockItem {
        id: String,
        path: String,
        name: String,
        is_folder: bool,
        body: Vec<u8>,
        content_type: Option<String>,
        etag: String,
        last_modified: String,
    }

    struct MockServer {
        base_url: String,
        _task: tokio::task::JoinHandle<()>,
    }

    impl MockDrive {
        fn new() -> Self {
            let root = MockItem {
                id: "root".to_string(),
                path: String::new(),
                name: "root".to_string(),
                is_folder: true,
                body: Vec::new(),
                content_type: None,
                etag: "root".to_string(),
                last_modified: "2026-04-25T00:00:00Z".to_string(),
            };

            let mut items_by_id = BTreeMap::new();
            items_by_id.insert(root.id.clone(), root.clone());

            let mut path_to_id = BTreeMap::new();
            path_to_id.insert(String::new(), root.id.clone());

            Self {
                items_by_id,
                path_to_id,
                next_id: 1,
            }
        }

        fn item_json(&self, item: &MockItem) -> Value {
            let mut value = json!({
                "id": item.id,
                "name": item.name,
                "size": if item.is_folder { 0 } else { item.body.len() as u64 },
                "eTag": item.etag,
                "lastModifiedDateTime": item.last_modified,
            });

            if item.is_folder {
                value["folder"] = json!({
                    "childCount": self.child_ids(&item.id).len() as u64,
                });
            } else {
                value["file"] = json!({
                    "mimeType": item.content_type.clone().unwrap_or_else(|| "application/octet-stream".to_string()),
                });
            }

            value
        }

        fn child_ids(&self, parent_id: &str) -> Vec<String> {
            let Some(parent) = self.items_by_id.get(parent_id) else {
                return Vec::new();
            };

            let prefix = if parent.path.is_empty() {
                String::new()
            } else {
                format!("{}/", parent.path)
            };
            let depth = if parent.path.is_empty() {
                1
            } else {
                parent.path.split('/').count() + 1
            };

            self.items_by_id
                .values()
                .filter(|item| item.path != parent.path)
                .filter(|item| {
                    if parent.path.is_empty() {
                        !item.path.contains('/')
                    } else {
                        item.path.starts_with(&prefix) && item.path.split('/').count() == depth
                    }
                })
                .map(|item| item.id.clone())
                .collect()
        }

        fn item_by_path(&self, path: &str) -> Option<MockItem> {
            self.path_to_id
                .get(path)
                .and_then(|id| self.items_by_id.get(id))
                .cloned()
        }

        fn create_folder(&mut self, parent_id: &str, name: &str) -> Result<MockItem, StatusCode> {
            let parent = self
                .items_by_id
                .get(parent_id)
                .cloned()
                .ok_or(StatusCode::NOT_FOUND)?;
            if !parent.is_folder {
                return Err(StatusCode::BAD_REQUEST);
            }

            let path = join_relative_key(&parent.path, name);
            if self.path_to_id.contains_key(&path) {
                return Err(StatusCode::CONFLICT);
            }

            let item = MockItem {
                id: format!("item-{}", self.next_id),
                path: path.clone(),
                name: name.to_string(),
                is_folder: true,
                body: Vec::new(),
                content_type: None,
                etag: format!("etag-folder-{}", self.next_id),
                last_modified: "2026-04-25T00:00:00Z".to_string(),
            };
            self.next_id += 1;
            self.path_to_id.insert(path, item.id.clone());
            self.items_by_id.insert(item.id.clone(), item.clone());
            Ok(item)
        }

        fn put_file(
            &mut self,
            path: &str,
            body: Vec<u8>,
            content_type: Option<String>,
        ) -> Result<MockItem, StatusCode> {
            let Some(parent) = parent_path(path) else {
                return Err(StatusCode::BAD_REQUEST);
            };

            let Some(parent_item) = self.item_by_path(&parent) else {
                return Err(StatusCode::NOT_FOUND);
            };

            if !parent_item.is_folder {
                return Err(StatusCode::BAD_REQUEST);
            }

            let name = path
                .split('/')
                .next_back()
                .expect("normalized path should contain a file name")
                .to_string();

            if let Some(existing_id) = self.path_to_id.get(path).cloned() {
                let existing = self
                    .items_by_id
                    .get_mut(&existing_id)
                    .expect("existing path index should remain valid");
                existing.is_folder = false;
                existing.body = body;
                existing.content_type = content_type;
                existing.etag = format!("etag-file-{}", self.next_id);
                existing.last_modified = "2026-04-25T00:00:00Z".to_string();
                self.next_id += 1;
                return Ok(existing.clone());
            }

            let item = MockItem {
                id: format!("item-{}", self.next_id),
                path: path.to_string(),
                name,
                is_folder: false,
                body,
                content_type,
                etag: format!("etag-file-{}", self.next_id),
                last_modified: "2026-04-25T00:00:00Z".to_string(),
            };
            self.next_id += 1;
            self.path_to_id.insert(item.path.clone(), item.id.clone());
            self.items_by_id.insert(item.id.clone(), item.clone());
            Ok(item)
        }

        fn delete_item(&mut self, item_id: &str) -> Result<(), StatusCode> {
            let Some(item) = self.items_by_id.get(item_id).cloned() else {
                return Err(StatusCode::NOT_FOUND);
            };

            let descendants = self
                .items_by_id
                .values()
                .filter(|candidate| {
                    candidate.path == item.path
                        || candidate.path.starts_with(&format!("{}/", item.path))
                })
                .map(|candidate| candidate.id.clone())
                .collect::<Vec<_>>();

            for id in descendants {
                if let Some(removed) = self.items_by_id.remove(&id) {
                    self.path_to_id.remove(&removed.path);
                }
            }

            Ok(())
        }
    }

    impl MockServer {
        async fn start() -> Self {
            let state = MockGraphState {
                drive: Arc::new(Mutex::new(MockDrive::new())),
            };

            let app = Router::new()
                .route(
                    "/v1.0/{*path}",
                    get(mock_graph_handler)
                        .post(mock_graph_handler)
                        .put(mock_graph_handler)
                        .delete(mock_graph_handler),
                )
                .with_state(state);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("mock listener should bind");
            let addr = listener
                .local_addr()
                .expect("mock listener should have addr");
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("mock server should stay alive");
            });

            Self {
                base_url: format!("http://{addr}/v1.0"),
                _task: task,
            }
        }
    }

    async fn mock_graph_handler(
        State(state): State<MockGraphState>,
        method: Method,
        headers: HeaderMap,
        OriginalUri(uri): OriginalUri,
        body: Bytes,
    ) -> Response {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some("Bearer test-token")
        {
            return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
        }

        let Some(path) = uri.path().strip_prefix("/v1.0/me/drive") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let mut drive = state
            .drive
            .lock()
            .expect("mock drive state should not poison");

        match method {
            Method::GET if path == "/root" => {
                let root = drive
                    .items_by_id
                    .get("root")
                    .cloned()
                    .expect("mock drive root should exist");
                Json(drive.item_json(&root)).into_response()
            }
            Method::GET if path.starts_with("/root:/") && path.ends_with(":") => {
                let object_path = decode_graph_path(path, "/root:/", ":");
                match drive.item_by_path(&object_path) {
                    Some(item) => Json(drive.item_json(&item)).into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
            Method::PUT if path.starts_with("/root:/") && path.ends_with(":/content") => {
                let object_path = decode_graph_path(path, "/root:/", ":/content");
                let content_type = headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);

                match drive.put_file(&object_path, body.to_vec(), content_type) {
                    Ok(item) => (StatusCode::CREATED, Json(drive.item_json(&item))).into_response(),
                    Err(status) => status.into_response(),
                }
            }
            Method::GET if path.starts_with("/items/") && path.ends_with("/children") => {
                let item_id = decode_segment(
                    path.trim_start_matches("/items/")
                        .trim_end_matches("/children"),
                );
                if !drive.items_by_id.contains_key(&item_id) {
                    return StatusCode::NOT_FOUND.into_response();
                }

                let mut children = drive
                    .child_ids(&item_id)
                    .into_iter()
                    .filter_map(|id| drive.items_by_id.get(&id).cloned())
                    .map(|item| drive.item_json(&item))
                    .collect::<Vec<_>>();
                children.sort_by(|left, right| {
                    left["name"]
                        .as_str()
                        .unwrap_or_default()
                        .cmp(right["name"].as_str().unwrap_or_default())
                });

                Json(json!({ "value": children })).into_response()
            }
            Method::GET if path.starts_with("/items/") && path.ends_with("/content") => {
                let item_id = decode_segment(
                    path.trim_start_matches("/items/")
                        .trim_end_matches("/content"),
                );

                match drive.items_by_id.get(&item_id) {
                    Some(item) if item.is_folder => StatusCode::BAD_REQUEST.into_response(),
                    Some(item) => (
                        StatusCode::OK,
                        [(
                            "content-type",
                            item.content_type
                                .as_deref()
                                .unwrap_or("application/octet-stream"),
                        )],
                        item.body.clone(),
                    )
                        .into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
            }
            Method::POST if path.starts_with("/items/") && path.ends_with("/children") => {
                let parent_id = decode_segment(
                    path.trim_start_matches("/items/")
                        .trim_end_matches("/children"),
                );
                let payload = serde_json::from_slice::<Value>(&body)
                    .expect("folder creation payload should be JSON");
                let name = payload["name"]
                    .as_str()
                    .expect("folder creation payload should contain a name");

                match drive.create_folder(&parent_id, name) {
                    Ok(item) => (StatusCode::CREATED, Json(drive.item_json(&item))).into_response(),
                    Err(status) => status.into_response(),
                }
            }
            Method::DELETE if path.starts_with("/items/") => {
                let item_id = decode_segment(path.trim_start_matches("/items/"));
                match drive.delete_item(&item_id) {
                    Ok(()) => StatusCode::NO_CONTENT.into_response(),
                    Err(status) => status.into_response(),
                }
            }
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }

    fn decode_graph_path(path: &str, prefix: &str, suffix: &str) -> String {
        decode_segment(
            path.trim_start_matches(prefix)
                .trim_end_matches(suffix)
                .trim_matches('/'),
        )
    }

    fn decode_segment(value: &str) -> String {
        percent_decode_str(value).decode_utf8_lossy().to_string()
    }

    fn test_config(base_url: &str) -> OneDriveConfig {
        OneDriveConfig {
            enabled: true,
            tenant: "common".to_string(),
            client_id: Some("unit-test-client".to_string()),
            use_device_code: false,
            redirect_url: Some("http://127.0.0.1:61082/auth/onedrive/callback".to_string()),
            drive_id: None,
            graph_base_url: base_url.to_string(),
            token_source: TokenSource::Static {
                bearer: "test-token".to_string(),
            },
            root_prefix: Some("ccbg-backups".to_string()),
            user_agent: "carrier-cloud-blob-gateway-test".to_string(),
            request_timeout_secs: 5,
        }
    }

    #[tokio::test]
    async fn health_reports_degraded_until_root_prefix_exists() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        let health = adapter.health().await.expect("health should succeed");
        assert!(matches!(health.status, HealthStatus::Degraded));
        assert!(
            health
                .notes
                .iter()
                .any(|note| note.contains("root_prefix_missing=ccbg-backups"))
        );
    }

    #[tokio::test]
    async fn put_get_and_delete_object_roundtrip() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        let put = adapter
            .put_object(PutObjectRequest {
                container: "primary-bucket".to_string(),
                key: "nested/report.txt".to_string(),
                body: b"onedrive body".to_vec(),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("put should succeed");
        assert!(put.etag.is_some());

        let object = adapter
            .get_object("primary-bucket", "nested/report.txt")
            .await
            .expect("get should succeed");
        assert_eq!(object.body, b"onedrive body".to_vec());
        assert_eq!(object.info.content_type.as_deref(), Some("text/plain"));

        adapter
            .delete_object("primary-bucket", "nested/report.txt")
            .await
            .expect("delete should succeed");

        let error = adapter
            .get_object("primary-bucket", "nested/report.txt")
            .await
            .expect_err("deleted object should not exist");
        assert!(matches!(error, BlobError::NotFound(_)));
    }

    #[tokio::test]
    async fn list_containers_and_objects_follow_bucket_mapping() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "a.txt".to_string(),
                body: b"A".to_vec(),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("first put should succeed");
        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                body: b"B".to_vec(),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("second put should succeed");
        adapter
            .put_object(PutObjectRequest {
                container: "beta".to_string(),
                key: "c.txt".to_string(),
                body: b"C".to_vec(),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("third put should succeed");

        let containers = adapter
            .list_containers()
            .await
            .expect("containers should list");
        assert_eq!(
            containers
                .into_iter()
                .map(|container| container.name)
                .collect::<Vec<_>>(),
            vec!["alpha".to_string(), "beta".to_string()]
        );

        let objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some("alpha".to_string()),
                prefix: None,
                limit: None,
            })
            .await
            .expect("objects should list");
        assert_eq!(
            objects
                .into_iter()
                .map(|object| object.key)
                .collect::<Vec<_>>(),
            vec!["a.txt".to_string(), "nested/b.txt".to_string()]
        );
    }
}
