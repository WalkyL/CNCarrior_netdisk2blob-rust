use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes::Aes128;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, ContainerInfo, HealthStatus, ListObjectsRequest,
    ObjectInfo, ObjectPayload, OutboundIpFamily, ServiceHealth, StorageCapacity,
    StorageScopeHealth, StorageScopeKind, TokenSource,
};
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use reqwest::{
    Method, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderName, HeaderValue, ORIGIN, REFERER, USER_AGENT},
};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value, json};

type Aes128CbcEncryptor = cbc::Encryptor<Aes128>;
type Aes128CbcDecryptor = cbc::Decryptor<Aes128>;

const UNICOM_AES_IV: &str = "wNSOYIB1k1DjY5lA";
const UNICOM_ROOT_CONTAINER: &str = "root";
const QUERY_ALL_FILES_OPERATION: &str = "QueryAllFiles";
const APP_QUERY_USER_OPERATION: &str = "AppQueryUser";
const QUERY_FAMILY_GROUPS_OPERATION: &str = "QueryFamilyGroups";
const GET_DOWNLOAD_URL_OPERATION: &str = "GetDownloadUrl";
const GET_DOWNLOAD_URL_V2_OPERATION: &str = "GetDownloadUrlV2";
const GET_DOWNLOAD_URL_V3_OPERATION: &str = "GetDownloadUrlV3";
const DOWNLOAD_URL_OPERATIONS: [&str; 3] = [
    GET_DOWNLOAD_URL_OPERATION,
    GET_DOWNLOAD_URL_V2_OPERATION,
    GET_DOWNLOAD_URL_V3_OPERATION,
];
const QUERY_ALL_FILES_ROOT_DIRECTORY_ID: &str = "0";
const DEFAULT_QUERY_ALL_FILES_PAGE_SIZE: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnicomConfig {
    pub base_url: String,
    pub token_source: TokenSource,
    pub outbound_ip_family: OutboundIpFamily,
    pub cookie_header: Option<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
    pub request_origin: Option<String>,
    pub request_referer: Option<String>,
    pub request_header_client_id: String,
    pub request_header_app_version: String,
    pub dispatcher_client_id: String,
    pub dispatcher_channel: String,
    pub dispatcher_secret: String,
    pub health_probe_operation: String,
    pub health_probe_style: String,
    pub health_probe_body_json: String,
    pub family_id: Option<String>,
    pub family_space_type: String,
    pub family_root_directory_id: String,
}

pub struct UnicomBlobAdapter {
    config: UnicomConfig,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthProbeStyle {
    ApiUserSecret,
    WohomeKey,
    WohomeSecret,
}

#[derive(Debug, Deserialize)]
struct DispatcherEnvelope {
    #[serde(rename = "RSP")]
    rsp: Option<DispatcherResponse>,
}

#[derive(Debug, Deserialize)]
struct DispatcherResponse {
    #[serde(rename = "RSP_CODE")]
    code: Option<String>,
    #[serde(rename = "RSP_DESC")]
    description: Option<String>,
    #[serde(rename = "DATA")]
    data: Option<Value>,
}

#[derive(Debug)]
struct AuthProbeResult {
    rsp_code: String,
    rsp_description: Option<String>,
    user_id: Option<String>,
    real_user_id: Option<String>,
    data_kind: Option<String>,
    data_keys: Vec<String>,
}

#[derive(Debug)]
struct DispatcherCallResult {
    code: String,
    description: Option<String>,
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct QueryAllFilesData {
    #[serde(default)]
    files: Vec<QueryAllFilesEntry>,
    #[serde(rename = "systemDirs", default)]
    _system_dirs: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct QueryAllFilesEntry {
    #[serde(deserialize_with = "deserialize_string_like")]
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "fileName", default)]
    file_name: Option<String>,
    #[serde(rename = "directoryName", default)]
    directory_name: Option<String>,
    #[serde(rename = "type", default)]
    entry_type: Option<u8>,
    #[serde(
        rename = "parentDirectoryId",
        default,
        deserialize_with = "deserialize_optional_string_like"
    )]
    _parent_directory_id: Option<String>,
    #[serde(
        rename = "directoryId",
        default,
        deserialize_with = "deserialize_optional_string_like"
    )]
    _directory_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    size: Option<u64>,
    #[serde(
        rename = "fileSize",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    file_size: Option<u64>,
    #[serde(rename = "createTime", default)]
    create_time: Option<String>,
    #[serde(rename = "updateTime", default)]
    update_time: Option<String>,
    #[serde(rename = "lastUpdateTime", default)]
    last_update_time: Option<String>,
    #[serde(rename = "contentType", default)]
    content_type: Option<String>,
    #[serde(rename = "suffix", default)]
    suffix: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct DownloadUrlEntry {
    #[serde(rename = "downloadUrl", default)]
    download_url: Option<String>,
    #[serde(rename = "url", default)]
    url: Option<String>,
}

#[derive(Debug, Clone)]
struct DownloadIdentifierCandidate {
    label: &'static str,
    value: String,
}

impl AuthProbeStyle {
    fn parse(raw: &str) -> Result<Self, BlobError> {
        match raw.trim() {
            "" | "wohome-secret" => Ok(Self::WohomeSecret),
            "wohome-key" => Ok(Self::WohomeKey),
            "api-user-secret" => Ok(Self::ApiUserSecret),
            other => Err(BlobError::Configuration(format!(
                "unsupported CCBG_UNICOM_AUTH_PROBE_STYLE: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ApiUserSecret => "api-user-secret",
            Self::WohomeKey => "wohome-key",
            Self::WohomeSecret => "wohome-secret",
        }
    }

    fn default_channel(self) -> &'static str {
        match self {
            Self::ApiUserSecret => "api-user",
            Self::WohomeKey | Self::WohomeSecret => "wohome",
        }
    }

    fn dispatcher_path(self) -> &'static str {
        match self {
            Self::ApiUserSecret => "/api-user/dispatcher",
            Self::WohomeKey | Self::WohomeSecret => "/wohome/dispatcher",
        }
    }
}

impl UnicomBlobAdapter {
    pub fn new(config: UnicomConfig) -> Result<Self, BlobError> {
        Ok(Self {
            client: build_http_client(&config)?,
            config,
        })
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.request_timeout_secs.max(1))
    }

    fn trimmed_base_url(&self) -> &str {
        self.config.base_url.trim_end_matches('/')
    }

    fn dispatcher_url(&self, style: AuthProbeStyle) -> String {
        format!("{}{}", self.trimmed_base_url(), style.dispatcher_path())
    }

    fn request(&self, method: Method, url: &str) -> Result<reqwest::RequestBuilder, BlobError> {
        let mut request = self
            .client
            .request(method, url)
            .header(USER_AGENT, self.config.user_agent.as_str())
            .timeout(self.timeout());

        if let Some(origin) = self
            .config
            .request_origin
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header(
                ORIGIN,
                HeaderValue::from_str(origin).map_err(|error| {
                    BlobError::Configuration(format!("invalid CCBG_UNICOM_REQUEST_ORIGIN: {error}"))
                })?,
            );
        }

