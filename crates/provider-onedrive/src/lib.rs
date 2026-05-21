use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, ContainerInfo, CopyObjectRequest, HealthStatus,
    ListObjectsRequest, MoveObjectRequest, ObjectInfo, ObjectPayload, PutObjectRequest,
    PutObjectResult, RenameObjectRequest, ServiceHealth, StorageScopeHealth, StorageScopeKind,
    TokenSource,
};
use bytes::Bytes;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::{
    Method, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, LOCATION, USER_AGENT},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use tokio::time::sleep;

pub const DEFAULT_ONEDRIVE_AUTH_BASE_URL: &str = "https://login.microsoftonline.com";
pub const DEFAULT_ONEDRIVE_SCOPES: &str = "offline_access Files.ReadWrite User.Read openid profile";
const SESSION_REFRESH_SKEW_SECS: u64 = 120;
const COPY_STATUS_POLL_INTERVAL_MS: u64 = 50;
const COPY_STATUS_POLL_MAX_ATTEMPTS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneDriveConfig {
    pub enabled: bool,
    pub tenant: String,
    pub client_id: Option<String>,
    pub use_device_code: bool,
    pub redirect_url: Option<String>,
    pub drive_id: Option<String>,
    pub graph_base_url: String,
    pub auth_base_url: String,
    pub scopes: String,
    pub session_file: Option<String>,
    pub token_source: TokenSource,
    pub root_prefix: Option<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
}

pub struct OneDriveBlobAdapter {
    config: OneDriveConfig,
    client: reqwest::Client,
    object_actions: Arc<dyn OneDriveObjectActionExecutor>,
}

#[derive(Debug, Clone)]
struct ResolvedOneDriveObject {
    container: String,
    key: String,
    path: String,
    item: DriveItemResponse,
}

#[derive(Debug, Clone)]
struct PreparedOneDriveDestination {
    container: String,
    key: String,
    parent_path: String,
    parent_id: String,
    name: String,
}

#[derive(Debug)]
struct TimedDriveItemResponse {
    item: DriveItemResponse,
    first_response_latency_ms: u64,
}

#[derive(Debug)]
struct TimedBytesPayload {
    body: Bytes,
    first_response_latency_ms: u64,
}

#[async_trait]
trait OneDriveObjectActionExecutor: Send + Sync {
    async fn rename_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError>;

    async fn copy_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError>;

    async fn move_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError>;
}

struct GraphOneDriveObjectActionExecutor;

#[derive(Debug, Deserialize)]
struct CopyOperationStatus {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    percentage_complete: Option<f64>,
    #[serde(default)]
    resource_id: Option<String>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct ParentReference {
    #[serde(rename = "driveId")]
    #[serde(default)]
    drive_id: Option<String>,
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
    #[serde(rename = "parentReference")]
    #[serde(default)]
    parent_reference: Option<ParentReference>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneDriveOAuthSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub scope: Option<String>,
    pub expires_at_unix: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    expires_in: Option<u64>,
}

#[async_trait]
impl OneDriveObjectActionExecutor for GraphOneDriveObjectActionExecutor {
    async fn rename_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError> {
        let same_parent = parent_path(&source.path).unwrap_or_default() == destination.parent_path;
        let action = format!(
            "rename OneDrive object {}/{} -> {}",
            source.container, source.key, destination.key
        );
        let mut payload = Map::new();
        payload.insert("name".to_string(), Value::String(destination.name.clone()));
        if !same_parent {
            let drive_id = adapter.current_drive_id().await?;
            payload.insert(
                "parentReference".to_string(),
                json!({
                    "driveId": drive_id,
                    "id": destination.parent_id,
                }),
            );
        }

        adapter
            .patch_item_json(&source.item.id, Value::Object(payload), &action)
            .await?;
        Ok(())
    }

    async fn copy_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError> {
        let action = format!(
            "copy OneDrive object {}/{} -> {}/{}",
            source.container, source.key, destination.container, destination.key
        );
        let drive_id = adapter.current_drive_id().await?;
        let monitor_url = adapter
            .post_copy_request(
                &source.item.id,
                json!({
                    "name": destination.name,
                    "parentReference": {
                        "driveId": drive_id,
                        "id": destination.parent_id,
                    }
                }),
                &action,
            )
            .await?;
        adapter.poll_copy_operation(&monitor_url, &action).await
    }