        if let Some(referer) = self
            .config
            .request_referer
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header(
                REFERER,
                HeaderValue::from_str(referer).map_err(|error| {
                    BlobError::Configuration(format!(
                        "invalid CCBG_UNICOM_REQUEST_REFERER: {error}"
                    ))
                })?,
            );
        }

        if let Some(cookie_header) = self
            .config
            .cookie_header
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            request = request.header(
                COOKIE,
                HeaderValue::from_str(cookie_header).map_err(|error| {
                    BlobError::Configuration(format!("invalid CCBG_UNICOM_COOKIE_HEADER: {error}"))
                })?,
            );
        }

        Ok(request)
    }

    fn request_with_wohome_headers(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> reqwest::RequestBuilder {
        request
            .header("accesstoken", token)
            .header(ACCEPT, "application/json, text/plain, */*")
    }

    fn request_with_api_user_headers(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> reqwest::RequestBuilder {
        let mut request = request
            .header("Access-Token", token)
            .header("X-YP-Access-Token", token);

        if !self.config.request_header_client_id.trim().is_empty() {
            request = request.header(
                HeaderName::from_static("client-id"),
                self.config.request_header_client_id.as_str(),
            );
            request = request.header("Client-Id", self.config.request_header_client_id.as_str());
        }

        if !self.config.request_header_app_version.trim().is_empty() {
            request = request.header(
                "App-Version",
                self.config.request_header_app_version.as_str(),
            );
        }

        request
    }

    fn dispatcher_channel(&self, style: AuthProbeStyle) -> String {
        let configured = self.config.dispatcher_channel.trim();
        if configured.is_empty() {
            style.default_channel().to_string()
        } else {
            configured.to_string()
        }
    }

    fn probe_body_value(&self) -> Result<Value, BlobError> {
        let raw = self.config.health_probe_body_json.trim();
        if raw.is_empty() {
            return Ok(json!({}));
        }

        serde_json::from_str(raw).map_err(|error| {
            BlobError::Configuration(format!("invalid CCBG_UNICOM_AUTH_PROBE_BODY_JSON: {error}"))
        })
    }

    fn probe_body_object(&self) -> Result<Map<String, Value>, BlobError> {
        match self.probe_body_value()? {
            Value::Null => Ok(Map::new()),
            Value::Object(map) => Ok(map),
            _ => Err(BlobError::Configuration(
                "CCBG_UNICOM_AUTH_PROBE_BODY_JSON must be a JSON object".to_string(),
            )),
        }
    }

    fn dispatcher_style(&self) -> Result<AuthProbeStyle, BlobError> {
        AuthProbeStyle::parse(self.config.health_probe_style.as_str())
    }

    fn configured_space_type(&self) -> Result<String, BlobError> {
        Ok(self
            .probe_body_object()?
            .get("spaceType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("0")
            .to_string())
    }

    fn configured_family_id(&self) -> Option<String> {
        self.config
            .family_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    fn configured_family_space_type(&self) -> &str {
        let value = self.config.family_space_type.trim();
        if value.is_empty() { "1" } else { value }
    }

    fn configured_family_root_directory_id(&self) -> &str {
        let value = self.config.family_root_directory_id.trim();
        if value.is_empty() {
            QUERY_ALL_FILES_ROOT_DIRECTORY_ID
        } else {
            value
        }
    }

    async fn dispatch_json(
        &self,
        operation: &str,
        dispatcher_body: Value,
    ) -> Result<DispatcherCallResult, BlobError> {
        let token = self.config.token_source.load()?;
        if operation.is_empty() {
            return Err(BlobError::Configuration(
                "dispatcher operation is empty".to_string(),
            ));
        }

        let style = self.dispatcher_style()?;
        let dispatcher_channel = self.dispatcher_channel(style);
        let dispatcher_url = self.dispatcher_url(style);

        let payload = match style {
            AuthProbeStyle::ApiUserSecret => build_api_user_dispatcher_payload(
                operation,
                dispatcher_channel.as_str(),
                self.config.dispatcher_client_id.trim(),
                self.config.dispatcher_secret.as_str(),
                dispatcher_body,
            )?,
            AuthProbeStyle::WohomeKey => build_wohome_dispatcher_payload(
                operation,
                dispatcher_channel.as_str(),
                self.config.dispatcher_client_id.trim(),
                token.as_str(),
                dispatcher_body,
                "key",
            )?,
            AuthProbeStyle::WohomeSecret => build_wohome_dispatcher_payload(
                operation,
                dispatcher_channel.as_str(),
                self.config.dispatcher_client_id.trim(),
                token.as_str(),
                dispatcher_body,
                "secret",
            )?,
        };

        let request = self
            .request(Method::POST, dispatcher_url.as_str())?
            .header(CONTENT_TYPE, "application/json");
        let request = match style {
            AuthProbeStyle::ApiUserSecret => self.request_with_api_user_headers(request, &token),
            AuthProbeStyle::WohomeKey | AuthProbeStyle::WohomeSecret => {
                self.request_with_wohome_headers(request, &token)
            }
        };

        let response =
            request.json(&payload).send().await.map_err(|error| {
                BlobError::Upstream(format!("{operation} request failed: {error}"))
            })?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(BlobError::Upstream(format!(
                "{operation} rejected the token with HTTP 401"
            )));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, operation).await);
        }

        let envelope = response
            .json::<DispatcherEnvelope>()
            .await
            .map_err(|error| {
                BlobError::Upstream(format!("{operation} returned invalid JSON: {error}"))
            })?;

        let rsp = envelope
            .rsp
            .ok_or_else(|| BlobError::Upstream(format!("{operation} returned no RSP payload")))?;

        let code = rsp.code.unwrap_or_else(|| "unknown".to_string());
        if code != "0000" {
            let hint = if code == "9999" {
                "; desktop flow commonly also requires matching clientId, Origin/Referer, and exact AES-IV handling"
            } else {
                ""
            };
            return Err(BlobError::Upstream(format!(
                "{operation} returned RSP_CODE={}{}{}",
                code,
                rsp.description
                    .as_deref()
                    .map(|description| format!(" ({description})"))
                    .unwrap_or_default(),
                hint,
            )));
        }

        let data = decode_dispatcher_data(
            style,
            rsp.data,
            token.as_str(),
            self.config.dispatcher_secret.as_str(),
        )?;

        Ok(DispatcherCallResult {
            code,
            description: rsp.description,
            data,
        })
    }

    async fn probe_auth(&self) -> Result<AuthProbeResult, BlobError> {
        let operation = self.config.health_probe_operation.trim();
        if operation.is_empty() {
            return Err(BlobError::Configuration(
                "CCBG_UNICOM_AUTH_PROBE_OPERATION is empty".to_string(),
            ));
        }

        let result = self
            .dispatch_json(operation, self.probe_body_value()?)
            .await?;
        let (user_id, real_user_id) = result
            .data
            .as_ref()
            .map(extract_user_identifiers)
            .unwrap_or_default();

        Ok(AuthProbeResult {
            rsp_code: result.code,
            rsp_description: result.description,
            user_id,
            real_user_id,
            data_kind: result.data.as_ref().map(value_kind),
            data_keys: top_level_keys(result.data.as_ref()),
        })
    }

    async fn query_all_files_page(
        &self,
        parent_directory_id: &str,
        page_num: usize,
        page_size: usize,
    ) -> Result<QueryAllFilesData, BlobError> {
        let space_type = self.configured_space_type()?;
        self.query_all_files_page_for_scope(
            parent_directory_id,
            page_num,
            page_size,
            space_type.as_str(),
            None,
        )
        .await
    }

    async fn query_all_files_page_for_scope(
        &self,
        parent_directory_id: &str,
        page_num: usize,
        page_size: usize,
        space_type: &str,
        family_id: Option<&str>,
    ) -> Result<QueryAllFilesData, BlobError> {
        let mut body = self.probe_body_object()?;
        body.insert(
            "parentDirectoryId".to_string(),
            Value::String(parent_directory_id.to_string()),
        );
        body.insert("pageNum".to_string(), json!(page_num));
        body.insert("pageSize".to_string(), json!(page_size));
        body.insert(
            "spaceType".to_string(),
            Value::String(space_type.trim().to_string()),
        );
        body.entry("sortRule".to_string())
            .or_insert_with(|| Value::Number(0.into()));
        if let Some(family_id) = family_id.map(str::trim).filter(|value| !value.is_empty()) {
            body.insert("familyId".to_string(), Value::String(family_id.to_string()));
        }

        let result = self
            .dispatch_json(QUERY_ALL_FILES_OPERATION, Value::Object(body))
            .await?;
        let payload = result
            .data
            .unwrap_or_else(|| json!({ "files": [], "systemDirs": [] }));

        serde_json::from_value(payload).map_err(|error| {
            BlobError::Upstream(format!(
                "{QUERY_ALL_FILES_OPERATION} returned unexpected DATA shape: {error}"
            ))
        })
    }

    async fn app_query_user(&self) -> Result<Option<Value>, BlobError> {
        self.dispatch_json(APP_QUERY_USER_OPERATION, json!({}))
            .await
            .map(|result| result.data)
    }

    async fn query_family_groups(&self) -> Result<Option<Value>, BlobError> {
        self.dispatch_json(QUERY_FAMILY_GROUPS_OPERATION, json!({}))
            .await
            .map(|result| result.data)
    }

    async fn resolved_family_id(&self) -> Result<Option<String>, BlobError> {
        if let Some(family_id) = self.configured_family_id() {
            return Ok(Some(family_id));
        }

        let payload = match self.query_family_groups().await {
            Ok(payload) => payload,
            Err(error) => {
                return Err(BlobError::Upstream(format!(
                    "{QUERY_FAMILY_GROUPS_OPERATION} failed: {error}"
                )));
            }
        };

        Ok(find_first_string_for_keys(
            payload.as_ref(),
            &["familyId", "id", "groupId"],
        ))
    }

    fn personal_scope_health(
        &self,
        root_page: &QueryAllFilesData,
        capacity: Option<StorageCapacity>,
    ) -> StorageScopeHealth {
        StorageScopeHealth {
            id: "personal".to_string(),
            label: "Personal Cloud".to_string(),
            kind: StorageScopeKind::Personal,
            writable: false,
            root: Some(QUERY_ALL_FILES_ROOT_DIRECTORY_ID.to_string()),
            container: Some(UNICOM_ROOT_CONTAINER.to_string()),
            object_count: Some(root_page.files.len() as u64),
            capacity: capacity.filter(|value| {
                value.total_bytes.is_some()
                    || value.used_bytes.is_some()
                    || value.free_bytes.is_some()
            }),
            notes: vec![format!(
                "space_type={}",
                self.configured_space_type()
                    .unwrap_or_else(|_| "0".to_string())
            )],
        }
    }

    fn family_scope_health(
        &self,
        family_id: &str,
        root_page: &QueryAllFilesData,
    ) -> StorageScopeHealth {
        StorageScopeHealth {
            id: family_id.to_string(),
            label: "Family Cloud".to_string(),
            kind: StorageScopeKind::Family,
            writable: false,
            root: Some(self.configured_family_root_directory_id().to_string()),
            container: None,
            object_count: Some(root_page.files.len() as u64),
            capacity: None,
            notes: vec![
                format!("family_id={family_id}"),
                format!("space_type={}", self.configured_family_space_type()),
                "scope discovered from web session; S3 object routing is still bound to the personal root in v1".to_string(),
            ],
        }
    }

    fn push_remediation_notes(&self, notes: &mut Vec<String>, error: &BlobError) {
        fn push_once(notes: &mut Vec<String>, note: &str) {
            if !notes.iter().any(|existing| existing == note) {
                notes.push(note.to_string());
            }
        }

        let message = error.to_string();
        if message.contains("missing environment variable: CCBG_UNICOM_TOKEN")
            || message.contains("inline token is empty")
        {
            push_once(
                notes,
                "remediation=fill China Unicom Access Token in Admin Web -> Provider Credentials -> China Unicom",
            );
        } else if message.contains("HTTP 401") || message.contains("RSP_CODE=9999") {
            push_once(
                notes,
                "remediation=China Unicom token is likely stale or the browser headers no longer match; refresh pan.wo.cn, capture a fresh accesstoken, then save it again",
            );
            push_once(
                notes,
                "remediation_hint=keep Origin=https://pan.wo.cn and Referer=https://pan.wo.cn/ aligned with the browser session",
            );
        }
    }

    async fn find_child_entry(
        &self,
        parent_directory_id: &str,
        child_name: &str,
        entry_kind: QueryAllFilesEntryKind,
    ) -> Result<QueryAllFilesEntry, BlobError> {
        let mut page_num = 0;

        loop {
            let page = self
                .query_all_files_page(
                    parent_directory_id,
                    page_num,
                    DEFAULT_QUERY_ALL_FILES_PAGE_SIZE,
                )
                .await?;
            let fetched_count = page.files.len();

            if let Some(entry) = page.files.into_iter().find(|entry| {
                entry.display_name() == Some(child_name) && entry.kind() == entry_kind
            }) {
                return Ok(entry);
            }

            if fetched_count < DEFAULT_QUERY_ALL_FILES_PAGE_SIZE {
                break;
            }
            page_num += 1;
        }

        Err(BlobError::NotFound(format!(
            "entry not found under {parent_directory_id}: {child_name}"
        )))
    }

    async fn resolve_object_entry(
        &self,
        key: &str,
    ) -> Result<(QueryAllFilesEntry, String), BlobError> {
        let normalized_key = normalize_object_key(key);
        if normalized_key.is_empty() {
            return Err(BlobError::NotFound("object key is empty".to_string()));
        }

        let segments = normalized_key.split('/').collect::<Vec<_>>();
        let mut parent_directory_id = QUERY_ALL_FILES_ROOT_DIRECTORY_ID.to_string();

        for segment in &segments[..segments.len().saturating_sub(1)] {
            let directory = self
                .find_child_entry(
                    &parent_directory_id,
                    segment,
                    QueryAllFilesEntryKind::Directory,
                )
                .await?;
            parent_directory_id = directory.id.clone();
        }

        let file_name = segments
            .last()
            .expect("normalized object key should contain at least one segment");
        let file = self
            .find_child_entry(
                &parent_directory_id,
                file_name,
                QueryAllFilesEntryKind::File,
            )
            .await?;

        Ok((file, normalized_key))
    }

    async fn download_url_for_entry(
        &self,
        entry: &QueryAllFilesEntry,
    ) -> Result<String, BlobError> {
        let client_id = self.config.dispatcher_client_id.trim();
        let space_type = self.configured_space_type()?;
        let identifiers = entry.download_identifier_candidates();
        let mut errors = Vec::new();

        for operation in DOWNLOAD_URL_OPERATIONS {
            for identifier in &identifiers {
                let body = json!({
                    "fidList": [identifier.value.as_str()],
                    "clientId": client_id,
                    "spaceType": space_type.as_str(),
                });

                match self.dispatch_json(operation, body).await {
                    Ok(result) => match extract_download_url(operation, result.data) {
                        Ok(url) => return Ok(url),
                        Err(error) => errors.push(format!(
                            "{operation} via {} failed: {error}",
                            identifier.label
                        )),
                    },
                    Err(error) => errors.push(format!(
                        "{operation} via {} failed: {error}",
                        identifier.label
                    )),
                }
            }
        }

        Err(BlobError::Upstream(format!(
            "failed to resolve a download URL for {}: {}",
            entry.display_name().unwrap_or(entry.id.as_str()),
            errors.join("; ")
        )))
    }

    fn download_request(&self, method: Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .header(USER_AGENT, self.config.user_agent.as_str())
            .timeout(self.timeout())
    }

    async fn get_bytes(&self, url: &str, action: &str) -> Result<Vec<u8>, BlobError> {
        let response = self
            .download_request(Method::GET, url)
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

    fn validate_container(&self, container: &str) -> Result<(), BlobError> {
        if normalize_object_key(container) == UNICOM_ROOT_CONTAINER {
            Ok(())
        } else {
            Err(BlobError::NotFound(format!(
                "container not found: {container}"
            )))
        }
    }

    async fn list_objects_in_root(
        &self,
        request: &ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        if matches!(request.limit, Some(0)) {
            return Ok(Vec::new());
        }

        let normalized_prefix = request.prefix.as_deref().map(normalize_object_key);
        let mut objects = BTreeMap::new();
        let mut stack = vec![(QUERY_ALL_FILES_ROOT_DIRECTORY_ID.to_string(), String::new())];

        while let Some((folder_id, folder_prefix)) = stack.pop() {
            let mut page_num = 0;

            loop {
                let page = self
                    .query_all_files_page(&folder_id, page_num, DEFAULT_QUERY_ALL_FILES_PAGE_SIZE)
                    .await?;
                let fetched_count = page.files.len();

                for entry in page.files {
                    if entry.is_directory() {
                        let child_name = entry.display_name().ok_or_else(|| {
                            BlobError::Upstream(format!(
                                "{QUERY_ALL_FILES_OPERATION} returned a directory without a name"
                            ))
                        })?;
                        let child_prefix = join_relative_key(&folder_prefix, child_name);

                        if normalized_prefix.as_deref().is_none_or(|prefix| {
                            directory_may_contain_prefix(&child_prefix, prefix)
                        }) {
                            stack.push((entry.id.clone(), child_prefix));
                        }
                        continue;
                    }

                    if !entry.is_file() {
                        continue;
                    }

                    let object_key = entry.object_key(&folder_prefix)?;
                    if normalized_prefix
                        .as_deref()
                        .is_none_or(|prefix| object_key.starts_with(prefix))
                    {
                        objects.insert(object_key.clone(), entry.into_object_info(object_key));
                        if let Some(limit) = request.limit {
                            trim_objects_to_limit(&mut objects, limit);
                        }
                    }
                }

                if fetched_count < DEFAULT_QUERY_ALL_FILES_PAGE_SIZE {
                    break;
                }
                page_num += 1;
            }
        }

        Ok(objects.into_values().collect())
    }
}

#[async_trait]
impl BlobBackend for UnicomBlobAdapter {
    fn name(&self) -> &'static str {
        "unicom-cloud-drive"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            read: true,
            write: false,
            delete: false,
            multipart_upload: false,
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
        let mut notes = vec![
            format!("base_url={}", self.config.base_url),
            format!("auth_source={}", self.config.token_source.describe()),
            format!(
                "outbound_ip_family={}",
                self.config.outbound_ip_family.as_str()
            ),
            format!("auth_probe={}", self.config.health_probe_operation),
            format!(
                "auth_probe_style={}",
                AuthProbeStyle::parse(self.config.health_probe_style.as_str())?.as_str()
            ),
            format!(
                "dispatcher_channel={}",
                self.dispatcher_channel(AuthProbeStyle::parse(
                    self.config.health_probe_style.as_str()
                )?)
            ),
            format!(
                "dispatcher_client_id={}",
                self.config.dispatcher_client_id.trim()
            ),
            format!("root_container={UNICOM_ROOT_CONTAINER}"),
            format!("list_operation={QUERY_ALL_FILES_OPERATION}"),
            format!("download_operations={}", DOWNLOAD_URL_OPERATIONS.join(",")),
            "browser-session interception is intentionally out of scope".to_string(),
        ];
        let mut scopes = Vec::new();

        if self
            .config
            .cookie_header
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            notes.push("cookie_header=present".to_string());
        }

        let mut status = HealthStatus::Unavailable;
        match self.probe_auth().await {
            Ok(result) => {
                notes.push(format!("auth_probe_rsp_code={}", result.rsp_code));
                if let Some(description) = result.rsp_description {
                    notes.push(format!("auth_probe_rsp_desc={description}"));
                }
                if let Some(kind) = result.data_kind {
                    notes.push(format!("auth_probe_data_kind={kind}"));
                }
                if !result.data_keys.is_empty() {
                    notes.push(format!(
                        "auth_probe_data_keys={}",
                        result.data_keys.join(",")
                    ));
                }
                if let Some(user_id) = result.user_id {
                    notes.push(format!("user_id={user_id}"));
                }
                if let Some(real_user_id) = result.real_user_id {
                    notes.push(format!("real_user_id={real_user_id}"));
                }
                notes.push("auth_probe_status=accepted".to_string());
                status = HealthStatus::Degraded;
            }
            Err(error) => {
                notes.push(error.to_string());
                self.push_remediation_notes(&mut notes, &error);
            }
        }

        match self
            .query_all_files_page(
                QUERY_ALL_FILES_ROOT_DIRECTORY_ID,
                0,
                DEFAULT_QUERY_ALL_FILES_PAGE_SIZE,
            )
            .await
        {
            Ok(personal_root) => {
                status = HealthStatus::Healthy;
                notes.push(format!(
                    "personal_root_entry_count={}",
                    personal_root.files.len()
                ));
                let capacity = match self.app_query_user().await {
                    Ok(payload) => {
                        let capacity = parse_capacity_from_value(payload.as_ref());
                        if capacity.is_none() {
                            notes.push(
                                "personal_capacity_probe=accepted_but_shape_unknown".to_string(),
                            );
                        }
                        capacity
                    }
                    Err(error) => {
                        notes.push(format!("personal_capacity_probe_failed={error}"));
                        self.push_remediation_notes(&mut notes, &error);
                        None
                    }
                };
                scopes.push(self.personal_scope_health(&personal_root, capacity));
            }
            Err(error) => {
                notes.push(format!("personal_root_probe_failed={error}"));
                self.push_remediation_notes(&mut notes, &error);
            }
        }

        match self.resolved_family_id().await {
            Ok(Some(family_id)) => match self
                .query_all_files_page_for_scope(
                    self.configured_family_root_directory_id(),
                    0,
                    DEFAULT_QUERY_ALL_FILES_PAGE_SIZE,
                    self.configured_family_space_type(),
                    Some(family_id.as_str()),
                )
                .await
            {
                Ok(family_root) => {
                    notes.push(format!(
                        "family_root_entry_count={}",
                        family_root.files.len()
                    ));
                    scopes.push(self.family_scope_health(&family_id, &family_root));
                }
                Err(error) => {
                    notes.push(format!("family_root_probe_failed={error}"));
                    self.push_remediation_notes(&mut notes, &error);
                }
            },
            Ok(None) => {
                notes.push("family_scope=not_discovered".to_string());
            }
            Err(error) => {
                notes.push(format!("family_scope_probe_failed={error}"));
                self.push_remediation_notes(&mut notes, &error);
            }
        };

        Ok(ServiceHealth {
            backend: self.name().to_string(),
            status,
            capabilities: self.capabilities(),
            scopes,
            notes,
        })
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
        self.query_all_files_page(
            QUERY_ALL_FILES_ROOT_DIRECTORY_ID,
            0,
            DEFAULT_QUERY_ALL_FILES_PAGE_SIZE,
        )
        .await?;

        Ok(vec![ContainerInfo {
            name: UNICOM_ROOT_CONTAINER.to_string(),
            object_count: None,
        }])
    }

    async fn list_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        let Some(container) = request.container.as_deref() else {
            return Ok(Vec::new());
        };

        self.validate_container(container)?;
        self.list_objects_in_root(&request).await
    }

    async fn head_container(&self, name: &str) -> Result<ContainerInfo, BlobError> {
        self.validate_container(name)?;
        self.query_all_files_page(QUERY_ALL_FILES_ROOT_DIRECTORY_ID, 0, 1)
            .await?;

        Ok(ContainerInfo {
            name: UNICOM_ROOT_CONTAINER.to_string(),
            object_count: None,
        })
    }

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        self.validate_container(container)?;
        let (entry, normalized_key) = self.resolve_object_entry(key).await?;
        Ok(entry.into_object_info(normalized_key))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        self.validate_container(container)?;
        let (entry, normalized_key) = self.resolve_object_entry(key).await?;
        let download_url = self.download_url_for_entry(&entry).await?;
        let info = entry.into_object_info(normalized_key);
        let body = self
            .get_bytes(&download_url, &format!("download object {container}/{key}"))
            .await?;

        Ok(ObjectPayload { info, body })
    }
}

impl QueryAllFilesEntry {
    fn kind(&self) -> QueryAllFilesEntryKind {
        if self.is_directory() {
            QueryAllFilesEntryKind::Directory
        } else {
            QueryAllFilesEntryKind::File
        }
    }

    fn is_directory(&self) -> bool {
        self.entry_type == Some(0)
    }

    fn is_file(&self) -> bool {
        self.entry_type == Some(1)
    }

    fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.file_name.as_deref())
            .or(self.directory_name.as_deref())
            .or_else(|| value_as_str(self.extra.get("name")))
            .or_else(|| value_as_str(self.extra.get("fileName")))
            .or_else(|| value_as_str(self.extra.get("directoryName")))
    }

    fn object_key(&self, folder_prefix: &str) -> Result<String, BlobError> {
        let name = self.display_name().ok_or_else(|| {
            BlobError::Upstream(format!(
                "{QUERY_ALL_FILES_OPERATION} returned a file without a name"
            ))
        })?;
        Ok(join_relative_key(folder_prefix, name))
    }

    fn size_bytes(&self) -> u64 {
        self.file_size
            .or(self.size)
            .or_else(|| value_as_u64(self.extra.get("fileSize")))
            .or_else(|| value_as_u64(self.extra.get("size")))
            .unwrap_or(0)
    }

    fn normalized_last_modified(&self) -> Option<String> {
        self.update_time
            .as_deref()
            .or(self.last_update_time.as_deref())
            .or(self.create_time.as_deref())
            .or_else(|| value_as_str(self.extra.get("updateTime")))
            .or_else(|| value_as_str(self.extra.get("lastUpdateTime")))
            .or_else(|| value_as_str(self.extra.get("createTime")))
            .and_then(normalize_upstream_timestamp)
    }

    fn resolved_content_type(&self) -> Option<String> {
        self.content_type
            .as_deref()
            .or_else(|| value_as_str(self.extra.get("contentType")))
            .map(str::to_string)
            .or_else(|| {
                self.suffix
                    .as_deref()
                    .and_then(content_type_from_suffix)
                    .map(str::to_string)
            })
    }

    fn download_identifier_candidates(&self) -> Vec<DownloadIdentifierCandidate> {
        let mut candidates = Vec::new();
        let mut seen = BTreeSet::new();
        let mut push = |label: &'static str,
                        value: Option<String>,
                        candidates: &mut Vec<DownloadIdentifierCandidate>| {
            let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                return;
            };
            if seen.insert(value.clone()) {
                candidates.push(DownloadIdentifierCandidate { label, value });
            }
        };

        push(
            "fid",
            value_as_string_like(self.extra.get("fid")),
            &mut candidates,
        );
        push(
            "fileId",
            value_as_string_like(self.extra.get("fileId"))
                .or_else(|| value_as_string_like(self.extra.get("fileID"))),
            &mut candidates,
        );
        push("id", Some(self.id.clone()), &mut candidates);

        candidates
    }

    fn into_object_info(self, key: String) -> ObjectInfo {
        ObjectInfo {
            key,
            size: self.size_bytes(),
            etag: None,
            content_type: self.resolved_content_type(),
            last_modified: self.normalized_last_modified(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryAllFilesEntryKind {
    Directory,
    File,
}

fn trim_objects_to_limit(objects: &mut BTreeMap<String, ObjectInfo>, limit: usize) {
    while objects.len() > limit {
        let Some(last_key) = objects.keys().next_back().cloned() else {
            break;
        };
        objects.remove(&last_key);
    }
}

fn extract_download_url(operation: &str, data: Option<Value>) -> Result<String, BlobError> {
    let Some(data) = data else {
        return Err(BlobError::Upstream(format!(
            "{operation} returned no DATA payload"
        )));
    };

    if let Ok(entries) = serde_json::from_value::<Vec<DownloadUrlEntry>>(data.clone()) {
        if let Some(url) = entries
            .into_iter()
            .find_map(|entry| entry.download_url.or(entry.url))
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
        {
            return Ok(url);
        }
    }

    if let Some(url) = find_download_url(&data) {
        return Ok(url.to_string());
    }

    Err(BlobError::Upstream(format!(
        "{operation} returned DATA without a download URL"
    )))
}

fn find_download_url(value: &Value) -> Option<&str> {
    match value {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() || !trimmed.starts_with("http") {
                None
            } else {
                Some(trimmed)
            }
        }
        Value::Array(entries) => entries.iter().find_map(find_download_url),
        Value::Object(map) => value_as_str(map.get("downloadUrl"))
            .or_else(|| value_as_str(map.get("url")))
            .or_else(|| {
                [
                    "data",
                    "downloadUrls",
                    "downloadUrlList",
                    "urls",
                    "list",
                    "items",
                ]
                .into_iter()
                .filter_map(|key| map.get(key))
                .find_map(find_download_url)
            }),
        _ => None,
    }
}

fn directory_may_contain_prefix(directory_prefix: &str, prefix: &str) -> bool {
    if directory_prefix.is_empty() {
        return true;
    }

    prefix == directory_prefix
        || prefix.starts_with(&format!("{directory_prefix}/"))
        || directory_prefix.starts_with(prefix)
}

fn normalize_object_key(value: &str) -> String {
    value
        .split('/')
        .filter(|segment| !segment.is_empty())
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

fn value_as_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn value_as_string_like(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(Value::Number(number)) => Some(number.to_string()),
        _ => None,
    }
}

fn value_as_u64(value: Option<&Value>) -> Option<u64> {
    match value {
        Some(Value::Number(number)) => number.as_u64(),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    }
}

fn normalize_upstream_timestamp(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.len() == 14 && trimmed.chars().all(|char| char.is_ascii_digit()) {
        return Some(format!(
            "{}-{}-{}T{}:{}:{}.000Z",
            &trimmed[0..4],
            &trimmed[4..6],
            &trimmed[6..8],
            &trimmed[8..10],
            &trimmed[10..12],
            &trimmed[12..14],
        ));
    }

    Some(trimmed.to_string())
}

fn content_type_from_suffix(suffix: &str) -> Option<&'static str> {
    match suffix.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "txt" | "log" | "md" => Some("text/plain"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "mp4" => Some("video/mp4"),
        "mp3" => Some("audio/mpeg"),
        _ => None,
    }
}

fn deserialize_string_like<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(text) => Ok(text),
        Value::Number(number) => Ok(number.to_string()),
        other => Err(de::Error::custom(format!(
            "expected string-like value, got {}",
            value_kind(&other)
        ))),
    }
}