    async fn move_object(
        &self,
        adapter: &OneDriveBlobAdapter,
        source: &ResolvedOneDriveObject,
        destination: &PreparedOneDriveDestination,
    ) -> Result<(), BlobError> {
        let action = format!(
            "move OneDrive object {}/{} -> {}/{}",
            source.container, source.key, destination.container, destination.key
        );
        let drive_id = adapter.current_drive_id().await?;
        adapter
            .patch_item_json(
                &source.item.id,
                json!({
                    "name": destination.name,
                    "parentReference": {
                        "driveId": drive_id,
                        "id": destination.parent_id,
                    }
                }),
                &action,
            )
            .await?;
        Ok(())
    }
}

impl OneDriveBlobAdapter {
    pub fn new(config: OneDriveConfig) -> Self {
        Self::new_with_object_actions(config, Arc::new(GraphOneDriveObjectActionExecutor))
    }

    fn new_with_object_actions(
        config: OneDriveConfig,
        object_actions: Arc<dyn OneDriveObjectActionExecutor>,
    ) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            object_actions,
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

    fn auth_base_url(&self) -> &str {
        self.config.auth_base_url.trim_end_matches('/')
    }

    fn auth_tenant(&self) -> &str {
        let trimmed = self.config.tenant.trim();
        if trimmed.is_empty() {
            "common"
        } else {
            trimmed
        }
    }

    pub fn token_endpoint_url(&self) -> String {
        format!(
            "{}/{}/oauth2/v2.0/token",
            self.auth_base_url(),
            encode_segment(self.auth_tenant())
        )
    }

    pub fn authorization_endpoint_url(&self) -> String {
        format!(
            "{}/{}/oauth2/v2.0/authorize",
            self.auth_base_url(),
            encode_segment(self.auth_tenant())
        )
    }