fn deserialize_optional_string_like<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(Value::Number(number)) => Ok(Some(number.to_string())),
        Some(other) => Err(de::Error::custom(format!(
            "expected optional string-like value, got {}",
            value_kind(&other)
        ))),
    }
}

fn deserialize_optional_u64_like<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| de::Error::custom("numeric value cannot be represented as u64"))
            .map(Some),
        Some(Value::String(text)) => text
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|error| de::Error::custom(format!("invalid u64 string: {error}"))),
        Some(other) => Err(de::Error::custom(format!(
            "expected optional u64-like value, got {}",
            value_kind(&other)
        ))),
    }
}

fn build_api_user_dispatcher_payload(
    operation: &str,
    channel: &str,
    dispatcher_client_id: &str,
    dispatcher_secret: &str,
    param_body: Value,
) -> Result<Value, BlobError> {
    if operation.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher operation is empty".to_string(),
        ));
    }
    if channel.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher channel is empty".to_string(),
        ));
    }
    if dispatcher_client_id.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher client id is empty".to_string(),
        ));
    }

    let res_time = current_unix_millis();
    let req_seq = 100_000 + (res_time % 89_999);
    let version = "";
    let sign = dispatcher_sign(operation, res_time, req_seq, channel, version);
    let param_body = serde_json::to_string(&param_body).map_err(|error| {
        BlobError::Upstream(format!("failed to encode dispatcher payload: {error}"))
    })?;
    let encrypted_param = aes_cbc_encrypt_base64(&param_body, dispatcher_secret)?;

    Ok(json!({
        "header": {
            "key": operation,
            "resTime": res_time,
            "reqSeq": req_seq,
            "channel": channel,
            "version": version,
            "sign": sign,
        },
        "body": {
            "param": encrypted_param,
            "clientId": dispatcher_client_id,
            "secret": true,
        }
    }))
}