    pub fn scope_string(&self) -> String {
        normalize_scope_string(&self.config.scopes)
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

    fn item_copy_url(&self, item_id: &str) -> String {
        format!("{}/copy", self.item_url(item_id))
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

    async fn request(
        &self,
        method: Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, BlobError> {
        let token = self.load_access_token().await?;

        Ok(self
            .client
            .request(method, url)
            .bearer_auth(token)
            .header(USER_AGENT, self.config.user_agent.as_str())
            .timeout(self.timeout()))
    }

    fn token_storage_candidates(&self) -> Vec<String> {
        let mut candidates = Vec::new();
        if let Some(path) = self
            .config
            .session_file
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            candidates.push(path.to_string());
        }

        if let TokenSource::File { path } = &self.config.token_source {
            if !candidates.iter().any(|candidate| candidate == path) {
                candidates.push(path.clone());
            }
        }

        candidates
    }

    async fn load_access_token(&self) -> Result<String, BlobError> {
        for path in self.token_storage_candidates() {
            match fs::read_to_string(&path) {
                Ok(raw) => {
                    let trimmed = raw.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if let Some(session) = decode_stored_oauth_session(trimmed) {
                        return self
                            .resolve_session_access_token(Some(path.as_str()), session)
                            .await;
                    }

                    return Ok(trimmed.to_string());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(BlobError::Configuration(format!(
                        "failed to read OneDrive token file {path}: {error}"
                    )));
                }
            }
        }

        self.config.token_source.load()
    }

    async fn resolve_session_access_token(
        &self,
        storage_path: Option<&str>,
        session: OneDriveOAuthSession,
    ) -> Result<String, BlobError> {
        if !session.needs_refresh(SESSION_REFRESH_SKEW_SECS) {
            return Ok(session.access_token);
        }

        let Some(storage_path) = storage_path else {
            return Err(BlobError::Configuration(
                "OneDrive OAuth session is expired and no writable session file is configured"
                    .to_string(),
            ));
        };

        let refreshed = self.refresh_session(&session).await?;
        persist_oauth_session(storage_path, &refreshed)?;
        Ok(refreshed.access_token.clone())
    }

    async fn refresh_session(
        &self,
        session: &OneDriveOAuthSession,
    ) -> Result<OneDriveOAuthSession, BlobError> {
        let refresh_token = session
            .refresh_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "OneDrive OAuth session is expired and has no refresh token".to_string(),
                )
            })?;
        let client_id = self
            .config
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "CCBG_ONEDRIVE_CLIENT_ID is required to refresh OneDrive OAuth sessions"
                        .to_string(),
                )
            })?;
        let scope = self.scope_string();

        let action = "refresh OneDrive OAuth session";
        let response = self
            .client
            .post(self.token_endpoint_url())
            .header(USER_AGENT, self.config.user_agent.as_str())
            .form(&[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("scope", scope.as_str()),
            ])
            .timeout(self.timeout())
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        let token_response = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(|error| {
                BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
            })?;

        token_response.into_session(session.refresh_token.as_deref())
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str, action: &str) -> Result<T, BlobError> {
        let response = self
            .request(Method::GET, url)
            .await?
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
            .request(Method::GET, url)
            .await?
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
        body: Bytes,
        content_type: Option<&str>,
        action: &str,
    ) -> Result<TimedDriveItemResponse, BlobError> {
        let request_started_at = Instant::now();
        let response = self
            .request(Method::PUT, url)
            .await?
            .header(
                CONTENT_TYPE,
                content_type.unwrap_or("application/octet-stream"),
            )
            .body(body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;
        let first_response_latency_ms = elapsed_millis(request_started_at);

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        let item = response.json::<DriveItemResponse>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })?;
        Ok(TimedDriveItemResponse {
            item,
            first_response_latency_ms,
        })
    }

    async fn get_bytes(&self, url: &str, action: &str) -> Result<TimedBytesPayload, BlobError> {
        let request_started_at = Instant::now();
        let response = self
            .request(Method::GET, url)
            .await?
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;
        let first_response_latency_ms = elapsed_millis(request_started_at);

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        let body = response.bytes().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid bytes: {error}"))
        })?;
        Ok(TimedBytesPayload {
            body,
            first_response_latency_ms,
        })
    }

    async fn delete_item_by_id(&self, item_id: &str, action: &str) -> Result<(), BlobError> {
        let response = self
            .request(Method::DELETE, &self.item_url(item_id))
            .await?
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        match response.status() {
            StatusCode::NO_CONTENT | StatusCode::OK => Ok(()),
            StatusCode::NOT_FOUND => Err(BlobError::NotFound(action.to_string())),
            _ => Err(response_to_error(response, action).await),
        }
    }

    async fn patch_item_json(
        &self,
        item_id: &str,
        body: Value,
        action: &str,
    ) -> Result<DriveItemResponse, BlobError> {
        let response = self
            .request(Method::PATCH, &self.item_url(item_id))
            .await?
            .header(ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response.json::<DriveItemResponse>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn post_copy_request(
        &self,
        item_id: &str,
        body: Value,
        action: &str,
    ) -> Result<String, BlobError> {
        let response = self
            .request(Method::POST, &self.item_copy_url(item_id))
            .await?
            .header(ACCEPT, "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if response.status() != StatusCode::ACCEPTED {
            return Err(response_to_error(response, action).await);
        }

        response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                BlobError::Upstream(format!(
                    "{action} did not return a usable Location header for copy monitoring"
                ))
            })
    }

    async fn poll_copy_operation(&self, monitor_url: &str, action: &str) -> Result<(), BlobError> {
        for attempt in 1..=COPY_STATUS_POLL_MAX_ATTEMPTS {
            let response = self
                .request(Method::GET, monitor_url)
                .await?
                .header(ACCEPT, "application/json")
                .send()
                .await
                .map_err(|error| {
                    BlobError::Upstream(format!(
                        "{action} monitor request failed on attempt {attempt}: {error}"
                    ))
                })?;

            if !response.status().is_success() {
                return Err(response_to_error(response, action).await);
            }

            let status = response
                .json::<CopyOperationStatus>()
                .await
                .map_err(|error| {
                    BlobError::Upstream(format!(
                        "{action} monitor returned invalid JSON on attempt {attempt}: {error}"
                    ))
                })?;

            match status
                .status
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("completed") => return Ok(()),
                Some("failed") => {
                    let detail = status
                        .error
                        .map(|value| trim_response_body(&value.to_string()))
                        .unwrap_or_else(|| "copy operation failed".to_string());
                    return Err(BlobError::Upstream(format!(
                        "{action} monitor failed: {detail}"
                    )));
                }
                Some("inprogress") | Some("in_progress") | Some("notstarted")
                | Some("not_started") | None => {
                    let _ = status.percentage_complete;
                    let _ = status.resource_id;
                    sleep(Duration::from_millis(COPY_STATUS_POLL_INTERVAL_MS)).await;
                }
                Some(other) => {
                    return Err(BlobError::Upstream(format!(
                        "{action} monitor returned unexpected status: {other}"
                    )));
                }
            }
        }

        Err(BlobError::Upstream(format!(
            "{action} monitor did not complete after {COPY_STATUS_POLL_MAX_ATTEMPTS} polling attempts"
        )))
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

    async fn resolve_object_with_path(
        &self,
        container: &str,
        key: &str,
    ) -> Result<ResolvedOneDriveObject, BlobError> {
        let path = self.object_path(container, key)?;
        let item = self
            .get_item_by_path(&path)
            .await?
            .ok_or_else(|| BlobError::NotFound(format!("object not found: {container}/{key}")))?;
        ensure_file(&item, &format!("object {container}/{key}"))?;
        Ok(ResolvedOneDriveObject {
            container: container.to_string(),
            key: normalize_path(key),
            path,
            item,
        })
    }

    async fn prepare_destination(
        &self,
        container: &str,
        key: &str,
    ) -> Result<PreparedOneDriveDestination, BlobError> {
        let path = self.object_path(container, key)?;
        let parent_path = parent_path(&path).ok_or_else(|| {
            BlobError::Configuration(format!(
                "destination path must include a container and object name: {container}/{key}"
            ))
        })?;
        self.ensure_folder_tree(&parent_path).await?;
        let parent = self.get_item_by_path(&parent_path).await?.ok_or_else(|| {
            BlobError::Upstream(format!("destination folder missing: {parent_path}"))
        })?;
        ensure_folder(&parent, &format!("destination folder {parent_path}"))?;
        let name = path
            .split('/')
            .next_back()
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(format!(
                    "destination key must include a file name: {container}/{key}"
                ))
            })?;

        Ok(PreparedOneDriveDestination {
            container: container.to_string(),
            key: normalize_path(key),
            parent_path,
            parent_id: parent.id,
            name,
        })
    }

    async fn current_drive_id(&self) -> Result<String, BlobError> {
        if let Some(drive_id) = self
            .config
            .drive_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(drive_id.to_string());
        }

        let root = self.get_root_item().await?;
        root.parent_reference
            .as_ref()
            .and_then(|value| value.drive_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "OneDrive root metadata did not include a usable driveId for copy requests"
                        .to_string(),
                )
            })
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
            .request(Method::POST, &url)
            .await?
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

        if let Err(error) = self.load_access_token().await {
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
            streaming_get: false,
            streaming_put: false,
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
            format!("auth_base_url={}", self.config.auth_base_url),
            format!("scopes={}", self.scope_string()),
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

        if let Some(session_file) = self
            .config
            .session_file
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            notes.push(format!("session_file={session_file}"));
        }

        let status = self.health_status(&mut notes).await;

        Ok(ServiceHealth {
            backend: self.name().to_string(),
            status,
            capabilities: self.capabilities(),
            scopes: vec![StorageScopeHealth {
                id: self
                    .config
                    .drive_id
                    .clone()
                    .unwrap_or_else(|| "default-drive".to_string()),
                label: "OneDrive".to_string(),
                kind: StorageScopeKind::Personal,
                writable: true,
                root: self.normalized_root_prefix(),
                container: None,
                object_count: None,
                capacity: None,
                notes: vec!["microsoft graph drive scope".to_string()],
            }],
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
        let downloaded = self
            .get_bytes(
                &self.item_content_url(&item.id),
                &format!("download object {container}/{key}"),
            )
            .await?;

        Ok(ObjectPayload {
            info: item.into_object_info(normalize_path(key)),
            body: downloaded.body.into(),
            first_response_latency_ms: Some(downloaded.first_response_latency_ms),
        })
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        self.ensure_enabled()?;

        let object_path = self.object_path(&request.container, &request.key)?;
        if let Some(parent_path) = parent_path(&object_path) {
            self.ensure_folder_tree(&parent_path).await?;
        }
        let body = request.body.collect().await?;

        let uploaded = self
            .put_bytes(
                &self.path_upload_url(&object_path),
                body,
                request.content_type.as_deref(),
                &format!("upload object {}/{}", request.container, request.key),
            )
            .await?;

        Ok(PutObjectResult {
            etag: uploaded.item.etag,
            first_response_latency_ms: Some(uploaded.first_response_latency_ms),
        })
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        self.ensure_enabled()?;

        let item = self.resolve_object(container, key).await?;
        self.delete_item_by_id(&item.id, &format!("delete object {container}/{key}"))
            .await
    }

    async fn rename_object(&self, request: RenameObjectRequest) -> Result<(), BlobError> {
        self.ensure_enabled()?;

        let source = self
            .resolve_object_with_path(&request.container, &request.key)
            .await?;
        let destination = self
            .prepare_destination(&request.container, &request.new_key)
            .await?;
        self.object_actions
            .rename_object(self, &source, &destination)
            .await
    }

    async fn copy_object(&self, request: CopyObjectRequest) -> Result<(), BlobError> {
        self.ensure_enabled()?;

        let source = self
            .resolve_object_with_path(&request.source_container, &request.source_key)
            .await?;
        let destination = self
            .prepare_destination(&request.destination_container, &request.destination_key)
            .await?;
        self.object_actions
            .copy_object(self, &source, &destination)
            .await
    }

    async fn move_object(&self, request: MoveObjectRequest) -> Result<(), BlobError> {
        self.ensure_enabled()?;

        let source = self
            .resolve_object_with_path(&request.source_container, &request.source_key)
            .await?;
        let destination = self
            .prepare_destination(&request.destination_container, &request.destination_key)
            .await?;
        self.object_actions
            .move_object(self, &source, &destination)
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

impl OneDriveOAuthSession {
    pub fn needs_refresh(&self, skew_secs: u64) -> bool {
        match self.expires_at_unix {
            Some(expires_at) => current_unix_time_secs().saturating_add(skew_secs) >= expires_at,
            None => false,
        }
    }
}

impl OAuthTokenResponse {
    fn into_session(
        self,
        previous_refresh_token: Option<&str>,
    ) -> Result<OneDriveOAuthSession, BlobError> {
        let access_token = self.access_token.trim();
        if access_token.is_empty() {
            return Err(BlobError::Upstream(
                "OneDrive OAuth token response did not include a usable access_token".to_string(),
            ));
        }

        Ok(OneDriveOAuthSession {
            access_token: access_token.to_string(),
            refresh_token: self
                .refresh_token
                .or_else(|| previous_refresh_token.map(ToString::to_string)),
            token_type: self
                .token_type
                .unwrap_or_else(|| "Bearer".to_string())
                .trim()
                .to_string(),
            scope: self.scope.map(|value| value.trim().to_string()),
            expires_at_unix: self
                .expires_in
                .map(|expires_in| current_unix_time_secs().saturating_add(expires_in)),
        })
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

fn elapsed_millis(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
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

fn normalize_scope_string(value: &str) -> String {
    let normalized = value
        .split(|char: char| char.is_ascii_whitespace() || char == ',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if normalized.is_empty() {
        DEFAULT_ONEDRIVE_SCOPES.to_string()
    } else {
        normalized
    }
}

pub fn decode_stored_oauth_session(raw: &str) -> Option<OneDriveOAuthSession> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') {
        return None;
    }

    serde_json::from_str::<OneDriveOAuthSession>(trimmed).ok()
}

pub fn persist_oauth_session(path: &str, session: &OneDriveOAuthSession) -> Result<(), BlobError> {
    let parent = Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            BlobError::Configuration(format!(
                "OneDrive session file path has no parent directory: {path}"
            ))
        })?;
    fs::create_dir_all(parent).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to create OneDrive session directory {}: {error}",
            parent.display()
        ))
    })?;

    let payload = serde_json::to_vec_pretty(session).map_err(|error| {
        BlobError::Upstream(format!("failed to encode OneDrive OAuth session: {error}"))
    })?;
    let temp_path = format!("{path}.tmp");
    fs::write(&temp_path, payload).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to write OneDrive session temp file {temp_path}: {error}"
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to replace OneDrive session file {path}: {error}"
        ))
    })?;
    Ok(())
}

fn current_unix_time_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
        routing::{get, post},
    };
    use percent_encoding::percent_decode_str;
    use serde_json::{Value, json};

    use super::*;

    #[derive(Clone)]
    struct MockGraphState {
        drive: Arc<Mutex<MockDrive>>,
        monitor_base_url: String,
    }

    struct MockDrive {
        items_by_id: BTreeMap<String, MockItem>,
        path_to_id: BTreeMap<String, String>,
        copy_operations: BTreeMap<String, CopyOperationRecord>,
        next_id: usize,
        next_copy_operation: usize,
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

    #[derive(Clone)]
    struct CopyOperationRecord {
        status: String,
        error: Option<Value>,
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
                copy_operations: BTreeMap::new(),
                next_id: 1,
                next_copy_operation: 1,
            }
        }

        fn item_json(&self, item: &MockItem) -> Value {
            let mut value = json!({
                "id": item.id,
                "name": item.name,
                "size": if item.is_folder { 0 } else { item.body.len() as u64 },
                "eTag": item.etag,
                "lastModifiedDateTime": item.last_modified,
                "parentReference": {
                    "driveId": "mock-drive",
                    "id": parent_path(&item.path)
                        .and_then(|path| self.path_to_id.get(&path).cloned())
                        .unwrap_or_else(|| "root".to_string()),
                }
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

        fn reindex_descendants(&mut self, item: &MockItem, previous_path: &str) {
            let descendants = self
                .items_by_id
                .values()
                .filter(|candidate| candidate.path.starts_with(&format!("{previous_path}/")))
                .map(|candidate| candidate.id.clone())
                .collect::<Vec<_>>();

            for descendant_id in descendants {
                let Some(descendant) = self.items_by_id.get_mut(&descendant_id) else {
                    continue;
                };
                self.path_to_id.remove(&descendant.path);
                let suffix = descendant
                    .path
                    .strip_prefix(&format!("{previous_path}/"))
                    .expect("descendant prefix should match");
                descendant.path = format!("{}/{}", item.path, suffix);
                descendant.name = descendant
                    .path
                    .split('/')
                    .next_back()
                    .unwrap_or_default()
                    .to_string();
                self.path_to_id
                    .insert(descendant.path.clone(), descendant.id.clone());
            }
        }

        fn patch_item(&mut self, item_id: &str, payload: &Value) -> Result<MockItem, StatusCode> {
            let Some(existing) = self.items_by_id.get(item_id).cloned() else {
                return Err(StatusCode::NOT_FOUND);
            };

            let new_name = payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(existing.name.as_str())
                .to_string();

            let parent_id = payload
                .get("parentReference")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    existing
                        .parent_reference_id(self)
                        .unwrap_or_else(|| "root".to_string())
                });

            let parent = self
                .items_by_id
                .get(&parent_id)
                .cloned()
                .ok_or(StatusCode::NOT_FOUND)?;
            if !parent.is_folder {
                return Err(StatusCode::BAD_REQUEST);
            }

            let new_path = join_relative_key(&parent.path, &new_name);
            if let Some(conflict_id) = self.path_to_id.get(&new_path) {
                if conflict_id != item_id {
                    return Err(StatusCode::CONFLICT);
                }
            }

            self.path_to_id.remove(&existing.path);
            let updated = self
                .items_by_id
                .get_mut(item_id)
                .expect("existing item should stay indexed");
            let previous_path = updated.path.clone();
            updated.name = new_name;
            updated.path = new_path.clone();
            updated.etag = format!("etag-file-{}", self.next_id);
            updated.last_modified = "2026-04-25T00:00:00Z".to_string();
            self.next_id += 1;
            let updated_snapshot = updated.clone();
            self.path_to_id.insert(new_path, item_id.to_string());
            if updated_snapshot.is_folder {
                self.reindex_descendants(&updated_snapshot, &previous_path);
            }
            Ok(updated_snapshot)
        }

        fn create_copy_operation(
            &mut self,
            item_id: &str,
            payload: &Value,
        ) -> Result<String, StatusCode> {
            let Some(source) = self.items_by_id.get(item_id).cloned() else {
                return Err(StatusCode::NOT_FOUND);
            };

            let target_name = payload
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(source.name.as_str())
                .to_string();
            let parent_id = payload
                .get("parentReference")
                .and_then(|value| value.get("id"))
                .and_then(Value::as_str)
                .ok_or(StatusCode::BAD_REQUEST)?
                .to_string();
            let parent = self
                .items_by_id
                .get(&parent_id)
                .cloned()
                .ok_or(StatusCode::NOT_FOUND)?;
            if !parent.is_folder {
                return Err(StatusCode::BAD_REQUEST);
            }

            let target_path = join_relative_key(&parent.path, &target_name);
            if self.path_to_id.contains_key(&target_path) {
                return Err(StatusCode::CONFLICT);
            }

            let copy = MockItem {
                id: format!("item-{}", self.next_id),
                path: target_path.clone(),
                name: target_name,
                is_folder: source.is_folder,
                body: source.body.clone(),
                content_type: source.content_type.clone(),
                etag: format!("etag-file-{}", self.next_id),
                last_modified: "2026-04-25T00:00:00Z".to_string(),
            };
            self.next_id += 1;
            self.path_to_id.insert(copy.path.clone(), copy.id.clone());
            self.items_by_id.insert(copy.id.clone(), copy);

            let operation_id = format!("copy-op-{}", self.next_copy_operation);
            self.next_copy_operation += 1;
            self.copy_operations.insert(
                operation_id.clone(),
                CopyOperationRecord {
                    status: "completed".to_string(),
                    error: None,
                },
            );
            Ok(operation_id)
        }

        fn copy_operation_status(&self, operation_id: &str) -> Option<CopyOperationRecord> {
            self.copy_operations.get(operation_id).cloned()
        }
    }

    impl MockItem {
        fn parent_reference_id(&self, drive: &MockDrive) -> Option<String> {
            parent_path(&self.path)
                .and_then(|path| drive.path_to_id.get(&path).cloned())
                .or_else(|| Some("root".to_string()))
        }
    }

    impl MockServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("mock listener should bind");
            let addr = listener
                .local_addr()
                .expect("mock listener should have addr");
            let base_url = format!("http://{addr}/v1.0");
            let state = MockGraphState {
                drive: Arc::new(Mutex::new(MockDrive::new())),
                monitor_base_url: format!("{base_url}/me/drive/monitor"),
            };

            let app = Router::new()
                .route("/common/oauth2/v2.0/token", post(mock_token_handler))
                .route(
                    "/v1.0/{*path}",
                    get(mock_graph_handler)
                        .post(mock_graph_handler)
                        .put(mock_graph_handler)
                        .patch(mock_graph_handler)
                        .delete(mock_graph_handler),
                )
                .with_state(state);
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("mock server should stay alive");
            });

            Self {
                base_url,
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
            .filter(|value| *value == "Bearer test-token" || *value == "Bearer refreshed-token")
            .is_none()
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
            Method::GET if path.starts_with("/monitor/") => {
                let operation_id = decode_segment(path.trim_start_matches("/monitor/"));
                match drive.copy_operation_status(&operation_id) {
                    Some(operation) => Json(json!({
                        "status": operation.status,
                        "error": operation.error,
                    }))
                    .into_response(),
                    None => StatusCode::NOT_FOUND.into_response(),
                }
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
            Method::PATCH if path.starts_with("/items/") => {
                let item_id = decode_segment(path.trim_start_matches("/items/"));
                let payload = serde_json::from_slice::<Value>(&body)
                    .expect("patch item payload should be JSON");
                match drive.patch_item(&item_id, &payload) {
                    Ok(item) => Json(drive.item_json(&item)).into_response(),
                    Err(status) => status.into_response(),
                }
            }
            Method::POST if path.starts_with("/items/") && path.ends_with("/copy") => {
                let item_id =
                    decode_segment(path.trim_start_matches("/items/").trim_end_matches("/copy"));
                let payload =
                    serde_json::from_slice::<Value>(&body).expect("copy payload should be JSON");
                match drive.create_copy_operation(&item_id, &payload) {
                    Ok(operation_id) => (
                        StatusCode::ACCEPTED,
                        [(
                            "location",
                            format!("{}/{}", state.monitor_base_url, operation_id),
                        )],
                    )
                        .into_response(),
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

    async fn mock_token_handler(body: Bytes) -> Response {
        let body = String::from_utf8(body.to_vec()).expect("token body should be utf-8");
        let params = body
            .split('&')
            .filter_map(|part| part.split_once('='))
            .map(|(key, value)| (decode_segment(key), decode_segment(value)))
            .collect::<BTreeMap<_, _>>();

        match (
            params.get("grant_type").map(String::as_str),
            params.get("refresh_token").map(String::as_str),
        ) {
            (Some("refresh_token"), Some("refresh-me")) => Json(json!({
                "access_token": "refreshed-token",
                "refresh_token": "refresh-me-2",
                "token_type": "Bearer",
                "scope": DEFAULT_ONEDRIVE_SCOPES,
                "expires_in": 3600
            }))
            .into_response(),
            _ => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_grant" })),
            )
                .into_response(),
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
            auth_base_url: base_url.trim_end_matches("/v1.0").to_string(),
            scopes: DEFAULT_ONEDRIVE_SCOPES.to_string(),
            session_file: None,
            token_source: TokenSource::Static {
                bearer: "test-token".to_string(),
            },
            root_prefix: Some("ccbg-backups".to_string()),
            user_agent: "carrier-cloud-blob-gateway-test".to_string(),
            request_timeout_secs: 5,
        }
    }

    fn temp_session_path() -> String {
        std::env::temp_dir()
            .join(format!(
                "ccbg-onedrive-session-{}-{}.json",
                std::process::id(),
                current_unix_time_secs()
            ))
            .display()
            .to_string()
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
                body: Bytes::from_static(b"onedrive body").into(),
                size: Some(13),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("put should succeed");
        assert!(put.etag.is_some());

        let object = adapter
            .get_object("primary-bucket", "nested/report.txt")
            .await
            .expect("get should succeed");
        assert_eq!(
            object
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"onedrive body"
        );
        assert_eq!(object.info.content_type.as_deref(), Some("text/plain"));

        adapter
            .delete_object("primary-bucket", "nested/report.txt")
            .await
            .expect("delete should succeed");

        let result = adapter
            .get_object("primary-bucket", "nested/report.txt")
            .await;
        assert!(matches!(result, Err(BlobError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_containers_and_objects_follow_bucket_mapping() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "a.txt".to_string(),
                body: Bytes::from_static(b"A").into(),
                size: Some(1),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("first put should succeed");
        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                body: Bytes::from_static(b"B").into(),
                size: Some(1),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("second put should succeed");
        adapter
            .put_object(PutObjectRequest {
                container: "beta".to_string(),
                key: "c.txt".to_string(),
                body: Bytes::from_static(b"C").into(),
                size: Some(1),
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

    #[tokio::test]
    async fn rename_object_supports_rename_and_cross_directory_move() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                body: Bytes::from_static(b"B").into(),
                size: Some(1),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed object should succeed");

        adapter
            .rename_object(RenameObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                new_key: "renamed/final.txt".to_string(),
            })
            .await
            .expect("rename should succeed");

        let result = adapter.get_object("alpha", "nested/b.txt").await;
        assert!(matches!(result, Err(BlobError::NotFound(_))));

        let renamed = adapter
            .get_object("alpha", "renamed/final.txt")
            .await
            .expect("renamed object should resolve");
        assert_eq!(
            renamed
                .body
                .collect()
                .await
                .expect("renamed body should collect")
                .as_ref(),
            b"B"
        );
    }

    #[tokio::test]
    async fn copy_object_supports_cross_container_destination() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                body: Bytes::from_static(b"B").into(),
                size: Some(1),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed object should succeed");

        adapter
            .copy_object(CopyObjectRequest {
                source_container: "alpha".to_string(),
                source_key: "nested/b.txt".to_string(),
                destination_container: "beta".to_string(),
                destination_key: "copied/b.txt".to_string(),
            })
            .await
            .expect("copy should succeed");

        let source = adapter
            .get_object("alpha", "nested/b.txt")
            .await
            .expect("source object should remain");
        assert_eq!(
            source
                .body
                .collect()
                .await
                .expect("source body should collect")
                .as_ref(),
            b"B"
        );

        let copied = adapter
            .get_object("beta", "copied/b.txt")
            .await
            .expect("copied object should resolve");
        assert_eq!(
            copied
                .body
                .collect()
                .await
                .expect("copied body should collect")
                .as_ref(),
            b"B"
        );
    }

    #[tokio::test]
    async fn move_object_supports_cross_container_destination() {
        let server = MockServer::start().await;
        let adapter = OneDriveBlobAdapter::new(test_config(&server.base_url));

        adapter
            .put_object(PutObjectRequest {
                container: "alpha".to_string(),
                key: "nested/b.txt".to_string(),
                body: Bytes::from_static(b"B").into(),
                size: Some(1),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed object should succeed");

        adapter
            .move_object(MoveObjectRequest {
                source_container: "alpha".to_string(),
                source_key: "nested/b.txt".to_string(),
                destination_container: "beta".to_string(),
                destination_key: "moved/b.txt".to_string(),
            })
            .await
            .expect("move should succeed");

        let result = adapter.get_object("alpha", "nested/b.txt").await;
        assert!(matches!(result, Err(BlobError::NotFound(_))));

        let moved = adapter
            .get_object("beta", "moved/b.txt")
            .await
            .expect("moved object should resolve");
        assert_eq!(
            moved
                .body
                .collect()
                .await
                .expect("moved body should collect")
                .as_ref(),
            b"B"
        );
    }

    #[tokio::test]
    async fn expired_session_file_refreshes_and_persists_new_tokens() {
        let server = MockServer::start().await;
        let session_file = temp_session_path();
        persist_oauth_session(
            &session_file,
            &OneDriveOAuthSession {
                access_token: "expired-token".to_string(),
                refresh_token: Some("refresh-me".to_string()),
                token_type: "Bearer".to_string(),
                scope: Some(DEFAULT_ONEDRIVE_SCOPES.to_string()),
                expires_at_unix: Some(current_unix_time_secs().saturating_sub(60)),
            },
        )
        .expect("session file should persist");

        let mut config = test_config(&server.base_url);
        config.session_file = Some(session_file.clone());
        config.token_source = TokenSource::EnvVar {
            key: "UNUSED_ONEDRIVE_TOKEN".to_string(),
        };
        let adapter = OneDriveBlobAdapter::new(config);

        let health = adapter.health().await.expect("health should succeed");
        assert!(
            !matches!(health.status, HealthStatus::Unavailable),
            "unexpected health notes: {:?}",
            health.notes
        );

        let stored = fs::read_to_string(&session_file).expect("session file should read");
        let stored =
            decode_stored_oauth_session(&stored).expect("session file should decode as JSON");
        assert_eq!(stored.access_token, "refreshed-token");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-me-2"));

        let _ = fs::remove_file(session_file);
    }
}