fn build_wohome_dispatcher_payload(
    operation: &str,
    channel: &str,
    dispatcher_client_id: &str,
    token: &str,
    param_body: Value,
    encrypted_flag_name: &str,
) -> Result<Value, BlobError> {
    if operation.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher operation is empty".to_string(),
        ));
    }
    if channel.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher channel is empty".to_string(),
        ));
    }
    if dispatcher_client_id.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher client id is empty".to_string(),
        ));
    }
    if encrypted_flag_name.trim().is_empty() {
        return Err(BlobError::Configuration(
            "dispatcher encrypted flag name is empty".to_string(),
        ));
    }

    let mut body = match param_body {
        Value::Null => Map::new(),
        Value::Object(map) => map,
        _ => {
            return Err(BlobError::Configuration(
                "wohome dispatcher body must be a JSON object".to_string(),
            ));
        }
    };
    body.entry("clientId".to_string())
        .or_insert_with(|| Value::String(dispatcher_client_id.to_string()));

    let res_time = current_unix_millis();
    let req_seq = 100_000 + (res_time % 89_999);
    let version = "";
    let sign = dispatcher_sign(operation, res_time, req_seq, channel, version);
    let param_body = serde_json::to_string(&Value::Object(body)).map_err(|error| {
        BlobError::Upstream(format!("failed to encode dispatcher payload: {error}"))
    })?;
    let encrypted_param = aes_cbc_encrypt_base64(&param_body, token)?;

    let mut payload_body = Map::new();
    payload_body.insert("param".to_string(), Value::String(encrypted_param));
    payload_body.insert(encrypted_flag_name.to_string(), Value::Bool(true));

    Ok(json!({
        "header": {
            "key": operation,
            "resTime": res_time,
            "reqSeq": req_seq,
            "channel": channel,
            "version": version,
            "sign": sign,
        },
        "body": Value::Object(payload_body)
    }))
}

fn decode_dispatcher_data(
    style: AuthProbeStyle,
    data: Option<Value>,
    token: &str,
    dispatcher_secret: &str,
) -> Result<Option<Value>, BlobError> {
    let Some(data) = data else {
        return Ok(None);
    };

    match data {
        Value::String(ciphertext) if ciphertext.trim().is_empty() => Ok(None),
        Value::String(ciphertext) => {
            let plaintext = match style {
                AuthProbeStyle::ApiUserSecret => {
                    aes_cbc_decrypt_base64(&ciphertext, dispatcher_secret)?
                }
                AuthProbeStyle::WohomeKey | AuthProbeStyle::WohomeSecret => {
                    aes_cbc_decrypt_base64(&ciphertext, token)?
                }
            };

            let parsed =
                serde_json::from_str::<Value>(&plaintext).unwrap_or(Value::String(plaintext));
            Ok(Some(parsed))
        }
        other => Ok(Some(other)),
    }
}

fn extract_user_identifiers(value: &Value) -> (Option<String>, Option<String>) {
    let user_id = value
        .get("userId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let real_user_id = value
        .get("realUserId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (user_id, real_user_id)
}

fn parse_capacity_from_value(value: Option<&Value>) -> Option<StorageCapacity> {
    let value = value?;
    let total = find_first_u64_for_keys(
        value,
        &["capacity", "totalCapacity", "totalSpace", "spaceSize"],
    );
    let free = find_first_u64_for_keys(
        value,
        &["available", "free", "freeCapacity", "availableCapacity"],
    );
    let used =
        find_first_u64_for_keys(value, &["used", "usedCapacity", "usedSpace"]).or_else(|| {
            match (total, free) {
                (Some(total), Some(free)) if total >= free => Some(total - free),
                _ => None,
            }
        });

    if total.is_none() && used.is_none() && free.is_none() {
        None
    } else {
        Some(StorageCapacity {
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
        })
    }
}

fn find_first_string_for_keys(value: Option<&Value>, keys: &[&str]) -> Option<String> {
    let value = value?;
    find_first_string_for_keys_in_value(value, keys)
}

fn find_first_string_for_keys_in_value(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map
                    .get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|candidate| !candidate.is_empty())
                {
                    return Some(found.to_string());
                }
            }
            for child in map.values() {
                if let Some(found) = find_first_string_for_keys_in_value(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_first_string_for_keys_in_value(child, keys)),
        _ => None,
    }
}

fn find_first_u64_for_keys(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = value_as_u64(map.get(*key)) {
                    return Some(found);
                }
            }
            for child in map.values() {
                if let Some(found) = find_first_u64_for_keys(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_first_u64_for_keys(child, keys)),
        _ => None,
    }
}

fn top_level_keys(value: Option<&Value>) -> Vec<String> {
    let Some(Value::Object(map)) = value else {
        return Vec::new();
    };

    let mut keys = map.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    keys
}

fn value_kind(value: &Value) -> String {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
    .to_string()
}

fn dispatcher_sign(
    operation: &str,
    res_time: u64,
    req_seq: u64,
    channel: &str,
    version: &str,
) -> String {
    let mut hasher = Md5::new();
    hasher.update(operation.as_bytes());
    hasher.update(res_time.to_string().as_bytes());
    hasher.update(req_seq.to_string().as_bytes());
    hasher.update(channel.as_bytes());
    hasher.update(version.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn build_http_client(config: &UnicomConfig) -> Result<reqwest::Client, BlobError> {
    let mut builder = reqwest::Client::builder();
    if let Some(local_address) = config.outbound_ip_family.local_address() {
        builder = builder.local_address(local_address);
    }
    builder.build().map_err(|error| {
        BlobError::Configuration(format!("failed to build China Unicom HTTP client: {error}"))
    })
}

fn aes_cbc_encrypt_base64(plaintext: &str, secret: &str) -> Result<String, BlobError> {
    let key = normalize_aes128_secret(secret)?;
    let iv = normalize_aes128_iv()?;
    let mut buffer = plaintext.as_bytes().to_vec();
    let message_len = buffer.len();
    buffer.resize(message_len + 16, 0);

    let ciphertext = Aes128CbcEncryptor::new_from_slices(&key, &iv)
        .map_err(|error| BlobError::Configuration(format!("invalid AES key length: {error}")))?
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, message_len)
        .map_err(|error| BlobError::Upstream(format!("AES encrypt failed: {error}")))?;

    Ok(BASE64_STANDARD.encode(ciphertext))
}

fn aes_cbc_decrypt_base64(ciphertext: &str, secret: &str) -> Result<String, BlobError> {
    let key = normalize_aes128_secret(secret)?;
    let iv = normalize_aes128_iv()?;
    let mut buffer = BASE64_STANDARD
        .decode(ciphertext)
        .map_err(|error| BlobError::Upstream(format!("invalid base64 ciphertext: {error}")))?;

    let plaintext = Aes128CbcDecryptor::new_from_slices(&key, &iv)
        .map_err(|error| BlobError::Configuration(format!("invalid AES key length: {error}")))?
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|error| BlobError::Upstream(format!("AES decrypt failed: {error}")))?;

    String::from_utf8(plaintext.to_vec())
        .map_err(|error| BlobError::Upstream(format!("decrypted payload was not UTF-8: {error}")))
}

fn normalize_aes128_secret(secret: &str) -> Result<[u8; 16], BlobError> {
    let trimmed = secret.trim().as_bytes();
    if trimmed.len() < 16 {
        return Err(BlobError::Configuration(format!(
            "dispatcher secret/token must be at least 16 bytes, got {}",
            trimmed.len()
        )));
    }

    let mut key = [0_u8; 16];
    key.copy_from_slice(&trimmed[..16]);
    Ok(key)
}

fn normalize_aes128_iv() -> Result<[u8; 16], BlobError> {
    let iv = UNICOM_AES_IV.as_bytes();
    if iv.len() != 16 {
        return Err(BlobError::Configuration(format!(
            "unicom AES IV must be exactly 16 bytes, got {}",
            iv.len()
        )));
    }

    let mut normalized = [0_u8; 16];
    normalized.copy_from_slice(iv);
    Ok(normalized)
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

async fn response_to_error(response: Response, action: &str) -> BlobError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<failed to read response body>".to_string());

    BlobError::Upstream(format!(
        "{action} failed with HTTP {}: {}",
        status,
        body.trim()
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
        body::Body,
        extract::{Path, State},
        http::StatusCode,
        response::IntoResponse,
        routing::{get, post},
    };
    use blob_core::{
        BlobBackend, HealthStatus, ListObjectsRequest, OutboundIpFamily, StorageScopeKind,
        TokenSource,
    };

    use super::{
        APP_QUERY_USER_OPERATION, GET_DOWNLOAD_URL_OPERATION, GET_DOWNLOAD_URL_V2_OPERATION,
        GET_DOWNLOAD_URL_V3_OPERATION, QUERY_ALL_FILES_OPERATION, QUERY_FAMILY_GROUPS_OPERATION,
        UNICOM_ROOT_CONTAINER, UnicomBlobAdapter, UnicomConfig, aes_cbc_decrypt_base64,
        aes_cbc_encrypt_base64, build_api_user_dispatcher_payload, build_wohome_dispatcher_payload,
        dispatcher_sign,
    };
    use serde_json::{Value, json};

    type MockDownloadRoutes = BTreeMap<String, BTreeMap<String, String>>;

    #[derive(Clone)]
    struct MockDispatcherState {
        token: String,
        base_url: String,
        entries_by_parent: Arc<BTreeMap<String, Vec<Value>>>,
        file_bodies_by_id: Arc<BTreeMap<String, Vec<u8>>>,
        download_routes_by_operation: Arc<MockDownloadRoutes>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    struct MockServer {
        base_url: String,
        requests: Arc<Mutex<Vec<Value>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        async fn start(
            entries_by_parent: BTreeMap<String, Vec<Value>>,
            file_bodies_by_id: BTreeMap<String, Vec<u8>>,
            token: &str,
        ) -> Self {
            let download_routes_by_operation =
                default_download_routes_by_operation(&file_bodies_by_id);
            Self::start_with_download_routes(
                entries_by_parent,
                file_bodies_by_id,
                download_routes_by_operation,
                token,
            )
            .await
        }

        async fn start_with_download_routes(
            entries_by_parent: BTreeMap<String, Vec<Value>>,
            file_bodies_by_id: BTreeMap<String, Vec<u8>>,
            download_routes_by_operation: MockDownloadRoutes,
            token: &str,
        ) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock dispatcher listener");
            let addr = listener.local_addr().expect("mock dispatcher local addr");
            let base_url = format!("http://{addr}");
            let state = MockDispatcherState {
                token: token.to_string(),
                base_url: base_url.clone(),
                entries_by_parent: Arc::new(entries_by_parent),
                file_bodies_by_id: Arc::new(file_bodies_by_id),
                download_routes_by_operation: Arc::new(download_routes_by_operation),
                requests: requests.clone(),
            };

            let app = Router::new()
                .route("/wohome/dispatcher", post(mock_dispatcher))
                .route("/download/{id}", get(mock_download))
                .with_state(state);
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("mock dispatcher should stay available");
            });

            Self {
                base_url,
                requests,
                _task: task,
            }
        }

        fn requests(&self) -> Vec<Value> {
            self.requests
                .lock()
                .expect("mock dispatcher requests")
                .clone()
        }
    }

    fn default_download_routes_by_operation(
        file_bodies_by_id: &BTreeMap<String, Vec<u8>>,
    ) -> MockDownloadRoutes {
        let identity_routes = file_bodies_by_id
            .keys()
            .map(|id| (id.clone(), id.clone()))
            .collect::<BTreeMap<_, _>>();

        BTreeMap::from([
            (
                GET_DOWNLOAD_URL_OPERATION.to_string(),
                identity_routes.clone(),
            ),
            (
                GET_DOWNLOAD_URL_V2_OPERATION.to_string(),
                identity_routes.clone(),
            ),
            (GET_DOWNLOAD_URL_V3_OPERATION.to_string(), identity_routes),
        ])
    }

    async fn mock_dispatcher(
        State(state): State<MockDispatcherState>,
        Json(payload): Json<Value>,
    ) -> Json<Value> {
        let operation = payload["header"]["key"]
            .as_str()
            .expect("dispatcher operation in mock request");

        let ciphertext = payload["body"]["param"]
            .as_str()
            .expect("encrypted request body");
        let decrypted =
            aes_cbc_decrypt_base64(ciphertext, &state.token).expect("decrypt mock request body");
        let mut request_body: Value =
            serde_json::from_str(&decrypted).expect("parse mock request body JSON");
        if let Value::Object(ref mut map) = request_body {
            map.insert(
                "__operation".to_string(),
                Value::String(operation.to_string()),
            );
        }
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(request_body.clone());

        let (rsp_code, rsp_desc, response_data) = match operation {
            QUERY_ALL_FILES_OPERATION => {
                let parent_directory_id = request_body["parentDirectoryId"]
                    .as_str()
                    .expect("parentDirectoryId in mock request");
                let page_num = request_body["pageNum"]
                    .as_u64()
                    .expect("pageNum in mock request") as usize;
                let page_size = request_body["pageSize"]
                    .as_u64()
                    .expect("pageSize in mock request") as usize;
                let space_type = request_body["spaceType"].as_str().unwrap_or("0");
                let family_id = request_body["familyId"].as_str().unwrap_or("");
                let scoped_parent_key = if space_type == "1" && !family_id.is_empty() {
                    format!("family:{family_id}:{parent_directory_id}")
                } else {
                    parent_directory_id.to_string()
                };

                let entries = state
                    .entries_by_parent
                    .get(&scoped_parent_key)
                    .or_else(|| state.entries_by_parent.get(parent_directory_id))
                    .cloned()
                    .unwrap_or_default();
                let files = entries
                    .into_iter()
                    .skip(page_num * page_size)
                    .take(page_size)
                    .collect::<Vec<_>>();
                (
                    "0000",
                    "成功",
                    Some(json!({
                        "files": files,
                        "systemDirs": [],
                    })),
                )
            }
            APP_QUERY_USER_OPERATION => (
                "0000",
                "成功",
                Some(json!({
                    "userId": "mock-user",
                    "realUserId": "mock-real-user",
                    "usageInfo": {
                        "capacity": 2048u64,
                        "available": 512u64,
                    }
                })),
            ),
            QUERY_FAMILY_GROUPS_OPERATION => (
                "0000",
                "成功",
                Some(json!({
                    "familyGroups": [
                        {
                            "familyId": "family-001",
                            "name": "Mock Family"
                        }
                    ]
                })),
            ),
            GET_DOWNLOAD_URL_OPERATION
            | GET_DOWNLOAD_URL_V2_OPERATION
            | GET_DOWNLOAD_URL_V3_OPERATION => {
                let fid_list = request_body["fidList"]
                    .as_array()
                    .expect("fidList in mock download request");
                let Some(identifier) = fid_list.iter().find_map(Value::as_str) else {
                    panic!("mock download request should contain a string identifier");
                };
                match state
                    .download_routes_by_operation
                    .get(operation)
                    .and_then(|routes| routes.get(identifier))
                {
                    Some(target_id) => (
                        "0000",
                        "成功",
                        Some(Value::Array(vec![json!({
                            "downloadUrl": format!("{}/download/{}", state.base_url, target_id)
                        })])),
                    ),
                    None => ("9999", "系统异常", None),
                }
            }
            other => panic!("unexpected mock dispatcher operation: {other}"),
        };
        let encrypted_response = response_data
            .map(|response_data| {
                Value::String(
                    aes_cbc_encrypt_base64(
                        &serde_json::to_string(&response_data).expect("encode mock response"),
                        &state.token,
                    )
                    .expect("encrypt mock response"),
                )
            })
            .unwrap_or(Value::Null);

        Json(json!({
            "STATUS": "200",
            "RSP": {
                "RSP_CODE": rsp_code,
                "RSP_DESC": rsp_desc,
                "DATA": encrypted_response,
            }
        }))
    }

    async fn mock_download(
        State(state): State<MockDispatcherState>,
        Path(id): Path<String>,
    ) -> impl IntoResponse {
        match state.file_bodies_by_id.get(&id) {
            Some(body) => ResponseTuple(StatusCode::OK, body.clone()),
            None => ResponseTuple(StatusCode::NOT_FOUND, Vec::new()),
        }
    }

    struct ResponseTuple(StatusCode, Vec<u8>);

    impl IntoResponse for ResponseTuple {
        fn into_response(self) -> axum::response::Response {
            (self.0, Body::from(self.1)).into_response()
        }
    }

    fn mock_unicom_adapter(base_url: &str, token: &str) -> UnicomBlobAdapter {
        UnicomBlobAdapter::new(UnicomConfig {
            base_url: base_url.to_string(),
            token_source: TokenSource::Static {
                bearer: token.to_string(),
            },
            outbound_ip_family: OutboundIpFamily::Auto,
            cookie_header: None,
            user_agent: "carrier-cloud-blob-gateway/test".to_string(),
            request_timeout_secs: 10,
            request_origin: Some("https://pan.wo.cn".to_string()),
            request_referer: Some("https://pan.wo.cn/".to_string()),
            request_header_client_id: "1001000021".to_string(),
            request_header_app_version: "5g-h5".to_string(),
            dispatcher_client_id: "1001000021".to_string(),
            dispatcher_channel: "wohome".to_string(),
            dispatcher_secret: "Py1J67PAQoCb8Iel".to_string(),
            health_probe_operation: QUERY_ALL_FILES_OPERATION.to_string(),
            health_probe_style: "wohome-secret".to_string(),
            health_probe_body_json:
                "{\"spaceType\":\"0\",\"parentDirectoryId\":\"0\",\"pageNum\":0,\"pageSize\":50,\"sortRule\":0}"
                    .to_string(),
            family_id: None,
            family_space_type: "1".to_string(),
            family_root_directory_id: "0".to_string(),
        })
        .expect("unicom test adapter should build")
    }

    fn sample_entries() -> BTreeMap<String, Vec<Value>> {
        BTreeMap::from([
            (
                "0".to_string(),
                vec![
                    json!({
                        "id": "dir-docs",
                        "name": "docs",
                        "type": 0,
                        "parentDirectoryId": "0",
                    }),
                    json!({
                        "id": "dir-media",
                        "name": "media",
                        "type": 0,
                        "parentDirectoryId": "0",
                    }),
                    json!({
                        "id": "file-root",
                        "name": "zzz-root.txt",
                        "type": 1,
                        "parentDirectoryId": "0",
                        "fileSize": "7",
                        "updateTime": "20260425101010",
                        "suffix": "txt",
                    }),
                ],
            ),
            (
                "dir-docs".to_string(),
                vec![
                    json!({
                        "id": "file-alpha",
                        "name": "alpha.txt",
                        "type": 1,
                        "parentDirectoryId": "dir-docs",
                        "fileSize": 5,
                        "updateTime": "20260425123045",
                        "suffix": "txt",
                    }),
                    json!({
                        "id": "file-beta",
                        "name": "beta.json",
                        "type": 1,
                        "parentDirectoryId": "dir-docs",
                        "fileSize": "6",
                        "updateTime": "20260425123100",
                        "suffix": "json",
                    }),
                    json!({
                        "id": "dir-nested",
                        "name": "nested",
                        "type": 0,
                        "parentDirectoryId": "dir-docs",
                    }),
                ],
            ),
            (
                "dir-nested".to_string(),
                vec![json!({
                    "id": "file-zeta",
                    "name": "zeta.log",
                    "type": 1,
                    "parentDirectoryId": "dir-nested",
                    "fileSize": 9,
                    "updateTime": "20260425150000",
                    "suffix": "log",
                })],
            ),
            (
                "dir-media".to_string(),
                vec![json!({
                    "id": "file-photo",
                    "name": "photo.png",
                    "type": 1,
                    "parentDirectoryId": "dir-media",
                    "fileSize": 11,
                    "updateTime": "20260425140000",
                    "suffix": "png",
                })],
            ),
            (
                "family:family-001:0".to_string(),
                vec![
                    json!({
                        "id": "family-dir",
                        "name": "shared",
                        "type": 0,
                        "parentDirectoryId": "0",
                    }),
                    json!({
                        "id": "family-file",
                        "name": "family-note.txt",
                        "type": 1,
                        "parentDirectoryId": "0",
                        "fileSize": 12,
                        "updateTime": "20260425170000",
                        "suffix": "txt",
                    }),
                ],
            ),
        ])
    }

    fn sample_file_bodies() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("file-root".to_string(), b"root!!!".to_vec()),
            ("file-alpha".to_string(), b"alpha".to_vec()),
            ("file-beta".to_string(), b"123456".to_vec()),
            ("file-zeta".to_string(), b"123456789".to_vec()),
            ("file-photo".to_string(), b"hello-photo".to_vec()),
        ])
    }

    #[test]
    fn dispatcher_sign_matches_frontend_formula() {
        assert_eq!(
            dispatcher_sign("AppQueryUser", 1_714_212_345_678, 123_456, "api-user", ""),
            "b153e4d2392c71a96a445185ef8fa315"
        );
    }

    #[test]
    fn aes_roundtrip_truncates_long_secret_like_frontend() {
        let plaintext = r#"{"directoryId":"0","clientId":"1001000021"}"#;
        let secret = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let encrypted = aes_cbc_encrypt_base64(plaintext, secret).expect("encrypt payload");
        assert_eq!(
            aes_cbc_decrypt_base64(&encrypted, secret).expect("decrypt payload"),
            plaintext
        );
    }

    #[test]
    fn decrypts_real_query_all_files_payload() {
        let ciphertext = "YwYbyyw2efzIt1qglFdM/Qcammj3gHr+9rxpwAoNRg2I4xtpo5kgni1XbpZKC4SS8gP6YohCOr0Z8tb+gJIDOq3MVcKpojBSD2YYqgAGBfUnDg524lRzKzrTpaIKDT3R6mASeabxDgpbkh/a5qCCGQ==";
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let plaintext =
            aes_cbc_decrypt_base64(ciphertext, token).expect("decrypt real QueryAllFiles request");
        assert_eq!(
            plaintext,
            r#"{"spaceType":"0","parentDirectoryId":"0","pageNum":0,"pageSize":50,"sortRule":0,"clientId":"1001000021"}"#
        );
    }

    #[test]
    fn wohome_secret_payload_encrypts_client_id_inside_body() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let payload = build_wohome_dispatcher_payload(
            "GetSearchDirectory",
            "wohome",
            "1001000021",
            token,
            json!({ "directoryId": "0" }),
            "secret",
        )
        .expect("build payload");

        assert_eq!(payload["header"]["key"], "GetSearchDirectory");
        assert_eq!(payload["body"]["secret"], Value::Bool(true));
        assert!(payload["body"].get("clientId").is_none());

        let ciphertext = payload["body"]["param"]
            .as_str()
            .expect("ciphertext in payload");
        let decrypted =
            aes_cbc_decrypt_base64(ciphertext, token).expect("decrypt wohome dispatcher payload");
        let decrypted: Value =
            serde_json::from_str(&decrypted).expect("parse wohome decrypted body");

        assert_eq!(decrypted["directoryId"], "0");
        assert_eq!(decrypted["clientId"], "1001000021");
    }

    #[test]
    fn api_user_payload_keeps_client_id_outside_ciphertext() {
        let payload = build_api_user_dispatcher_payload(
            "AppQueryUser",
            "api-user",
            "1001000003",
            "Py1J67PAQoCb8Iel",
            json!({ "accessToken": "abc" }),
        )
        .expect("build payload");

        assert_eq!(payload["body"]["clientId"], "1001000003");
        assert_eq!(payload["body"]["secret"], Value::Bool(true));

        let ciphertext = payload["body"]["param"]
            .as_str()
            .expect("ciphertext in payload");
        let decrypted = aes_cbc_decrypt_base64(ciphertext, "Py1J67PAQoCb8Iel")
            .expect("decrypt api-user dispatcher payload");
        let decrypted: Value =
            serde_json::from_str(&decrypted).expect("parse api-user decrypted body");

        assert_eq!(decrypted["accessToken"], "abc");
    }

    #[tokio::test]
    async fn list_containers_exposes_root_bucket_and_uses_query_all_files() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let server = MockServer::start(sample_entries(), sample_file_bodies(), token).await;
        let adapter = mock_unicom_adapter(&server.base_url, token);

        let containers = adapter
            .list_containers()
            .await
            .expect("list mock Unicom containers");

        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, UNICOM_ROOT_CONTAINER);
        assert_eq!(containers[0].object_count, None);

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["__operation"], QUERY_ALL_FILES_OPERATION);
        assert_eq!(requests[0]["parentDirectoryId"], "0");
        assert_eq!(requests[0]["clientId"], "1001000021");
        assert_eq!(requests[0]["spaceType"], "0");
    }

    #[tokio::test]
    async fn health_reports_personal_and_family_scopes() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let server = MockServer::start(sample_entries(), sample_file_bodies(), token).await;
        let adapter = mock_unicom_adapter(&server.base_url, token);

        let health = adapter
            .health()
            .await
            .expect("unicom health should succeed");

        assert!(matches!(health.status, HealthStatus::Healthy));
        assert_eq!(health.scopes.len(), 2);
        assert_eq!(health.scopes[0].kind, StorageScopeKind::Personal);
        assert_eq!(
            health.scopes[0]
                .capacity
                .as_ref()
                .and_then(|capacity| capacity.total_bytes),
            Some(2048)
        );
        assert_eq!(health.scopes[1].kind, StorageScopeKind::Family);
        assert!(
            health
                .notes
                .iter()
                .any(|note| note.contains("family_root_entry_count=2"))
        );

        let operations = server
            .requests()
            .into_iter()
            .filter_map(|request| request["__operation"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(
            operations
                .iter()
                .any(|operation| operation == APP_QUERY_USER_OPERATION)
        );
        assert!(
            operations
                .iter()
                .any(|operation| operation == QUERY_FAMILY_GROUPS_OPERATION)
        );
    }

    #[tokio::test]
    async fn list_objects_recurses_and_applies_prefix_limit() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let server = MockServer::start(sample_entries(), sample_file_bodies(), token).await;
        let adapter = mock_unicom_adapter(&server.base_url, token);

        let all_objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some(UNICOM_ROOT_CONTAINER.to_string()),
                prefix: None,
                limit: None,
            })
            .await
            .expect("list all mock Unicom objects");

        let all_keys = all_objects
            .iter()
            .map(|object| object.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            all_keys,
            vec![
                "docs/alpha.txt",
                "docs/beta.json",
                "docs/nested/zeta.log",
                "media/photo.png",
                "zzz-root.txt",
            ]
        );
        assert_eq!(all_objects[0].size, 5);
        assert_eq!(
            all_objects[0].last_modified.as_deref(),
            Some("2026-04-25T12:30:45.000Z")
        );
        assert_eq!(all_objects[0].content_type.as_deref(), Some("text/plain"));

        let limited_objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some(UNICOM_ROOT_CONTAINER.to_string()),
                prefix: Some("docs/".to_string()),
                limit: Some(1),
            })
            .await
            .expect("list mock Unicom objects with prefix and limit");

        assert_eq!(limited_objects.len(), 1);
        assert_eq!(limited_objects[0].key, "docs/alpha.txt");

        let requests = server.requests();
        let requested_parents = requests
            .iter()
            .filter_map(|request| request["parentDirectoryId"].as_str())
            .collect::<Vec<_>>();
        assert!(requested_parents.contains(&"0"));
        assert!(requested_parents.contains(&"dir-docs"));
        assert!(requested_parents.contains(&"dir-nested"));
    }

    #[tokio::test]
    async fn head_and_get_object_use_precise_lookup_and_download_url_flow() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let server = MockServer::start(sample_entries(), sample_file_bodies(), token).await;
        let adapter = mock_unicom_adapter(&server.base_url, token);

        let object = adapter
            .head_object(UNICOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("head mock Unicom object");
        assert_eq!(object.key, "docs/alpha.txt");
        assert_eq!(object.size, 5);
        assert_eq!(object.content_type.as_deref(), Some("text/plain"));

        let payload = adapter
            .get_object(UNICOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("download mock Unicom object");
        assert_eq!(payload.info.key, "docs/alpha.txt");
        assert_eq!(payload.body, b"alpha");

        let requests = server.requests();
        let operations = requests
            .iter()
            .filter_map(|request| request["__operation"].as_str())
            .collect::<Vec<_>>();
        assert!(operations.contains(&QUERY_ALL_FILES_OPERATION));
        assert!(operations.contains(&GET_DOWNLOAD_URL_OPERATION));
        let download_request = requests
            .iter()
            .find(|request| request["__operation"] == GET_DOWNLOAD_URL_OPERATION)
            .expect("download request should be recorded");
        assert_eq!(download_request["fidList"][0], "file-alpha");
        assert_eq!(download_request["spaceType"], "0");
    }

    #[tokio::test]
    async fn get_object_falls_back_to_fid_and_v2_download_operation() {
        let token = "341e39ff-91a6-4a86-9a98-6cd41501b2a8";
        let mut entries = sample_entries();
        let docs_entries = entries
            .get_mut("dir-docs")
            .expect("docs directory should exist in sample entries");
        let alpha_entry = docs_entries
            .iter_mut()
            .find(|entry| entry["id"] == "file-alpha")
            .expect("alpha entry should exist");
        alpha_entry["fid"] = Value::String("fid-alpha".to_string());

        let download_routes = BTreeMap::from([(
            GET_DOWNLOAD_URL_V2_OPERATION.to_string(),
            BTreeMap::from([("fid-alpha".to_string(), "file-alpha".to_string())]),
        )]);
        let server = MockServer::start_with_download_routes(
            entries,
            sample_file_bodies(),
            download_routes,
            token,
        )
        .await;
        let adapter = mock_unicom_adapter(&server.base_url, token);

        let payload = adapter
            .get_object(UNICOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("download mock Unicom object through fid/v2 fallback");
        assert_eq!(payload.info.key, "docs/alpha.txt");
        assert_eq!(payload.body, b"alpha");

        let requests = server.requests();
        let download_requests = requests
            .iter()
            .filter(|request| {
                matches!(
                    request["__operation"].as_str(),
                    Some(
                        GET_DOWNLOAD_URL_OPERATION
                            | GET_DOWNLOAD_URL_V2_OPERATION
                            | GET_DOWNLOAD_URL_V3_OPERATION
                    )
                )
            })
            .collect::<Vec<_>>();
        assert!(!download_requests.is_empty());
        assert_eq!(download_requests[0]["fidList"][0], "fid-alpha");
        assert!(
            download_requests
                .iter()
                .any(|request| request["__operation"] == GET_DOWNLOAD_URL_OPERATION)
        );
        let v2_request = download_requests
            .iter()
            .find(|request| request["__operation"] == GET_DOWNLOAD_URL_V2_OPERATION)
            .expect("GetDownloadUrlV2 request should be recorded");
        assert_eq!(v2_request["fidList"][0], "fid-alpha");
    }
}
