use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use aes::{
    Aes128,
    cipher::{BlockEncryptMut, KeyInit, block_padding::Pkcs7},
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, BodySpoolLease, BrowserRequestProfile,
    ContainerInfo, CopyObjectRequest, HealthStatus, ListObjectsRequest, MoveObjectRequest,
    ObjectBody, ObjectInfo, ObjectPayload, OutboundIpFamily, PutObjectRequest, PutObjectResult,
    RenameObjectRequest, ServiceHealth, SharedBodySpoolObserver, StorageCapacity,
    StorageScopeHealth, StorageScopeKind, StreamFirstProgressObserver, TokenSource,
};
use ecb::Encryptor;
use futures_util::{StreamExt, stream::try_unfold};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rand::{Rng, rngs::OsRng};
use reqwest::{
    Method, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderName, HeaderValue, REFERER, USER_AGENT},
};
use rsa::{Pkcs1v15Encrypt, RsaPublicKey, pkcs1::DecodeRsaPublicKey, pkcs8::DecodePublicKey};
use serde::{Deserialize, Deserializer, Serialize, de, de::DeserializeOwned};
use serde_json::Value;
use sha1::Sha1;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use tokio::{
    fs::File as TokioFile,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    time::{sleep, timeout},
};

const TELECOM_ROOT_CONTAINER: &str = "root";
const TELECOM_FAMILY_CONTAINER: &str = "family";
const DEFAULT_ROOT_FOLDER_ID: &str = "-11";
const DEFAULT_FAMILY_ROOT_FOLDER_ID: &str = "home";
const DEFAULT_PAGE_SIZE: usize = 60;
const DEFAULT_UPLOAD_PART_SIZE_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_UPLOAD_CONTROL_BASE_URL: &str = "https://upload.cloud.189.cn";
const TELECOM_BATCH_POLL_INTERVAL_MS: u64 = 500;
const URI_COMPONENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelecomConfig {
    pub base_url: String,
    pub token_source: TokenSource,
    pub outbound_ip_family: OutboundIpFamily,
    pub browser_id: Option<String>,
    pub cookie_header: Option<String>,
    pub user_agent: String,
    #[serde(default)]
    pub browser_profile: Option<BrowserRequestProfile>,
    pub request_timeout_secs: u64,
    pub sign_type: String,
    #[serde(default)]
    pub family_id: Option<String>,
    pub root_folder_id: String,
    pub page_size: usize,
    #[serde(default)]
    pub root_prefix: Option<String>,
    #[serde(default)]
    pub upload_part_size_bytes: u64,
    #[serde(default)]
    pub max_single_upload_bytes: Option<u64>,
    #[serde(default)]
    pub max_single_download_bytes: Option<u64>,
    #[serde(default)]
    pub body_spool_dir: Option<String>,
    #[serde(skip, default)]
    pub body_spool_observer: Option<SharedBodySpoolObserver>,
}

pub struct TelecomBlobAdapter {
    config: TelecomConfig,
    client: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelecomContainerScope {
    Personal,
    Family,
}

#[derive(Debug, Deserialize)]
struct TelecomListFilesResponse {
    #[serde(default, deserialize_with = "deserialize_optional_string_like")]
    res_code: Option<String>,
    #[serde(default)]
    res_message: Option<String>,
    #[serde(rename = "fileListAO")]
    file_list_ao: Option<TelecomFileListAo>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelecomFileListAo {
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    count: Option<u64>,
    #[serde(rename = "fileList", default)]
    file_list: Vec<TelecomFileEntry>,
    #[serde(rename = "folderList", default)]
    folder_list: Vec<TelecomFolderEntry>,
}

#[derive(Debug, Deserialize)]
struct TelecomDownloadUrlResponse {
    #[serde(default, deserialize_with = "deserialize_optional_string_like")]
    res_code: Option<String>,
    #[serde(default)]
    res_message: Option<String>,
    #[serde(rename = "fileDownloadUrl", default)]
    file_download_url: Option<String>,
    #[serde(rename = "downloadUrl", default)]
    download_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelecomUserInfoResponse {
    #[serde(default, deserialize_with = "deserialize_optional_string_like")]
    res_code: Option<String>,
    #[serde(default)]
    res_message: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    capacity: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    available: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelecomFolderEntry {
    #[serde(default, deserialize_with = "deserialize_optional_string_like")]
    id: Option<String>,
    #[serde(
        rename = "parentId",
        default,
        deserialize_with = "deserialize_optional_string_like"
    )]
    _parent_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "fileName", default)]
    file_name: Option<String>,
    #[serde(rename = "createDate", default)]
    _create_date: Option<String>,
    #[serde(rename = "lastOpTime", default)]
    _last_op_time: Option<String>,
    #[serde(
        rename = "fileCount",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    _file_count: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct TelecomFileEntry {
    #[serde(default, deserialize_with = "deserialize_optional_string_like")]
    id: Option<String>,
    #[serde(
        rename = "fileId",
        default,
        deserialize_with = "deserialize_optional_string_like"
    )]
    file_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "fileName", default)]
    file_name: Option<String>,
    #[serde(
        rename = "parentId",
        default,
        deserialize_with = "deserialize_optional_string_like"
    )]
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    size: Option<u64>,
    #[serde(
        rename = "fileSize",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    file_size: Option<u64>,
    #[serde(rename = "createDate", default)]
    create_date: Option<String>,
    #[serde(rename = "lastOpTime", default)]
    last_op_time: Option<String>,
    #[serde(rename = "contentType", default)]
    content_type: Option<String>,
    #[serde(rename = "fileType", default)]
    file_type: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(rename = "downloadUrl", default)]
    download_url: Option<String>,
}

#[derive(Debug)]
struct TimedObjectBody {
    body: ObjectBody,
    first_response_latency_ms: u64,
}

#[derive(Debug)]
struct PreparedTelecomUpload {
    spool_file: NamedTempFile,
    size: u64,
    file_md5_upper: String,
    slice_md5_upper: String,
    part_md5_upper: Vec<String>,
    part_md5_base64: Vec<String>,
    part_size_bytes: u64,
    _spool_lease: Option<Box<dyn BodySpoolLease>>,
}

#[derive(Debug, Clone)]
struct TelecomUploadRsaKey {
    pk_id: String,
    pub_key: String,
}

#[derive(Debug, Clone)]
struct TelecomUploadBootstrap {
    session_key: String,
    rsa_key: TelecomUploadRsaKey,
}

#[derive(Debug)]
struct TelecomUploadPlan {
    upload_host: String,
    upload_file_id: String,
    file_data_exists: bool,
}

type HmacSha1 = Hmac<Sha1>;

impl TelecomFileListAo {
    fn fetched_count(&self) -> usize {
        self.file_list.len() + self.folder_list.len()
    }
}

impl TelecomFolderEntry {
    fn id(&self) -> Option<&str> {
        self.id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.file_name.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

impl TelecomFileEntry {
    fn id(&self) -> Option<&str> {
        self.id
            .as_deref()
            .or(self.file_id.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.file_name.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn parent_id(&self) -> Option<&str> {
        self.parent_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn resolved_size(&self) -> u64 {
        self.size.or(self.file_size).unwrap_or(0)
    }

    fn resolved_last_modified(&self) -> Option<String> {
        self.last_op_time
            .as_deref()
            .or(self.create_date.as_deref())
            .map(normalize_timestamp)
    }

    fn resolved_content_type(&self) -> Option<String> {
        self.content_type.clone().or_else(|| {
            self.display_name()
                .and_then(|name| guess_content_type(name).map(str::to_string))
                .or_else(|| {
                    self.file_type.as_deref().and_then(|suffix| {
                        guess_content_type_from_suffix(suffix).map(str::to_string)
                    })
                })
        })
    }

    fn object_key(&self, prefix: &str) -> Result<String, BlobError> {
        let name = self.display_name().ok_or_else(|| {
            BlobError::Upstream("listFiles.action returned a file without a name".to_string())
        })?;
        Ok(join_relative_key(prefix, name))
    }

    fn to_object_info(&self, key: String) -> ObjectInfo {
        ObjectInfo {
            key,
            size: self.resolved_size(),
            etag: self.md5.clone(),
            content_type: self.resolved_content_type(),
            last_modified: self.resolved_last_modified(),
        }
    }
}

impl TelecomBlobAdapter {
    pub fn new(config: TelecomConfig) -> Result<Self, BlobError> {
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

    fn effective_user_agent(&self) -> &str {
        self.config
            .browser_profile
            .as_ref()
            .and_then(BrowserRequestProfile::effective_user_agent)
            .unwrap_or(self.config.user_agent.as_str())
    }

    fn profile_header(&self, header_name: &str) -> Option<&str> {
        self.config
            .browser_profile
            .as_ref()
            .and_then(|profile| profile.header(header_name))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn sign_type(&self) -> &str {
        let configured = self.config.sign_type.trim();
        if configured.is_empty() {
            "1"
        } else {
            configured
        }
    }

    fn root_folder_id(&self) -> &str {
        let folder_id = self.config.root_folder_id.trim();
        if folder_id.is_empty() {
            DEFAULT_ROOT_FOLDER_ID
        } else {
            folder_id
        }
    }

    fn page_size(&self) -> usize {
        if self.config.page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            self.config.page_size
        }
    }

    fn upload_part_size_bytes(&self) -> u64 {
        if self.config.upload_part_size_bytes == 0 {
            DEFAULT_UPLOAD_PART_SIZE_BYTES
        } else {
            self.config.upload_part_size_bytes
        }
    }

    fn configured_family_id(&self) -> Option<&str> {
        self.config
            .family_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn family_id(&self) -> Result<&str, BlobError> {
        self.configured_family_id().ok_or_else(|| {
            BlobError::Configuration(
                "missing China Telecom Family ID; capture a family session in Admin Web or set CCBG_TELECOM_FAMILY_ID"
                    .to_string(),
            )
        })
    }

    fn normalized_root_prefix(&self) -> Option<String> {
        self.config
            .root_prefix
            .as_deref()
            .map(normalize_object_key)
            .filter(|value| !value.is_empty())
    }

    fn managed_container_root(&self, container: &str) -> Result<String, BlobError> {
        self.validate_container(container)?;
        Ok(self.normalized_root_prefix().unwrap_or_default())
    }

    fn provider_object_key(&self, container: &str, key: &str) -> Result<String, BlobError> {
        self.validate_container(container)?;
        let key = normalize_object_key(key);
        if key.is_empty() {
            return Err(BlobError::NotFound("object key is empty".to_string()));
        }
        let managed_root = self.managed_container_root(container)?;
        if managed_root.is_empty() {
            Ok(key)
        } else {
            Ok(join_relative_key(managed_root.as_str(), key.as_str()))
        }
    }

    fn user_visible_object_key(
        &self,
        container: &str,
        provider_key: &str,
    ) -> Result<String, BlobError> {
        self.validate_container(container)?;
        let provider_key = normalize_object_key(provider_key);
        if provider_key.is_empty() {
            return Err(BlobError::NotFound("object key is empty".to_string()));
        }

        let managed_root = self.managed_container_root(container)?;
        if managed_root.is_empty() {
            return Ok(provider_key);
        }

        let prefix = format!("{managed_root}/");
        provider_key
            .strip_prefix(prefix.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                BlobError::NotFound(format!(
                    "object is outside managed root for {container}: {provider_key}"
                ))
            })
    }

    fn browser_id(&self) -> Result<&str, BlobError> {
        self.config
            .browser_id
            .as_deref()
            .or_else(|| self.profile_header("browser-id"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "missing China Telecom Browser ID; set it in Admin Web or CCBG_TELECOM_BROWSER_ID".to_string(),
                )
            })
    }

    fn cookie_header(&self) -> Result<&str, BlobError> {
        self.config
            .cookie_header
            .as_deref()
            .or_else(|| self.profile_header("cookie"))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "missing China Telecom Cookie Header; set it in Admin Web or CCBG_TELECOM_COOKIE_HEADER".to_string(),
                )
            })
    }

    fn optional_token(&self) -> Option<String> {
        self.config
            .token_source
            .load()
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn apply_browser_profile_headers(
        &self,
        mut request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, BlobError> {
        let Some(profile) = self.config.browser_profile.as_ref() else {
            return Ok(request);
        };
        for (name, value) in profile.forwarded_headers(&[
            "accept",
            "accesstoken",
            "browser-id",
            "cookie",
            "referer",
            "sign-type",
            "user-agent",
        ]) {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid forwarded China Telecom browser profile header name {name}: {error}"
                ))
            })?;
            let header_value = HeaderValue::from_str(value.as_str()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid forwarded China Telecom browser profile header {name}: {error}"
                ))
            })?;
            request = request.header(header_name, header_value);
        }
        Ok(request)
    }

    fn request(
        &self,
        method: Method,
        url: &str,
        referer: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, BlobError> {
        let browser_id = self.browser_id()?;
        let cookie_header = self.cookie_header()?;
        let mut request = self
            .client
            .request(method, url)
            .header(USER_AGENT, self.effective_user_agent())
            .header(ACCEPT, "application/json;charset=UTF-8")
            .header("Browser-Id", browser_id)
            .header("Sign-Type", self.sign_type())
            .timeout(self.timeout());

        request = request.header(
            COOKIE,
            HeaderValue::from_str(cookie_header).map_err(|error| {
                BlobError::Configuration(format!("invalid CCBG_TELECOM_COOKIE_HEADER: {error}"))
            })?,
        );

        if let Some(referer) = referer
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| self.profile_header("referer"))
        {
            request = request.header(
                REFERER,
                HeaderValue::from_str(referer).map_err(|error| {
                    BlobError::Configuration(format!(
                        "invalid China Telecom Referer header value: {error}"
                    ))
                })?,
            );
        }

        self.apply_browser_profile_headers(request)
    }

    fn list_files_url(&self) -> String {
        format!("{}/api/open/file/listFiles.action", self.trimmed_base_url())
    }

    fn download_url_api(&self) -> String {
        format!(
            "{}/api/open/file/getFileDownloadUrl.action",
            self.trimmed_base_url()
        )
    }

    fn family_api_base_url(&self) -> String {
        if self.trimmed_base_url().contains("cloud.189.cn") {
            "https://api.cloud.189.cn".to_string()
        } else {
            self.trimmed_base_url().to_string()
        }
    }

    fn family_list_files_url(&self) -> String {
        format!(
            "{}/open/family/file/listFiles.action",
            self.family_api_base_url()
        )
    }

    fn family_download_url_api(&self) -> String {
        format!(
            "{}/open/family/file/getFileDownloadUrl.action",
            self.family_api_base_url()
        )
    }

    fn personal_batch_create_task_url(&self) -> String {
        format!(
            "{}/api/portal/createBatchTask.action",
            self.trimmed_base_url()
        )
    }

    fn personal_batch_check_task_url(&self) -> String {
        format!(
            "{}/api/portal/checkBatchTask.action",
            self.trimmed_base_url()
        )
    }

    fn family_batch_create_task_url(&self) -> String {
        format!(
            "{}/open/batch/createBatchTask.action",
            self.family_api_base_url()
        )
    }

    fn family_batch_check_task_url(&self) -> String {
        format!(
            "{}/open/batch/checkBatchTask.action",
            self.family_api_base_url()
        )
    }

    fn user_info_url(&self) -> String {
        format!(
            "{}/api/open/user/getUserInfoForPortal.action",
            self.trimmed_base_url()
        )
    }

    fn folder_referer(&self, folder_id: &str) -> String {
        format!(
            "{}/web/main/file/folder/{folder_id}",
            self.trimmed_base_url()
        )
    }

    fn main_referer(&self) -> String {
        format!("{}/web/main/", self.trimmed_base_url())
    }

    fn family_referer(&self, folder_id: &str) -> String {
        format!(
            "{}/web/family/file/folder/{folder_id}",
            self.trimmed_base_url()
        )
    }

    fn apply_signed_headers(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
        extra_params: &[(String, String)],
    ) -> reqwest::RequestBuilder {
        let timestamp = current_unix_ms().to_string();
        let mut pairs = vec![
            ("AccessToken".to_string(), token.to_string()),
            ("Timestamp".to_string(), timestamp.clone()),
        ];
        pairs.extend(extra_params.iter().cloned());
        let signature = telecom_signature(&pairs);

        request
            .header("AccessToken", token)
            .header("Timestamp", timestamp)
            .header("Signature", signature)
    }

    async fn send_get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        referer: &str,
        query_params: &[(String, String)],
        signed_token: Option<&str>,
        action: &str,
    ) -> Result<T, BlobError> {
        let mut query = query_params.to_vec();
        if signed_token.is_none() {
            query.push(("noCache".to_string(), random_nocache_value()));
        }

        let mut request = self.request(Method::GET, url, Some(referer))?;
        request = request.query(&query);
        if let Some(token) = signed_token {
            request = self.apply_signed_headers(request, token, query_params);
        }

        let response = request
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(BlobError::Upstream(format!(
                "{action} rejected the current China Telecom web session with HTTP 401"
            )));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        response.json::<T>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn send_signed_form_value(
        &self,
        url: &str,
        referer: &str,
        form_fields: &[(String, String)],
        action: &str,
    ) -> Result<Value, BlobError> {
        let token = self.optional_token().ok_or_else(|| {
            BlobError::Configuration(format!(
                "{action} requires China Telecom Access Token; capture the current session in Admin Web or set CCBG_TELECOM_TOKEN"
            ))
        })?;
        let mut request = self
            .request(Method::POST, url, Some(referer))?
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        request = self.apply_signed_headers(request, token.as_str(), form_fields);

        let response =
            request.form(form_fields).send().await.map_err(|error| {
                BlobError::Upstream(format!("{action} request failed: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }
        response.json::<Value>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn list_files_page_with_signature(
        &self,
        folder_id: &str,
        page_num: usize,
        signed_token: Option<&str>,
    ) -> Result<TelecomFileListAo, BlobError> {
        let query = vec![
            ("pageSize".to_string(), self.page_size().to_string()),
            ("pageNum".to_string(), page_num.to_string()),
            ("mediaType".to_string(), "0".to_string()),
            ("folderId".to_string(), folder_id.to_string()),
            ("iconOption".to_string(), "5".to_string()),
            ("orderBy".to_string(), "lastOpTime".to_string()),
            ("descending".to_string(), "true".to_string()),
        ];
        let response = self
            .send_get_json::<TelecomListFilesResponse>(
                self.list_files_url().as_str(),
                self.folder_referer(folder_id).as_str(),
                &query,
                signed_token,
                "listFiles.action",
            )
            .await?;
        ensure_success_code(
            response.res_code.as_deref(),
            response.res_message.as_deref(),
            "listFiles.action",
        )?;
        response.file_list_ao.ok_or_else(|| {
            BlobError::Upstream("listFiles.action returned no fileListAO payload".to_string())
        })
    }

    async fn list_files_page(
        &self,
        folder_id: &str,
        page_num: usize,
    ) -> Result<TelecomFileListAo, BlobError> {
        let unsigned_error = match self
            .list_files_page_with_signature(folder_id, page_num, None)
            .await
        {
            Ok(page) => return Ok(page),
            Err(error) => error,
        };

        let Some(token) = self.optional_token() else {
            return Err(unsigned_error);
        };

        match self
            .list_files_page_with_signature(folder_id, page_num, Some(token.as_str()))
            .await
        {
            Ok(page) => Ok(page),
            Err(signed_error) => Err(BlobError::Upstream(format!(
                "unsigned listFiles.action failed: {unsigned_error}; signed retry failed: {signed_error}"
            ))),
        }
    }

    async fn family_list_files_page(
        &self,
        folder_id: &str,
        page_num: usize,
    ) -> Result<TelecomFileListAo, BlobError> {
        let family_id = self.family_id()?;
        let token = self.optional_token().ok_or_else(|| {
            BlobError::Configuration(
                "China Telecom family listing requires Access Token; capture the family session in Admin Web or set CCBG_TELECOM_TOKEN"
                    .to_string(),
            )
        })?;
        let query = vec![
            ("pageSize".to_string(), self.page_size().to_string()),
            ("pageNum".to_string(), page_num.to_string()),
            ("mediaType".to_string(), "0".to_string()),
            ("familyId".to_string(), family_id.to_string()),
            ("folderId".to_string(), folder_id.to_string()),
            ("iconOption".to_string(), "5".to_string()),
            ("orderBy".to_string(), "lastOpTime".to_string()),
            ("descending".to_string(), "true".to_string()),
        ];
        let response = self
            .send_get_json::<TelecomListFilesResponse>(
                self.family_list_files_url().as_str(),
                self.family_referer(folder_id).as_str(),
                &query,
                Some(token.as_str()),
                "family listFiles.action",
            )
            .await?;
        if response.res_code.is_some() {
            ensure_success_code(
                response.res_code.as_deref(),
                response.res_message.as_deref(),
                "family listFiles.action",
            )?;
        }
        response.file_list_ao.ok_or_else(|| {
            BlobError::Upstream(
                "family listFiles.action returned no fileListAO payload".to_string(),
            )
        })
    }

    async fn list_files_page_for_scope(
        &self,
        scope: TelecomContainerScope,
        folder_id: &str,
        page_num: usize,
    ) -> Result<TelecomFileListAo, BlobError> {
        match scope {
            TelecomContainerScope::Personal => self.list_files_page(folder_id, page_num).await,
            TelecomContainerScope::Family => self.family_list_files_page(folder_id, page_num).await,
        }
    }

    async fn download_url_with_signature(
        &self,
        file_id: &str,
        signed_token: Option<&str>,
    ) -> Result<String, BlobError> {
        let query = vec![
            ("fileId".to_string(), file_id.to_string()),
            ("dt".to_string(), "1".to_string()),
            ("shareId".to_string(), String::new()),
        ];
        let response = self
            .send_get_json::<TelecomDownloadUrlResponse>(
                self.download_url_api().as_str(),
                self.main_referer().as_str(),
                &query,
                signed_token,
                "getFileDownloadUrl.action",
            )
            .await?;
        ensure_success_code(
            response.res_code.as_deref(),
            response.res_message.as_deref(),
            "getFileDownloadUrl.action",
        )?;

        let raw_url = response
            .file_download_url
            .or(response.download_url)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "getFileDownloadUrl.action returned no fileDownloadUrl".to_string(),
                )
            })?;
        Ok(normalize_remote_url(
            self.trimmed_base_url(),
            raw_url.as_str(),
        ))
    }

    async fn download_url_for_file(&self, file_id: &str) -> Result<String, BlobError> {
        let unsigned_error = match self.download_url_with_signature(file_id, None).await {
            Ok(url) => return Ok(url),
            Err(error) => error,
        };

        let Some(token) = self.optional_token() else {
            return Err(unsigned_error);
        };

        match self
            .download_url_with_signature(file_id, Some(token.as_str()))
            .await
        {
            Ok(url) => Ok(url),
            Err(signed_error) => Err(BlobError::Upstream(format!(
                "unsigned getFileDownloadUrl.action failed: {unsigned_error}; signed retry failed: {signed_error}"
            ))),
        }
    }

    async fn family_download_url_for_file(&self, file_id: &str) -> Result<String, BlobError> {
        let family_id = self.family_id()?;
        let token = self.optional_token().ok_or_else(|| {
            BlobError::Configuration(
                "China Telecom family download requires Access Token; capture the family session in Admin Web or set CCBG_TELECOM_TOKEN"
                    .to_string(),
            )
        })?;
        let query = vec![
            ("familyId".to_string(), family_id.to_string()),
            ("fileId".to_string(), file_id.to_string()),
            ("type".to_string(), "1".to_string()),
        ];
        let response = self
            .send_get_json::<TelecomDownloadUrlResponse>(
                self.family_download_url_api().as_str(),
                self.family_referer(DEFAULT_FAMILY_ROOT_FOLDER_ID).as_str(),
                &query,
                Some(token.as_str()),
                "family getFileDownloadUrl.action",
            )
            .await?;
        if response.res_code.is_some() {
            ensure_success_code(
                response.res_code.as_deref(),
                response.res_message.as_deref(),
                "family getFileDownloadUrl.action",
            )?;
        }
        let raw_url = response
            .file_download_url
            .or(response.download_url)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "family getFileDownloadUrl.action returned no fileDownloadUrl".to_string(),
                )
            })?;
        Ok(normalize_remote_url(
            self.trimmed_base_url(),
            raw_url.as_str(),
        ))
    }

    async fn download_url_for_scope(
        &self,
        scope: TelecomContainerScope,
        file_id: &str,
    ) -> Result<String, BlobError> {
        match scope {
            TelecomContainerScope::Personal => self.download_url_for_file(file_id).await,
            TelecomContainerScope::Family => self.family_download_url_for_file(file_id).await,
        }
    }

    async fn user_info_with_signature(
        &self,
        signed_token: Option<&str>,
    ) -> Result<TelecomUserInfoResponse, BlobError> {
        let response = self
            .send_get_json::<TelecomUserInfoResponse>(
                self.user_info_url().as_str(),
                self.main_referer().as_str(),
                &[],
                signed_token,
                "getUserInfoForPortal.action",
            )
            .await?;
        ensure_success_code(
            response.res_code.as_deref(),
            response.res_message.as_deref(),
            "getUserInfoForPortal.action",
        )?;
        Ok(response)
    }

    async fn user_info(&self) -> Result<TelecomUserInfoResponse, BlobError> {
        let unsigned_error = match self.user_info_with_signature(None).await {
            Ok(response) => return Ok(response),
            Err(error) => error,
        };

        let Some(token) = self.optional_token() else {
            return Err(unsigned_error);
        };

        match self.user_info_with_signature(Some(token.as_str())).await {
            Ok(response) => Ok(response),
            Err(signed_error) => Err(BlobError::Upstream(format!(
                "unsigned getUserInfoForPortal.action failed: {unsigned_error}; signed retry failed: {signed_error}"
            ))),
        }
    }

    fn personal_scope_health(
        &self,
        root_page: &TelecomFileListAo,
        user_info: Option<&TelecomUserInfoResponse>,
        notes: &mut Vec<String>,
    ) -> StorageScopeHealth {
        let object_count = Some(root_page.count.unwrap_or(root_page.fetched_count() as u64));
        let capacity = user_info.and_then(|response| {
            let total = response.capacity;
            let free = response.available;
            let used = match (total, free) {
                (Some(total), Some(free)) if total >= free => Some(total - free),
                _ => None,
            };

            if total.is_none() && free.is_none() && used.is_none() {
                None
            } else {
                Some(StorageCapacity {
                    total_bytes: total,
                    used_bytes: used,
                    free_bytes: free,
                })
            }
        });

        if let Some(capacity) = capacity.as_ref() {
            if let Some(total) = capacity.total_bytes {
                notes.push(format!("personal_capacity_total_bytes={total}"));
            }
            if let Some(free) = capacity.free_bytes {
                notes.push(format!("personal_capacity_free_bytes={free}"));
            }
        } else {
            notes.push("personal_capacity=unknown".to_string());
        }

        StorageScopeHealth {
            id: "personal".to_string(),
            label: "Personal Cloud".to_string(),
            kind: StorageScopeKind::Personal,
            writable: true,
            root: Some(self.root_folder_id().to_string()),
            container: Some(TELECOM_ROOT_CONTAINER.to_string()),
            object_count,
            capacity,
            notes: vec!["backed by cloud.189.cn web session".to_string()],
        }
    }

    fn family_scope_health(
        &self,
        family_id: &str,
        root_page: &TelecomFileListAo,
    ) -> StorageScopeHealth {
        StorageScopeHealth {
            id: family_id.to_string(),
            label: "Family Cloud".to_string(),
            kind: StorageScopeKind::Family,
            writable: false,
            root: Some(DEFAULT_FAMILY_ROOT_FOLDER_ID.to_string()),
            container: Some(TELECOM_FAMILY_CONTAINER.to_string()),
            object_count: Some(root_page.count.unwrap_or(root_page.fetched_count() as u64)),
            capacity: None,
            notes: vec![
                format!("family_id={family_id}"),
                "scope is mapped to container=family".to_string(),
                "family upload is not enabled yet; delete uses the web recycle-bin batch API"
                    .to_string(),
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
        if message.contains("InvalidSessionKey")
            || message.contains("cookieUserSession is null or invalid")
        {
            push_once(
                notes,
                "remediation=China Telecom web session expired; re-open cloud.189.cn, refresh the file list page, then paste a fresh Browser ID and Cookie Header into Admin Web",
            );
            push_once(
                notes,
                "remediation_hint=if the browser session was captured over IPv4, keep CCBG_TELECOM_IP_FAMILY=ipv4 on the gateway host",
            );
        } else if message.contains("missing China Telecom Browser ID") {
            push_once(
                notes,
                "remediation=fill China Telecom Browser ID in Admin Web -> Provider Credentials -> China Telecom",
            );
        } else if message.contains("missing China Telecom Cookie Header") {
            push_once(
                notes,
                "remediation=fill China Telecom Cookie Header in Admin Web -> Provider Credentials -> China Telecom",
            );
        } else if message.contains("requires China Telecom Access Token")
            || message.contains("family listing requires Access Token")
            || message.contains("family download requires Access Token")
        {
            push_once(
                notes,
                "remediation=capture the Telecom family session in Admin Web so Access Token and Family ID are stored together",
            );
        }
    }

    async fn find_child_folder(
        &self,
        scope: TelecomContainerScope,
        parent_folder_id: &str,
        child_name: &str,
    ) -> Result<TelecomFolderEntry, BlobError> {
        let mut page_num = 1;
        loop {
            let page = self
                .list_files_page_for_scope(scope, parent_folder_id, page_num)
                .await?;
            let fetched_count = page.fetched_count();

            if let Some(entry) = page
                .folder_list
                .into_iter()
                .find(|entry| entry.display_name() == Some(child_name))
            {
                return Ok(entry);
            }

            if fetched_count < self.page_size() {
                break;
            }
            page_num += 1;
        }

        Err(BlobError::NotFound(format!(
            "folder not found under {parent_folder_id}: {child_name}"
        )))
    }

    async fn find_child_folder_id(
        &self,
        scope: TelecomContainerScope,
        parent_folder_id: &str,
        child_name: &str,
    ) -> Result<Option<String>, BlobError> {
        let mut page_num = 1;
        loop {
            let page = self
                .list_files_page_for_scope(scope, parent_folder_id, page_num)
                .await?;
            let fetched_count = page.fetched_count();

            if let Some(entry) = page
                .folder_list
                .into_iter()
                .find(|entry| entry.display_name() == Some(child_name))
            {
                return entry
                    .id()
                    .map(str::to_string)
                    .ok_or_else(|| {
                        BlobError::Upstream(
                            "listFiles.action returned a folder without an id".to_string(),
                        )
                    })
                    .map(Some);
            }

            if fetched_count < self.page_size() {
                return Ok(None);
            }
            page_num += 1;
        }
    }

    async fn find_child_file(
        &self,
        scope: TelecomContainerScope,
        parent_folder_id: &str,
        child_name: &str,
    ) -> Result<TelecomFileEntry, BlobError> {
        let mut page_num = 1;
        loop {
            let page = self
                .list_files_page_for_scope(scope, parent_folder_id, page_num)
                .await?;
            let fetched_count = page.fetched_count();

            if let Some(entry) = page
                .file_list
                .into_iter()
                .find(|entry| entry.display_name() == Some(child_name))
            {
                return Ok(entry);
            }

            if fetched_count < self.page_size() {
                break;
            }
            page_num += 1;
        }

        Err(BlobError::NotFound(format!(
            "file not found under {parent_folder_id}: {child_name}"
        )))
    }

    async fn resolve_child_directory_path_if_exists_in_scope(
        &self,
        scope: TelecomContainerScope,
        parent_folder_id: &str,
        directory_path: &str,
    ) -> Result<Option<String>, BlobError> {
        let normalized = normalize_object_key(directory_path);
        if normalized.is_empty() {
            return Ok(Some(parent_folder_id.to_string()));
        }

        let mut current_id = parent_folder_id.to_string();
        for segment in normalized
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
        {
            let Some(child_id) = self
                .find_child_folder_id(scope, current_id.as_str(), segment)
                .await?
            else {
                return Ok(None);
            };
            current_id = child_id;
        }

        Ok(Some(current_id))
    }

    async fn resolve_scope_container_root_folder_id(
        &self,
        container: &str,
    ) -> Result<Option<String>, BlobError> {
        let scope = self.container_scope(container)?;
        let managed_root = self.managed_container_root(container)?;
        if managed_root.is_empty() {
            let root = match scope {
                TelecomContainerScope::Personal => self.root_folder_id(),
                TelecomContainerScope::Family => DEFAULT_FAMILY_ROOT_FOLDER_ID,
            };
            return Ok(Some(root.to_string()));
        }
        let root = match scope {
            TelecomContainerScope::Personal => self.root_folder_id(),
            TelecomContainerScope::Family => DEFAULT_FAMILY_ROOT_FOLDER_ID,
        };
        self.resolve_child_directory_path_if_exists_in_scope(scope, root, &managed_root)
            .await
    }

    async fn resolve_provider_file_entry(
        &self,
        scope: TelecomContainerScope,
        provider_key: &str,
    ) -> Result<(TelecomFileEntry, String), BlobError> {
        let normalized_key = normalize_object_key(provider_key);
        if normalized_key.is_empty() {
            return Err(BlobError::NotFound("object key is empty".to_string()));
        }

        let segments = normalized_key.split('/').collect::<Vec<_>>();
        let mut parent_folder_id = match scope {
            TelecomContainerScope::Personal => self.root_folder_id(),
            TelecomContainerScope::Family => DEFAULT_FAMILY_ROOT_FOLDER_ID,
        }
        .to_string();

        for segment in &segments[..segments.len().saturating_sub(1)] {
            let folder = self
                .find_child_folder(scope, &parent_folder_id, segment)
                .await?;
            parent_folder_id = folder
                .id()
                .ok_or_else(|| {
                    BlobError::Upstream(
                        "listFiles.action returned a folder without an id".to_string(),
                    )
                })?
                .to_string();
        }

        let file_name = segments
            .last()
            .expect("normalized object key should contain at least one segment");
        let file = self
            .find_child_file(scope, &parent_folder_id, file_name)
            .await?;
        Ok((file, normalized_key))
    }

    async fn get_stream(&self, url: &str, action: &str) -> Result<TimedObjectBody, BlobError> {
        let request_started_at = Instant::now();
        let request_timeout = self.timeout();
        let response = timeout(request_timeout, async {
            self.client
                .request(Method::GET, url)
                .header(USER_AGENT, self.effective_user_agent())
                .send()
                .await
        })
        .await
        .map_err(|_| {
            BlobError::Upstream(format!(
                "{action} request timed out after {}s waiting for response headers",
                request_timeout.as_secs()
            ))
        })?
        .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;
        let first_response_latency_ms = elapsed_millis(request_started_at);

        if response.status() == StatusCode::NOT_FOUND {
            return Err(BlobError::NotFound(action.to_string()));
        }

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        let action = action.to_string();
        let idle_timeout = self.timeout();
        Ok(TimedObjectBody {
            body: ObjectBody::from_stream(try_unfold(response, move |mut response| {
                let action = action.clone();
                let idle_timeout = idle_timeout;
                async move {
                    let chunk = timeout(idle_timeout, response.chunk())
                        .await
                        .map_err(|_| {
                            BlobError::Upstream(format!(
                                "{action} timed out while reading response body after {}s without progress",
                                idle_timeout.as_secs()
                            ))
                        })?
                        .map_err(|error| {
                        BlobError::Upstream(format!("{action} returned invalid bytes: {error}"))
                    })?;
                    Ok(chunk.map(|chunk| (chunk, response)))
                }
            })),
            first_response_latency_ms,
        })
    }

    fn create_upload_spool_file(&self) -> Result<NamedTempFile, BlobError> {
        let mut builder = TempFileBuilder::new();
        builder.prefix("ccbg-telecom-upload-").suffix(".spool");
        match self
            .config
            .body_spool_dir
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(path) => {
                fs::create_dir_all(path).map_err(|error| {
                    BlobError::Upstream(format!(
                        "failed to create China Telecom upload spool directory {path}: {error}"
                    ))
                })?;
                builder.tempfile_in(Path::new(path)).map_err(|error| {
                    BlobError::Upstream(format!(
                        "failed to create China Telecom upload spool file in {path}: {error}"
                    ))
                })
            }
            None => builder.tempfile().map_err(|error| {
                BlobError::Upstream(format!(
                    "failed to create China Telecom upload spool file in system temp directory: {error}"
                ))
            }),
        }
    }

    fn upload_control_base_url(&self) -> &str {
        let base = self.trimmed_base_url();
        if base.contains("cloud.189.cn") {
            DEFAULT_UPLOAD_CONTROL_BASE_URL
        } else {
            base
        }
    }

    fn upload_session_url(&self) -> String {
        format!(
            "{}/api/portal/v2/getUserBriefInfo.action",
            self.trimmed_base_url()
        )
    }

    fn generate_rsa_key_url(&self) -> String {
        format!(
            "{}/api/security/generateRsaKey.action",
            self.trimmed_base_url()
        )
    }

    fn create_folder_url(&self) -> String {
        format!(
            "{}/api/open/file/createFolder.action",
            self.trimmed_base_url()
        )
    }

    async fn send_get_value(
        &self,
        url: &str,
        referer: &str,
        query_params: &[(String, String)],
        action: &str,
    ) -> Result<Value, BlobError> {
        let mut query = query_params.to_vec();
        query.push(("noCache".to_string(), random_nocache_value()));
        let response = self
            .request(Method::GET, url, Some(referer))?
            .query(&query)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }
        response.json::<Value>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn send_form_value(
        &self,
        url: &str,
        referer: &str,
        form_fields: &[(String, String)],
        action: &str,
    ) -> Result<Value, BlobError> {
        let response = self
            .request(Method::POST, url, Some(referer))?
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .query(&[("noCache".to_string(), random_nocache_value())])
            .form(form_fields)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;
        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }
        response.json::<Value>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })
    }

    async fn fetch_upload_session_key(&self) -> Result<String, BlobError> {
        let value = self
            .send_get_value(
                self.upload_session_url().as_str(),
                self.main_referer().as_str(),
                &[],
                "getUserBriefInfo.action",
            )
            .await?;
        json_lookup_string(&value, &[&["sessionKey"], &["data", "sessionKey"]]).ok_or_else(|| {
            BlobError::Upstream("getUserBriefInfo.action returned no sessionKey".to_string())
        })
    }

    async fn fetch_upload_rsa_key(&self) -> Result<TelecomUploadRsaKey, BlobError> {
        let value = self
            .send_get_value(
                self.generate_rsa_key_url().as_str(),
                self.main_referer().as_str(),
                &[],
                "generateRsaKey.action",
            )
            .await?;
        let pub_key =
            json_lookup_string(&value, &[&["pubKey"], &["data", "pubKey"]]).ok_or_else(|| {
                BlobError::Upstream("generateRsaKey.action returned no pubKey".to_string())
            })?;
        let pk_id =
            json_lookup_string(&value, &[&["pkId"], &["data", "pkId"]]).ok_or_else(|| {
                BlobError::Upstream("generateRsaKey.action returned no pkId".to_string())
            })?;
        Ok(TelecomUploadRsaKey { pk_id, pub_key })
    }

    async fn fetch_upload_bootstrap(&self) -> Result<TelecomUploadBootstrap, BlobError> {
        Ok(TelecomUploadBootstrap {
            session_key: self.fetch_upload_session_key().await?,
            rsa_key: self.fetch_upload_rsa_key().await?,
        })
    }

    async fn prepare_upload_body(
        &self,
        body: ObjectBody,
        declared_size: Option<u64>,
        preferred_part_size_bytes: Option<u64>,
    ) -> Result<PreparedTelecomUpload, BlobError> {
        let spool_file = self.create_upload_spool_file()?;
        let mut spool_lease = self
            .config
            .body_spool_observer
            .as_ref()
            .map(|observer| observer.start_tracking());
        let async_file = spool_file.reopen().map_err(|error| {
            BlobError::Upstream(format!(
                "failed to reopen China Telecom upload spool file for writing: {error}"
            ))
        })?;
        let mut file = TokioFile::from_std(async_file);
        let mut stream = body.into_stream();
        let part_size_bytes = preferred_part_size_bytes
            .filter(|value| *value > 0)
            .unwrap_or_else(|| self.upload_part_size_bytes())
            .max(1);
        let mut total_len = 0u64;
        let mut file_hasher = Md5::new();
        let mut part_hasher = Md5::new();
        let mut current_part_len = 0u64;
        let mut part_md5_upper = Vec::new();
        let mut part_md5_base64 = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)
                .await
                .map_err(|error| BlobError::BodyStream(error.to_string()))?;
            total_len = total_len.saturating_add(chunk.len() as u64);
            file_hasher.update(&chunk);
            if let Some(lease) = spool_lease.as_mut() {
                lease.update_tracked_bytes(total_len);
            }

            let mut remaining = chunk.as_ref();
            while !remaining.is_empty() {
                let room =
                    (part_size_bytes - current_part_len).min(remaining.len() as u64) as usize;
                let (current, rest) = remaining.split_at(room);
                part_hasher.update(current);
                current_part_len += current.len() as u64;
                remaining = rest;

                if current_part_len == part_size_bytes {
                    let digest = std::mem::replace(&mut part_hasher, Md5::new()).finalize();
                    part_md5_upper.push(hex_upper(digest.as_slice()));
                    part_md5_base64.push(BASE64_STANDARD.encode(digest.as_slice()));
                    current_part_len = 0;
                }
            }
        }

        if let Some(expected) = declared_size {
            if expected != total_len {
                return Err(BlobError::BodyStream(format!(
                    "object body size mismatch: declared {expected} bytes, received {total_len}"
                )));
            }
        }

        if current_part_len > 0 || part_md5_upper.is_empty() {
            let digest = part_hasher.finalize();
            part_md5_upper.push(hex_upper(digest.as_slice()));
            part_md5_base64.push(BASE64_STANDARD.encode(digest.as_slice()));
        }

        file.flush()
            .await
            .map_err(|error| BlobError::BodyStream(error.to_string()))?;

        let file_md5_upper = hex_upper(file_hasher.finalize().as_slice());
        let slice_md5_upper = if part_md5_upper.len() <= 1 {
            file_md5_upper.clone()
        } else {
            hex_upper(Md5::digest(part_md5_upper.join("\n").as_bytes()).as_slice())
        };

        Ok(PreparedTelecomUpload {
            spool_file,
            size: total_len,
            file_md5_upper,
            slice_md5_upper,
            part_md5_upper,
            part_md5_base64,
            part_size_bytes,
            _spool_lease: spool_lease,
        })
    }

    async fn ensure_folder(
        &self,
        parent_folder_id: &str,
        folder_name: &str,
    ) -> Result<String, BlobError> {
        if let Some(existing) = self
            .find_child_folder_id(
                TelecomContainerScope::Personal,
                parent_folder_id,
                folder_name,
            )
            .await?
        {
            return Ok(existing);
        }

        let payload = self
            .send_form_value(
                self.create_folder_url().as_str(),
                self.folder_referer(parent_folder_id).as_str(),
                &[
                    ("parentFolderId".to_string(), parent_folder_id.to_string()),
                    ("folderName".to_string(), folder_name.to_string()),
                ],
                "createFolder.action",
            )
            .await?;

        if let Some(res_code) = json_lookup_string(&payload, &[&["res_code"], &["resCode"]]) {
            if res_code != "0" {
                if res_code == "FileAlreadyExists" {
                    return self
                        .find_child_folder_id(
                            TelecomContainerScope::Personal,
                            parent_folder_id,
                            folder_name,
                        )
                        .await?
                        .ok_or_else(|| {
                            BlobError::Upstream(format!(
                                "createFolder.action reported an existing folder but it could not be listed: {folder_name}"
                            ))
                        });
                }
                let message =
                    json_lookup_string(&payload, &[&["res_message"], &["message"], &["msg"]])
                        .unwrap_or_else(|| "unknown error".to_string());
                return Err(BlobError::Upstream(format!(
                    "createFolder.action returned res_code={res_code} ({message})"
                )));
            }
        }

        json_lookup_string(
            &payload,
            &[
                &["id"],
                &["folderId"],
                &["data", "id"],
                &["data", "folderId"],
            ],
        )
        .ok_or_else(|| {
            BlobError::Upstream(
                "createFolder.action returned no folder id for the created directory".to_string(),
            )
        })
    }

    async fn ensure_managed_parent_folder_id(
        &self,
        container: &str,
        key: &str,
    ) -> Result<(String, String), BlobError> {
        let provider_key = self.provider_object_key(container, key)?;
        let (parent_path, file_name) = match provider_key.rsplit_once('/') {
            Some((parent_path, file_name)) => (Some(parent_path), file_name.to_string()),
            None => return Ok((self.root_folder_id().to_string(), provider_key)),
        };

        let mut parent_id = self.root_folder_id().to_string();
        if let Some(parent_path) = parent_path {
            for segment in parent_path
                .split('/')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
            {
                parent_id = self.ensure_folder(parent_id.as_str(), segment).await?;
            }
        }

        Ok((parent_id, file_name))
    }

    async fn send_upload_control_get(
        &self,
        host: &str,
        uri: &str,
        data_pairs: &[(String, String)],
        bootstrap: &TelecomUploadBootstrap,
        action: &str,
    ) -> Result<Value, BlobError> {
        let request_id = random_request_id();
        let secret = random_upload_secret();
        let request_date = current_unix_ms().to_string();
        let params_plain = build_upload_params_plaintext(data_pairs);
        let encrypted_params = encrypt_upload_params_hex(secret.as_str(), params_plain.as_str())?;
        let signature = telecom_upload_signature(
            bootstrap.session_key.as_str(),
            "GET",
            uri,
            request_date.as_str(),
            encrypted_params.as_str(),
            secret.as_str(),
        )?;
        let encryption_text =
            rsa_encrypt_upload_secret(bootstrap.rsa_key.pub_key.as_str(), secret.as_str())?;
        let url = format!(
            "{}{}?params={}",
            host.trim_end_matches('/'),
            uri,
            encrypted_params
        );

        let response = self
            .client
            .request(Method::GET, &url)
            .header(USER_AGENT, self.effective_user_agent())
            .header(ACCEPT, "application/json;charset=UTF-8")
            .header("SessionKey", bootstrap.session_key.as_str())
            .header("Signature", signature)
            .header("X-Request-Date", request_date)
            .header("X-Request-ID", request_id)
            .header("EncryptionText", encryption_text)
            .header("PkId", bootstrap.rsa_key.pk_id.as_str())
            .timeout(self.timeout())
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

        if !response.status().is_success() {
            return Err(response_to_error(response, action).await);
        }

        let value = response.json::<Value>().await.map_err(|error| {
            BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
        })?;
        ensure_upload_success(&value, action)?;
        Ok(value)
    }

    async fn init_multi_upload(
        &self,
        bootstrap: &TelecomUploadBootstrap,
        parent_folder_id: &str,
        file_name: &str,
        upload: &PreparedTelecomUpload,
    ) -> Result<TelecomUploadPlan, BlobError> {
        let mut fields = vec![
            ("parentFolderId".to_string(), parent_folder_id.to_string()),
            ("fileName".to_string(), encode_uri_component(file_name)),
            ("fileSize".to_string(), upload.size.to_string()),
            ("sliceSize".to_string(), upload.part_size_bytes.to_string()),
        ];
        if upload.part_md5_upper.len() == 1 {
            fields.push(("fileMd5".to_string(), upload.file_md5_upper.clone()));
            fields.push(("sliceMd5".to_string(), upload.slice_md5_upper.clone()));
        } else {
            fields.push(("lazyCheck".to_string(), "1".to_string()));
        }

        let value = self
            .send_upload_control_get(
                self.upload_control_base_url(),
                "/person/initMultiUpload",
                &fields,
                bootstrap,
                "initMultiUpload",
            )
            .await?;
        let upload_host = json_lookup_string(&value, &[&["data", "uploadHost"]])
            .unwrap_or_else(|| self.upload_control_base_url().to_string());
        let upload_file_id =
            json_lookup_string(&value, &[&["data", "uploadFileId"]]).ok_or_else(|| {
                BlobError::Upstream("initMultiUpload returned no uploadFileId".to_string())
            })?;
        let file_data_exists =
            json_lookup_boolish(&value, &[&["data", "fileDataExists"]]).unwrap_or(false);

        Ok(TelecomUploadPlan {
            upload_host,
            upload_file_id,
            file_data_exists,
        })
    }

    async fn get_multi_upload_url(
        &self,
        bootstrap: &TelecomUploadBootstrap,
        upload_host: &str,
        upload_file_id: &str,
        part_number: usize,
        part_md5_base64: &str,
    ) -> Result<(String, String), BlobError> {
        let value = self
            .send_upload_control_get(
                upload_host,
                "/person/getMultiUploadUrls",
                &[
                    ("uploadFileId".to_string(), upload_file_id.to_string()),
                    (
                        "partInfo".to_string(),
                        format!("{part_number}-{part_md5_base64}"),
                    ),
                ],
                bootstrap,
                "getMultiUploadUrls",
            )
            .await?;
        let key = format!("partNumber_{part_number}");
        let request_url =
            json_lookup_string(&value, &[&["uploadUrls", key.as_str(), "requestURL"]]).ok_or_else(
                || {
                    BlobError::Upstream(format!(
                        "getMultiUploadUrls returned no requestURL for part {part_number}"
                    ))
                },
            )?;
        let request_header =
            json_lookup_string(&value, &[&["uploadUrls", key.as_str(), "requestHeader"]])
                .ok_or_else(|| {
                    BlobError::Upstream(format!(
                        "getMultiUploadUrls returned no requestHeader for part {part_number}"
                    ))
                })?;
        Ok((request_url, request_header))
    }

    async fn upload_part(
        &self,
        upload_started_at: Instant,
        first_upload_progress_ms: Arc<AtomicU64>,
        upload: &PreparedTelecomUpload,
        part_number: usize,
        request_url: &str,
        request_header: &str,
    ) -> Result<(), BlobError> {
        let offset = (part_number.saturating_sub(1) as u64).saturating_mul(upload.part_size_bytes);
        let part_len = (upload.size.saturating_sub(offset)).min(upload.part_size_bytes);
        let file = upload.spool_file.reopen().map_err(|error| {
            BlobError::Upstream(format!(
                "failed to reopen China Telecom upload spool file for reading: {error}"
            ))
        })?;
        let mut file = TokioFile::from_std(file);
        file.seek(SeekFrom::Start(offset)).await.map_err(|error| {
            BlobError::Upstream(format!("failed to seek upload spool file: {error}"))
        })?;

        let mut request =
            self.client
                .request(Method::PUT, request_url)
                .timeout(Duration::from_secs(
                    self.config.request_timeout_secs.max(180),
                ));
        for (name, value) in parse_upload_request_headers(request_header)? {
            request = request.header(name, value);
        }
        let body = if part_len == 0 {
            reqwest::Body::from(Vec::<u8>::new())
        } else {
            let progress_observer = StreamFirstProgressObserver::new({
                let first_upload_progress_ms = Arc::clone(&first_upload_progress_ms);
                move || {
                    first_upload_progress_ms
                        .store(elapsed_millis(upload_started_at).max(1), Ordering::SeqCst);
                }
            });
            reqwest::Body::wrap_stream(try_unfold(
                (file, part_len, progress_observer),
                move |(mut file, remaining, progress_observer)| async move {
                    if remaining == 0 {
                        return Ok(None);
                    }
                    let next_len = remaining.min(1024 * 1024) as usize;
                    let mut chunk = vec![0u8; next_len];
                    let read = file.read(&mut chunk).await?;
                    if read == 0 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            format!(
                                "China Telecom upload spool file ended early while streaming part {part_number}"
                            ),
                        ));
                    }
                    chunk.truncate(read);
                    if !chunk.is_empty() {
                        progress_observer.notify();
                    }
                    Ok(Some((
                        bytes::Bytes::from(chunk),
                        (
                            file,
                            remaining.saturating_sub(read as u64),
                            progress_observer,
                        ),
                    )))
                },
            ))
        };
        let response = request.body(body).send().await.map_err(|error| {
            BlobError::Upstream(format!("upload part {part_number} failed: {error}"))
        })?;
        if !response.status().is_success() {
            return Err(response_to_error(response, &format!("upload part {part_number}")).await);
        }
        Ok(())
    }

    async fn upload_multi_parts(
        &self,
        bootstrap: &TelecomUploadBootstrap,
        plan: &TelecomUploadPlan,
        upload: &PreparedTelecomUpload,
        upload_started_at: Instant,
    ) -> Result<Option<u64>, BlobError> {
        let first_upload_progress_ms = Arc::new(AtomicU64::new(0));
        for (index, part_md5_base64) in upload.part_md5_base64.iter().enumerate() {
            let part_number = index + 1;
            let (request_url, request_header) = self
                .get_multi_upload_url(
                    bootstrap,
                    plan.upload_host.as_str(),
                    plan.upload_file_id.as_str(),
                    part_number,
                    part_md5_base64.as_str(),
                )
                .await?;
            self.upload_part(
                upload_started_at,
                Arc::clone(&first_upload_progress_ms),
                upload,
                part_number,
                request_url.as_str(),
                request_header.as_str(),
            )
            .await?;
        }
        Ok(match first_upload_progress_ms.load(Ordering::SeqCst) {
            0 => None,
            value => Some(value),
        })
    }

    async fn check_trans_second(
        &self,
        bootstrap: &TelecomUploadBootstrap,
        plan: &mut TelecomUploadPlan,
        upload: &PreparedTelecomUpload,
    ) -> Result<(), BlobError> {
        let value = self
            .send_upload_control_get(
                plan.upload_host.as_str(),
                "/person/checkTransSecond",
                &[
                    ("fileMd5".to_string(), upload.file_md5_upper.clone()),
                    ("sliceMd5".to_string(), upload.slice_md5_upper.clone()),
                    ("uploadFileId".to_string(), plan.upload_file_id.clone()),
                ],
                bootstrap,
                "checkTransSecond",
            )
            .await?;
        if let Some(upload_file_id) = json_lookup_string(&value, &[&["data", "uploadFileId"]]) {
            plan.upload_file_id = upload_file_id;
        }
        if let Some(file_data_exists) = json_lookup_boolish(&value, &[&["data", "fileDataExists"]])
        {
            plan.file_data_exists = file_data_exists;
        }
        Ok(())
    }

    async fn commit_multi_upload(
        &self,
        bootstrap: &TelecomUploadBootstrap,
        plan: &TelecomUploadPlan,
        upload: &PreparedTelecomUpload,
    ) -> Result<String, BlobError> {
        let value = self
            .send_upload_control_get(
                plan.upload_host.as_str(),
                "/person/commitMultiUploadFile",
                &[
                    ("uploadFileId".to_string(), plan.upload_file_id.clone()),
                    (
                        "lazyCheck".to_string(),
                        if upload.part_md5_upper.len() > 1 {
                            "1"
                        } else {
                            "0"
                        }
                        .to_string(),
                    ),
                    ("fileMd5".to_string(), upload.file_md5_upper.clone()),
                    ("sliceMd5".to_string(), upload.slice_md5_upper.clone()),
                    ("opertype".to_string(), "3".to_string()),
                ],
                bootstrap,
                "commitMultiUploadFile",
            )
            .await?;
        Ok(json_lookup_string(
            &value,
            &[&["file", "fileMd5"], &["file", "userFileId"], &["fileMd5"]],
        )
        .unwrap_or_else(|| upload.file_md5_upper.clone()))
    }

    fn container_scope(&self, container: &str) -> Result<TelecomContainerScope, BlobError> {
        match normalize_object_key(container).as_str() {
            TELECOM_ROOT_CONTAINER => Ok(TelecomContainerScope::Personal),
            TELECOM_FAMILY_CONTAINER if self.configured_family_id().is_some() => {
                Ok(TelecomContainerScope::Family)
            }
            TELECOM_FAMILY_CONTAINER => Err(BlobError::Configuration(
                "China Telecom family container requires Family ID; capture a family session in Admin Web or set CCBG_TELECOM_FAMILY_ID"
                    .to_string(),
            )),
            _ => Err(BlobError::NotFound(format!(
                "container not found: {container}"
            ))),
        }
    }

    fn validate_container(&self, container: &str) -> Result<(), BlobError> {
        self.container_scope(container).map(|_| ())
    }

    fn batch_task_infos_for_file(&self, entry: &TelecomFileEntry) -> Result<String, BlobError> {
        let file_id = entry.id().ok_or_else(|| {
            BlobError::Upstream(
                "China Telecom delete cannot run because file id is missing".to_string(),
            )
        })?;
        let file_name = entry.display_name().ok_or_else(|| {
            BlobError::Upstream(
                "China Telecom delete cannot run because file name is missing".to_string(),
            )
        })?;
        serde_json::to_string(&[serde_json::json!({
            "fileId": file_id,
            "fileName": file_name,
            "isFolder": 0,
            "srcParentId": entry.parent_id().unwrap_or_default(),
        })])
        .map_err(|error| BlobError::Upstream(format!("failed to encode delete taskInfos: {error}")))
    }

    async fn create_personal_delete_task(
        &self,
        entry: &TelecomFileEntry,
    ) -> Result<String, BlobError> {
        let fields = vec![
            ("type".to_string(), "DELETE".to_string()),
            (
                "taskInfos".to_string(),
                self.batch_task_infos_for_file(entry)?,
            ),
            ("targetFolderId".to_string(), String::new()),
        ];
        let payload = self
            .send_form_value(
                self.personal_batch_create_task_url().as_str(),
                self.main_referer().as_str(),
                &fields,
                "createBatchTask.action",
            )
            .await?;
        ensure_json_success(&payload, "createBatchTask.action")?;
        json_lookup_string(&payload, &[&["taskId"], &["data", "taskId"]]).ok_or_else(|| {
            BlobError::Upstream("createBatchTask.action returned no taskId".to_string())
        })
    }

    async fn create_family_delete_task(
        &self,
        entry: &TelecomFileEntry,
    ) -> Result<String, BlobError> {
        let family_id = self.family_id()?;
        let fields = vec![
            ("type".to_string(), "DELETE".to_string()),
            (
                "taskInfos".to_string(),
                self.batch_task_infos_for_file(entry)?,
            ),
            ("targetFolderId".to_string(), String::new()),
            ("familyId".to_string(), family_id.to_string()),
        ];
        let payload = self
            .send_signed_form_value(
                self.family_batch_create_task_url().as_str(),
                self.family_referer(DEFAULT_FAMILY_ROOT_FOLDER_ID).as_str(),
                &fields,
                "family createBatchTask.action",
            )
            .await?;
        ensure_json_success(&payload, "family createBatchTask.action")?;
        json_lookup_string(&payload, &[&["taskId"], &["data", "taskId"]]).ok_or_else(|| {
            BlobError::Upstream("family createBatchTask.action returned no taskId".to_string())
        })
    }

    async fn check_personal_batch_task(
        &self,
        task_id: &str,
        task_type: &str,
    ) -> Result<Value, BlobError> {
        self.send_get_value(
            self.personal_batch_check_task_url().as_str(),
            self.main_referer().as_str(),
            &[
                ("taskId".to_string(), task_id.to_string()),
                ("type".to_string(), task_type.to_string()),
            ],
            "checkBatchTask.action",
        )
        .await
    }

    async fn check_family_batch_task(
        &self,
        task_id: &str,
        task_type: &str,
    ) -> Result<Value, BlobError> {
        self.send_signed_form_value(
            self.family_batch_check_task_url().as_str(),
            self.family_referer(DEFAULT_FAMILY_ROOT_FOLDER_ID).as_str(),
            &[
                ("taskId".to_string(), task_id.to_string()),
                ("type".to_string(), task_type.to_string()),
            ],
            "family checkBatchTask.action",
        )
        .await
    }

    async fn check_batch_task_for_scope(
        &self,
        scope: TelecomContainerScope,
        task_id: &str,
        task_type: &str,
    ) -> Result<Value, BlobError> {
        match scope {
            TelecomContainerScope::Personal => {
                self.check_personal_batch_task(task_id, task_type).await
            }
            TelecomContainerScope::Family => self.check_family_batch_task(task_id, task_type).await,
        }
    }

    async fn wait_for_batch_task(
        &self,
        scope: TelecomContainerScope,
        task_id: &str,
        task_type: &str,
    ) -> Result<(), BlobError> {
        let started_at = Instant::now();
        loop {
            let payload = self
                .check_batch_task_for_scope(scope, task_id, task_type)
                .await?;
            ensure_json_success(&payload, "checkBatchTask.action")?;

            let task_status =
                json_lookup_i64(&payload, &[&["taskStatus"], &["data", "taskStatus"]]);
            let failed_count =
                json_lookup_i64(&payload, &[&["failedCount"], &["data", "failedCount"]])
                    .unwrap_or(0);
            if failed_count > 0 {
                return Err(BlobError::Upstream(format!(
                    "checkBatchTask.action reported failedCount={failed_count} for taskId={task_id}"
                )));
            }

            match task_status {
                Some(4) => return Ok(()),
                Some(status @ (-1 | 5)) => {
                    return Err(BlobError::Upstream(format!(
                        "checkBatchTask.action reported terminal taskStatus={} for taskId={task_id}",
                        status
                    )));
                }
                _ => {}
            }

            if started_at.elapsed() >= self.timeout() {
                return Err(BlobError::Upstream(format!(
                    "checkBatchTask.action did not finish before timeout for taskId={task_id}"
                )));
            }
            sleep(Duration::from_millis(TELECOM_BATCH_POLL_INTERVAL_MS)).await;
        }
    }

    async fn delete_file_for_scope(
        &self,
        scope: TelecomContainerScope,
        entry: &TelecomFileEntry,
    ) -> Result<(), BlobError> {
        let task_id = match scope {
            TelecomContainerScope::Personal => self.create_personal_delete_task(entry).await?,
            TelecomContainerScope::Family => self.create_family_delete_task(entry).await?,
        };
        self.wait_for_batch_task(scope, task_id.as_str(), "DELETE")
            .await
    }

    async fn list_objects_in_root(
        &self,
        container: &str,
        request: &ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        if matches!(request.limit, Some(0)) {
            return Ok(Vec::new());
        }

        let scope = self.container_scope(container)?;
        let normalized_prefix = request.prefix.as_deref().map(normalize_object_key);
        let Some(start_folder_id) = self
            .resolve_scope_container_root_folder_id(container)
            .await?
        else {
            return Ok(Vec::new());
        };
        let mut objects = BTreeMap::new();
        let mut stack = vec![(start_folder_id, String::new())];

        while let Some((folder_id, folder_prefix)) = stack.pop() {
            let mut page_num = 1;
            loop {
                let page = self
                    .list_files_page_for_scope(scope, &folder_id, page_num)
                    .await?;
                let fetched_count = page.fetched_count();

                for folder in page.folder_list {
                    let child_name = folder.display_name().ok_or_else(|| {
                        BlobError::Upstream(
                            "listFiles.action returned a folder without a name".to_string(),
                        )
                    })?;
                    let child_id = folder.id().ok_or_else(|| {
                        BlobError::Upstream(
                            "listFiles.action returned a folder without an id".to_string(),
                        )
                    })?;
                    let child_prefix = join_relative_key(&folder_prefix, child_name);

                    if normalized_prefix
                        .as_deref()
                        .is_none_or(|prefix| directory_may_contain_prefix(&child_prefix, prefix))
                    {
                        stack.push((child_id.to_string(), child_prefix));
                    }
                }

                for file in page.file_list {
                    let object_key = file.object_key(&folder_prefix)?;
                    if normalized_prefix
                        .as_deref()
                        .is_none_or(|prefix| object_key.starts_with(prefix))
                    {
                        objects.insert(object_key.clone(), file.to_object_info(object_key));
                        if let Some(limit) = request.limit {
                            trim_objects_to_limit(&mut objects, limit);
                        }
                    }
                }

                if fetched_count < self.page_size() {
                    break;
                }
                page_num += 1;
            }
        }

        Ok(objects.into_values().collect())
    }
}

#[async_trait]
impl BlobBackend for TelecomBlobAdapter {
    fn name(&self) -> &'static str {
        "telecom-cloud-drive"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            read: true,
            write: true,
            delete: true,
            multipart_upload: false,
            streaming_get: true,
            streaming_put: true,
            max_single_upload_bytes: self.config.max_single_upload_bytes,
            max_single_download_bytes: self.config.max_single_download_bytes,
            upload_part_size_bytes: Some(self.upload_part_size_bytes()),
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
        let managed_root = self
            .normalized_root_prefix()
            .unwrap_or_else(|| "<provider-root>".to_string());
        let mut notes = vec![
            format!("base_url={}", self.config.base_url),
            format!(
                "outbound_ip_family={}",
                self.config.outbound_ip_family.as_str()
            ),
            format!("root_folder_id={}", self.root_folder_id()),
            format!("page_size={}", self.page_size()),
            format!("sign_type={}", self.sign_type()),
            format!("download_token_present={}", self.optional_token().is_some()),
            format!("managed_root={managed_root}"),
            format!("upload_part_size_bytes={}", self.upload_part_size_bytes()),
        ];
        let mut scopes = Vec::new();

        let status = match self.list_files_page(self.root_folder_id(), 1).await {
            Ok(page) => {
                notes.push(format!(
                    "root_entry_count={}",
                    page.count.unwrap_or(page.fetched_count() as u64)
                ));

                match self.user_info().await {
                    Ok(user_info) => {
                        scopes.push(self.personal_scope_health(
                            &page,
                            Some(&user_info),
                            &mut notes,
                        ));
                    }
                    Err(error) => {
                        notes.push(format!("personal_capacity_probe_failed={error}"));
                        self.push_remediation_notes(&mut notes, &error);
                        scopes.push(self.personal_scope_health(&page, None, &mut notes));
                    }
                }

                if let Some(family_id) = self.configured_family_id().map(str::to_string) {
                    match self
                        .family_list_files_page(DEFAULT_FAMILY_ROOT_FOLDER_ID, 1)
                        .await
                    {
                        Ok(family_page) => {
                            notes.push(format!(
                                "family_root_entry_count={}",
                                family_page
                                    .count
                                    .unwrap_or(family_page.fetched_count() as u64)
                            ));
                            scopes.push(self.family_scope_health(family_id.as_str(), &family_page));
                            HealthStatus::Healthy
                        }
                        Err(error) => {
                            notes.push(format!("family_scope_probe_failed={error}"));
                            self.push_remediation_notes(&mut notes, &error);
                            HealthStatus::Healthy
                        }
                    }
                } else {
                    notes.push("family_scope=not_configured".to_string());
                    HealthStatus::Healthy
                }
            }
            Err(error) => {
                notes.push(error.to_string());
                self.push_remediation_notes(&mut notes, &error);
                HealthStatus::Unavailable
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
        let _ = self.list_files_page(self.root_folder_id(), 1).await?;
        let mut containers = vec![ContainerInfo {
            name: TELECOM_ROOT_CONTAINER.to_string(),
            object_count: None,
        }];
        if self.configured_family_id().is_some() {
            if self
                .family_list_files_page(DEFAULT_FAMILY_ROOT_FOLDER_ID, 1)
                .await
                .is_ok()
            {
                containers.push(ContainerInfo {
                    name: TELECOM_FAMILY_CONTAINER.to_string(),
                    object_count: None,
                });
            }
        }
        Ok(containers)
    }

    async fn list_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        let Some(container) = request.container.as_deref() else {
            return Ok(Vec::new());
        };
        self.validate_container(container)?;
        self.list_objects_in_root(container, &request).await
    }

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        let scope = self.container_scope(container)?;
        let provider_key = self.provider_object_key(container, key)?;
        let (entry, _) = self
            .resolve_provider_file_entry(scope, &provider_key)
            .await?;
        Ok(entry.to_object_info(self.user_visible_object_key(container, &provider_key)?))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        let scope = self.container_scope(container)?;
        let provider_key = self.provider_object_key(container, key)?;
        let (entry, _) = self
            .resolve_provider_file_entry(scope, &provider_key)
            .await?;
        let download_url = entry
            .download_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| normalize_remote_url(self.trimmed_base_url(), value))
            .unwrap_or_else(String::new);
        let download_url = if download_url.is_empty() {
            let file_id = entry.id().ok_or_else(|| {
                BlobError::Upstream(
                    "getFileDownloadUrl.action cannot run because the file id is missing"
                        .to_string(),
                )
            })?;
            self.download_url_for_scope(scope, file_id).await?
        } else {
            download_url
        };
        let visible_key = self.user_visible_object_key(container, &provider_key)?;
        let downloaded = self
            .get_stream(&download_url, "telecom object download")
            .await?;
        Ok(ObjectPayload {
            info: entry.to_object_info(visible_key),
            body: downloaded.body,
            first_response_latency_ms: Some(downloaded.first_response_latency_ms),
        })
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        let scope = self.container_scope(&request.container)?;
        if scope == TelecomContainerScope::Family {
            return Err(BlobError::NotImplemented(
                "China Telecom family upload is not enabled yet; use container=root for writes"
                    .to_string(),
            ));
        }
        let (parent_folder_id, file_name) = self
            .ensure_managed_parent_folder_id(&request.container, &request.key)
            .await?;
        let upload = self
            .prepare_upload_body(
                request.body,
                request.size,
                request.preferred_upload_part_size_bytes,
            )
            .await?;
        let bootstrap = self.fetch_upload_bootstrap().await?;
        let first_response_started_at = Instant::now();
        let mut plan = self
            .init_multi_upload(
                &bootstrap,
                parent_folder_id.as_str(),
                file_name.as_str(),
                &upload,
            )
            .await?;
        let create_latency_ms = elapsed_millis(first_response_started_at);
        if !plan.file_data_exists {
            let upload_started_at = Instant::now();
            let first_response_latency_ms = self
                .upload_multi_parts(&bootstrap, &plan, &upload, upload_started_at)
                .await?
                .or(Some(elapsed_millis(upload_started_at).max(1)));
            self.check_trans_second(&bootstrap, &mut plan, &upload)
                .await?;
            let etag = self.commit_multi_upload(&bootstrap, &plan, &upload).await?;
            return Ok(PutObjectResult {
                etag: Some(etag),
                first_response_latency_ms,
            });
        }
        let etag = self.commit_multi_upload(&bootstrap, &plan, &upload).await?;
        Ok(PutObjectResult {
            etag: Some(etag),
            first_response_latency_ms: Some(create_latency_ms),
        })
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        let scope = self.container_scope(container)?;
        let object_key = self.provider_object_key(container, key)?;
        let (entry, _) = self.resolve_provider_file_entry(scope, &object_key).await?;
        self.delete_file_for_scope(scope, &entry).await
    }

    async fn rename_object(&self, request: RenameObjectRequest) -> Result<(), BlobError> {
        let source = self.provider_object_key(&request.container, &request.key)?;
        let destination = self.provider_object_key(&request.container, &request.new_key)?;
        Err(BlobError::NotImplemented(format!(
            "China Telecom native rename is not completed yet; source={source} destination={destination}"
        )))
    }

    async fn copy_object(&self, request: CopyObjectRequest) -> Result<(), BlobError> {
        let source = self.provider_object_key(&request.source_container, &request.source_key)?;
        let destination =
            self.provider_object_key(&request.destination_container, &request.destination_key)?;
        Err(BlobError::NotImplemented(format!(
            "China Telecom native copy is not completed yet; source={source} destination={destination}"
        )))
    }

    async fn move_object(&self, request: MoveObjectRequest) -> Result<(), BlobError> {
        let source = self.provider_object_key(&request.source_container, &request.source_key)?;
        let destination =
            self.provider_object_key(&request.destination_container, &request.destination_key)?;
        Err(BlobError::NotImplemented(format!(
            "China Telecom native move is not completed yet; source={source} destination={destination}"
        )))
    }
}

fn ensure_success_code(
    code: Option<&str>,
    message: Option<&str>,
    action: &str,
) -> Result<(), BlobError> {
    match code.map(str::trim).filter(|value| !value.is_empty()) {
        Some("0") => Ok(()),
        Some(code) => Err(BlobError::Upstream(format!(
            "{action} returned res_code={code}{}",
            message
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!(" ({value})"))
                .unwrap_or_default()
        ))),
        None => Err(BlobError::Upstream(format!(
            "{action} returned no res_code field"
        ))),
    }
}

fn normalize_object_key(key: &str) -> String {
    key.split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn join_relative_key(prefix: &str, child_name: &str) -> String {
    let normalized_prefix = prefix.trim_matches('/');
    if normalized_prefix.is_empty() {
        child_name.trim_matches('/').to_string()
    } else {
        format!("{normalized_prefix}/{}", child_name.trim_matches('/'))
    }
}

fn directory_may_contain_prefix(directory_key: &str, prefix: &str) -> bool {
    if directory_key.is_empty() || prefix.is_empty() {
        return true;
    }

    let directory_prefix = format!("{directory_key}/");
    prefix.starts_with(&directory_prefix) || directory_key.starts_with(prefix)
}

fn trim_objects_to_limit(objects: &mut BTreeMap<String, ObjectInfo>, limit: usize) {
    while objects.len() > limit {
        let Some(last_key) = objects.keys().next_back().cloned() else {
            break;
        };
        objects.remove(&last_key);
    }
}

fn normalize_remote_url(base_url: &str, raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else if trimmed.starts_with('/') {
        format!("{base_url}{trimmed}")
    } else {
        format!("{base_url}/{trimmed}")
    }
}

fn normalize_timestamp(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() == 19 && trimmed.as_bytes().get(10) == Some(&b' ') {
        return format!("{}T{}.000Z", &trimmed[..10], &trimmed[11..]);
    }

    if trimmed.len() == 14 && trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return format!(
            "{}-{}-{}T{}:{}:{}.000Z",
            &trimmed[0..4],
            &trimmed[4..6],
            &trimmed[6..8],
            &trimmed[8..10],
            &trimmed[10..12],
            &trimmed[12..14]
        );
    }

    trimmed.to_string()
}

fn guess_content_type(name: &str) -> Option<&'static str> {
    let suffix = name.rsplit('.').next()?;
    if suffix.eq_ignore_ascii_case(name) {
        None
    } else {
        guess_content_type_from_suffix(suffix)
    }
}

fn guess_content_type_from_suffix(suffix: &str) -> Option<&'static str> {
    match suffix.trim().to_ascii_lowercase().as_str() {
        "txt" | "log" | "md" => Some("text/plain"),
        "json" => Some("application/json"),
        "pdf" => Some("application/pdf"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "csv" => Some("text/csv"),
        _ => None,
    }
}

fn random_nocache_value() -> String {
    format!("0.{}", current_unix_ms())
}

fn current_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis()
}

fn telecom_signature(params: &[(String, String)]) -> String {
    let mut pairs = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    pairs.sort();

    let mut hasher = Md5::new();
    hasher.update(pairs.join("&"));
    format!("{:x}", hasher.finalize())
}

fn hex_upper(bytes: &[u8]) -> String {
    hex::encode_upper(bytes)
}

fn encode_uri_component(value: &str) -> String {
    utf8_percent_encode(value, URI_COMPONENT_ENCODE_SET).to_string()
}

fn build_upload_params_plaintext(data_pairs: &[(String, String)]) -> String {
    data_pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn encrypt_upload_params_hex(secret: &str, plaintext: &str) -> Result<String, BlobError> {
    if secret.len() < 16 {
        return Err(BlobError::Configuration(
            "upload encryption secret is shorter than 16 characters".to_string(),
        ));
    }
    let key_bytes: [u8; 16] = secret.as_bytes()[..16]
        .try_into()
        .map_err(|_| BlobError::Configuration("invalid upload AES key length".to_string()))?;
    let cipher = Encryptor::<Aes128>::new(&key_bytes.into());
    let mut buffer = plaintext.as_bytes().to_vec();
    let message_len = buffer.len();
    let next_block_len = ((message_len / 16) + 1) * 16;
    buffer.resize(next_block_len, 0);
    let ciphertext = cipher
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, message_len)
        .map_err(|error| {
            BlobError::Configuration(format!(
                "failed to AES-encrypt Telecom upload params: {error}"
            ))
        })?;
    Ok(hex::encode(ciphertext))
}

fn telecom_upload_signature(
    session_key: &str,
    operate: &str,
    request_uri: &str,
    request_date: &str,
    encrypted_params: &str,
    secret: &str,
) -> Result<String, BlobError> {
    let payload = [
        format!("SessionKey={session_key}"),
        format!("Operate={operate}"),
        format!("RequestURI={request_uri}"),
        format!("Date={request_date}"),
        format!("params={encrypted_params}"),
    ]
    .join("&");
    let mut mac = <HmacSha1 as Mac>::new_from_slice(secret.as_bytes()).map_err(|error| {
        BlobError::Configuration(format!("failed to initialize Telecom upload HMAC: {error}"))
    })?;
    mac.update(payload.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

fn rsa_encrypt_upload_secret(public_key_pem: &str, secret: &str) -> Result<String, BlobError> {
    let public_key = parse_upload_public_key(public_key_pem)?;
    let ciphertext = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, secret.as_bytes())
        .map_err(|error| {
            BlobError::Upstream(format!("failed to encrypt Telecom upload secret: {error}"))
        })?;
    Ok(BASE64_STANDARD.encode(ciphertext))
}

fn parse_upload_public_key(public_key: &str) -> Result<RsaPublicKey, BlobError> {
    let normalized = normalize_upload_public_key(public_key);
    match RsaPublicKey::from_public_key_pem(normalized.as_str()) {
        Ok(public_key) => return Ok(public_key),
        Err(pem_error) => {
            let base64_body = upload_public_key_base64_body(normalized.as_str());
            if !base64_body.is_empty() {
                let der = BASE64_STANDARD
                    .decode(base64_body.as_bytes())
                    .map_err(|error| {
                        BlobError::Configuration(format!(
                            "invalid Telecom upload RSA public key base64: {error}"
                        ))
                    })?;
                if let Ok(public_key) = RsaPublicKey::from_public_key_der(der.as_slice()) {
                    return Ok(public_key);
                }
                if let Ok(public_key) = RsaPublicKey::from_pkcs1_der(der.as_slice()) {
                    return Ok(public_key);
                }
            }
            Err(BlobError::Configuration(format!(
                "invalid Telecom upload RSA public key: {pem_error}"
            )))
        }
    }
}

fn normalize_upload_public_key(public_key: &str) -> String {
    public_key
        .chars()
        .filter(|value| *value != '\0')
        .collect::<String>()
        .trim()
        .to_string()
}

fn upload_public_key_base64_body(public_key: &str) -> String {
    public_key
        .lines()
        .filter(|line| !line.trim_start().starts_with("-----"))
        .flat_map(str::chars)
        .filter(|value| !value.is_whitespace() && *value != '\0')
        .collect()
}

fn random_request_id() -> String {
    let mut rng = rand::thread_rng();
    "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"
        .chars()
        .map(|ch| match ch {
            'x' => format!("{:x}", rng.gen_range(0..16)),
            'y' => format!("{:x}", rng.gen_range(8..12)),
            other => other.to_string(),
        })
        .collect::<String>()
}

fn random_upload_secret() -> String {
    let mut rng = rand::thread_rng();
    let full = "xxxxxxxxxxxx4xxxyxxxxxxxxxxxxxxx"
        .chars()
        .map(|ch| match ch {
            'x' => format!("{:x}", rng.gen_range(0..16)),
            'y' => format!("{:x}", rng.gen_range(8..12)),
            other => other.to_string(),
        })
        .collect::<String>();
    let len = rng.gen_range(16..32);
    full[..len].to_string()
}

fn parse_upload_request_headers(
    raw_headers: &str,
) -> Result<Vec<(HeaderName, HeaderValue)>, BlobError> {
    let mut headers = Vec::new();
    for pair in raw_headers
        .split('&')
        .filter(|value| !value.trim().is_empty())
    {
        let Some((name, value)) = pair.split_once('=') else {
            return Err(BlobError::Upstream(format!(
                "invalid Telecom upload requestHeader segment: {pair}"
            )));
        };
        headers.push((
            HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                BlobError::Upstream(format!(
                    "invalid Telecom upload requestHeader name {name}: {error}"
                ))
            })?,
            HeaderValue::from_str(value).map_err(|error| {
                BlobError::Upstream(format!(
                    "invalid Telecom upload requestHeader value for {name}: {error}"
                ))
            })?,
        ));
    }
    Ok(headers)
}

fn json_lookup<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn json_lookup_string(value: &Value, candidates: &[&[&str]]) -> Option<String> {
    candidates.iter().find_map(|path| {
        json_lookup(value, path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn json_lookup_i64(value: &Value, candidates: &[&[&str]]) -> Option<i64> {
    candidates.iter().find_map(|path| {
        let value = json_lookup(value, path)?;
        match value {
            Value::Number(value) => value.as_i64(),
            Value::String(value) => value.trim().parse::<i64>().ok(),
            _ => None,
        }
    })
}

fn json_lookup_boolish(value: &Value, candidates: &[&[&str]]) -> Option<bool> {
    candidates.iter().find_map(|path| {
        let value = json_lookup(value, path)?;
        match value {
            Value::Bool(value) => Some(*value),
            Value::Number(value) => value.as_u64().map(|value| value != 0),
            Value::String(value) => {
                let normalized = value.trim();
                match normalized {
                    "1" | "true" | "TRUE" => Some(true),
                    "0" | "false" | "FALSE" => Some(false),
                    _ => None,
                }
            }
            _ => None,
        }
    })
}

fn ensure_upload_success(value: &Value, action: &str) -> Result<(), BlobError> {
    if let Some(code) = json_lookup_string(value, &[&["code"], &["res_code"], &["resCode"]]) {
        if code == "SUCCESS" || code == "0" {
            return Ok(());
        }
        let message = json_lookup_string(value, &[&["msg"], &["message"], &["res_message"]])
            .unwrap_or_else(|| "unknown error".to_string());
        return Err(BlobError::Upstream(format!(
            "{action} returned code={code} ({message})"
        )));
    }
    if value
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| success)
    {
        return Ok(());
    }
    Ok(())
}

fn ensure_json_success(value: &Value, action: &str) -> Result<(), BlobError> {
    for key in ["res_code", "resCode", "code", "errorCode"] {
        let Some(code) = value.get(key).and_then(|value| match value {
            Value::String(value) => Some(value.trim().to_string()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        }) else {
            continue;
        };
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        if code == "0" || code.eq_ignore_ascii_case("SUCCESS") {
            return Ok(());
        }
        let message = json_lookup_string(value, &[&["res_message"], &["message"], &["msg"]])
            .unwrap_or_else(|| "unknown error".to_string());
        return Err(BlobError::Upstream(format!(
            "{action} returned {key}={code} ({message})"
        )));
    }

    if value
        .get("success")
        .and_then(Value::as_bool)
        .is_some_and(|success| !success)
    {
        let message = json_lookup_string(value, &[&["res_message"], &["message"], &["msg"]])
            .unwrap_or_else(|| "unknown error".to_string());
        return Err(BlobError::Upstream(format!("{action} failed: {message}")));
    }

    Ok(())
}

fn build_http_client(config: &TelecomConfig) -> Result<reqwest::Client, BlobError> {
    let mut builder = reqwest::Client::builder();
    if let Some(local_address) = config.outbound_ip_family.local_address() {
        builder = builder.local_address(local_address);
    }
    builder.build().map_err(|error| {
        BlobError::Configuration(format!(
            "failed to build China Telecom HTTP client: {error}"
        ))
    })
}

fn elapsed_millis(started_at: Instant) -> u64 {
    started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

async fn response_to_error(response: Response, action: &str) -> BlobError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<body unavailable>".to_string());
    BlobError::Upstream(format!(
        "{action} failed with HTTP {}: {}",
        status.as_u16(),
        truncate_for_error(body.trim(), 240)
    ))
}

fn truncate_for_error(body: &str, max_len: usize) -> String {
    if body.chars().count() <= max_len {
        body.to_string()
    } else {
        let truncated = body.chars().take(max_len).collect::<String>();
        format!("{truncated}...")
    }
}

fn deserialize_optional_string_like<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(serde_json::Value::Number(value)) => Ok(Some(value.to_string())),
        Some(serde_json::Value::Bool(value)) => Ok(Some(value.to_string())),
        Some(other) => Err(de::Error::custom(format!(
            "expected string-like value, got {other}"
        ))),
    }
}

fn deserialize_optional_u64_like<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(value)) => value
            .as_u64()
            .ok_or_else(|| de::Error::custom(format!("invalid unsigned integer: {value}")))
            .map(Some),
        Some(serde_json::Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|error| de::Error::custom(format!("invalid u64 string: {error}")))
            }
        }
        Some(other) => Err(de::Error::custom(format!(
            "expected u64-like value, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap},
        convert::Infallible,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::{
        DEFAULT_FAMILY_ROOT_FOLDER_ID, DEFAULT_ROOT_FOLDER_ID, ListObjectsRequest,
        TELECOM_FAMILY_CONTAINER, TELECOM_ROOT_CONTAINER, TelecomBlobAdapter, TelecomConfig,
        TokenSource, normalize_upload_public_key, telecom_signature, upload_public_key_base64_body,
    };
    use axum::{
        Router,
        body::{Body, Bytes as AxumBytes},
        extract::{Path, Query, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::{get, post, put},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use blob_core::{
        BlobBackend, BodySpoolLease, BodySpoolObserver, HealthStatus, ObjectBody, OutboundIpFamily,
        PutObjectRequest, StorageScopeKind,
    };
    use md5::{Digest, Md5};
    use percent_encoding::percent_decode_str;
    use serde_json::{Value, json};

    #[derive(Clone)]
    struct MockServerState {
        base_url: String,
        access_token: String,
        require_signed_download_url: bool,
        fail_family_list_with_internal_error: bool,
        entries_by_parent: Arc<BTreeMap<String, Vec<Value>>>,
        file_bodies_by_id: Arc<BTreeMap<String, Vec<u8>>>,
        requests: Arc<Mutex<Vec<Value>>>,
    }

    struct MockServer {
        base_url: String,
        requests: Arc<Mutex<Vec<Value>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    #[derive(Debug, Default)]
    struct MockSpoolStats {
        active_files: u64,
        active_bytes: u64,
        peak_files: u64,
        peak_bytes: u64,
    }

    #[derive(Debug)]
    struct MockSpoolObserver {
        stats: Arc<Mutex<MockSpoolStats>>,
    }

    #[derive(Debug)]
    struct MockSpoolLeaseImpl {
        stats: Arc<Mutex<MockSpoolStats>>,
        tracked_bytes: u64,
    }

    impl BodySpoolObserver for MockSpoolObserver {
        fn start_tracking(&self) -> Box<dyn BodySpoolLease> {
            let mut stats = self.stats.lock().expect("mock spool stats poisoned");
            stats.active_files += 1;
            stats.peak_files = stats.peak_files.max(stats.active_files);
            Box::new(MockSpoolLeaseImpl {
                stats: Arc::clone(&self.stats),
                tracked_bytes: 0,
            })
        }
    }

    impl BodySpoolLease for MockSpoolLeaseImpl {
        fn update_tracked_bytes(&mut self, next_bytes: u64) {
            let mut stats = self.stats.lock().expect("mock spool stats poisoned");
            stats.active_bytes = stats.active_bytes + next_bytes.saturating_sub(self.tracked_bytes);
            stats.peak_bytes = stats.peak_bytes.max(stats.active_bytes);
            self.tracked_bytes = next_bytes;
        }
    }

    impl Drop for MockSpoolLeaseImpl {
        fn drop(&mut self) {
            let mut stats = self.stats.lock().expect("mock spool stats poisoned");
            stats.active_files = stats.active_files.saturating_sub(1);
            stats.active_bytes = stats.active_bytes.saturating_sub(self.tracked_bytes);
        }
    }

    #[derive(Clone)]
    struct MockUploadServerState {
        base_url: String,
        entries_by_parent: Arc<Mutex<BTreeMap<String, Vec<Value>>>>,
        next_folder_id: Arc<AtomicUsize>,
        next_part_number: Arc<AtomicUsize>,
        created_folders: Arc<Mutex<Vec<HashMap<String, String>>>>,
        uploaded_parts: Arc<Mutex<Vec<(usize, Vec<u8>, Option<String>)>>>,
        control_headers: Arc<Mutex<Vec<HashMap<String, String>>>>,
    }

    struct MockUploadServer {
        base_url: String,
        created_folders: Arc<Mutex<Vec<HashMap<String, String>>>>,
        uploaded_parts: Arc<Mutex<Vec<(usize, Vec<u8>, Option<String>)>>>,
        control_headers: Arc<Mutex<Vec<HashMap<String, String>>>>,
        _task: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        async fn start(
            entries_by_parent: BTreeMap<String, Vec<Value>>,
            file_bodies_by_id: BTreeMap<String, Vec<u8>>,
            access_token: &str,
            require_signed_download_url: bool,
        ) -> Self {
            Self::start_with_options(
                entries_by_parent,
                file_bodies_by_id,
                access_token,
                require_signed_download_url,
                false,
            )
            .await
        }

        async fn start_with_options(
            entries_by_parent: BTreeMap<String, Vec<Value>>,
            file_bodies_by_id: BTreeMap<String, Vec<u8>>,
            access_token: &str,
            require_signed_download_url: bool,
            fail_family_list_with_internal_error: bool,
        ) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock telecom listener");
            let addr = listener.local_addr().expect("mock telecom local addr");
            let base_url = format!("http://{addr}");
            let state = MockServerState {
                base_url: base_url.clone(),
                access_token: access_token.to_string(),
                require_signed_download_url,
                fail_family_list_with_internal_error,
                entries_by_parent: Arc::new(entries_by_parent),
                file_bodies_by_id: Arc::new(file_bodies_by_id),
                requests: requests.clone(),
            };

            let app = Router::new()
                .route("/api/open/file/listFiles.action", get(mock_list_files))
                .route(
                    "/api/open/user/getUserInfoForPortal.action",
                    get(mock_get_user_info),
                )
                .route(
                    "/api/open/file/getFileDownloadUrl.action",
                    get(mock_get_download_url),
                )
                .route(
                    "/api/portal/createBatchTask.action",
                    post(mock_create_batch_task),
                )
                .route(
                    "/api/portal/checkBatchTask.action",
                    get(mock_check_batch_task),
                )
                .route(
                    "/open/family/file/listFiles.action",
                    get(mock_family_list_files),
                )
                .route(
                    "/open/family/file/getFileDownloadUrl.action",
                    get(mock_family_get_download_url),
                )
                .route(
                    "/open/batch/createBatchTask.action",
                    post(mock_family_create_batch_task),
                )
                .route(
                    "/open/batch/checkBatchTask.action",
                    post(mock_family_check_batch_task),
                )
                .route("/download/{id}", get(mock_download))
                .with_state(state);
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("mock telecom server should stay available");
            });

            Self {
                base_url,
                requests,
                _task: task,
            }
        }

        fn requests(&self) -> Vec<Value> {
            self.requests.lock().expect("mock telecom requests").clone()
        }
    }

    impl MockUploadServer {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock telecom upload listener");
            let addr = listener
                .local_addr()
                .expect("mock telecom upload local addr");
            let base_url = format!("http://{addr}");
            let state = MockUploadServerState {
                base_url: base_url.clone(),
                entries_by_parent: Arc::new(Mutex::new(BTreeMap::from([(
                    DEFAULT_ROOT_FOLDER_ID.to_string(),
                    Vec::new(),
                )]))),
                next_folder_id: Arc::new(AtomicUsize::new(1)),
                next_part_number: Arc::new(AtomicUsize::new(1)),
                created_folders: Arc::new(Mutex::new(Vec::new())),
                uploaded_parts: Arc::new(Mutex::new(Vec::new())),
                control_headers: Arc::new(Mutex::new(Vec::new())),
            };
            let app = Router::new()
                .route(
                    "/api/open/file/listFiles.action",
                    get(mock_upload_list_files),
                )
                .route(
                    "/api/open/file/createFolder.action",
                    post(mock_upload_create_folder),
                )
                .route(
                    "/api/portal/v2/getUserBriefInfo.action",
                    get(mock_upload_get_user_brief_info),
                )
                .route(
                    "/api/security/generateRsaKey.action",
                    get(mock_upload_generate_rsa_key),
                )
                .route(
                    "/person/initMultiUpload",
                    get(mock_upload_init_multi_upload),
                )
                .route(
                    "/person/getMultiUploadUrls",
                    get(mock_upload_get_multi_upload_urls),
                )
                .route(
                    "/person/checkTransSecond",
                    get(mock_upload_check_trans_second),
                )
                .route(
                    "/person/commitMultiUploadFile",
                    get(mock_upload_commit_multi_upload_file),
                )
                .route("/upload/{part_number}", put(mock_upload_part))
                .with_state(state.clone());
            let created_folders = Arc::clone(&state.created_folders);
            let uploaded_parts = Arc::clone(&state.uploaded_parts);
            let control_headers = Arc::clone(&state.control_headers);
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("mock telecom upload server should stay available");
            });

            Self {
                base_url,
                created_folders,
                uploaded_parts,
                control_headers,
                _task: task,
            }
        }
    }

    async fn mock_upload_list_files(
        State(state): State<MockUploadServerState>,
        Query(query): Query<HashMap<String, String>>,
    ) -> axum::Json<Value> {
        let folder_id = query
            .get("folderId")
            .cloned()
            .unwrap_or_else(|| DEFAULT_ROOT_FOLDER_ID.to_string());
        let entries = state
            .entries_by_parent
            .lock()
            .expect("mock upload entries poisoned")
            .get(&folder_id)
            .cloned()
            .unwrap_or_default();
        let total_count = entries.len();
        let mut folder_list = Vec::new();
        let mut file_list = Vec::new();
        for entry in entries {
            if entry["isFolder"].as_bool().unwrap_or(false) {
                folder_list.push(entry);
            } else {
                file_list.push(entry);
            }
        }
        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileListAO": {
                "count": total_count,
                "fileList": file_list,
                "folderList": folder_list,
            }
        }))
    }

    async fn mock_upload_create_folder(
        State(state): State<MockUploadServerState>,
        body: AxumBytes,
    ) -> axum::Json<Value> {
        let fields = parse_form_fields(body.as_ref());
        let parent_folder_id = fields
            .get("parentFolderId")
            .cloned()
            .unwrap_or_else(|| DEFAULT_ROOT_FOLDER_ID.to_string());
        let folder_name = fields
            .get("folderName")
            .cloned()
            .unwrap_or_else(|| "folder".to_string());
        state
            .created_folders
            .lock()
            .expect("mock created folders poisoned")
            .push(fields.clone());
        let folder_id = format!(
            "dir-created-{}",
            state.next_folder_id.fetch_add(1, Ordering::SeqCst)
        );
        let entry = json!({
            "isFolder": true,
            "id": folder_id,
            "parentId": parent_folder_id,
            "name": folder_name,
            "createDate": "2026-05-21 03:43:54",
            "lastOpTime": "2026-05-21 03:43:54",
            "fileCount": 0,
        });
        state
            .entries_by_parent
            .lock()
            .expect("mock upload entries poisoned")
            .entry(parent_folder_id)
            .or_default()
            .push(entry.clone());
        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "id": entry["id"],
            "name": entry["name"],
            "createDate": entry["createDate"],
            "lastOpTime": entry["lastOpTime"],
        }))
    }

    async fn mock_upload_get_user_brief_info() -> axum::Json<Value> {
        axum::Json(json!({
            "sessionKey": "telecom-session-key"
        }))
    }

    async fn mock_upload_generate_rsa_key() -> axum::Json<Value> {
        axum::Json(json!({
            "pkId": "pk-test-1",
            "pubKey": TEST_UPLOAD_PUBLIC_KEY
        }))
    }

    async fn mock_upload_init_multi_upload(
        State(state): State<MockUploadServerState>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .control_headers
            .lock()
            .expect("mock control headers poisoned")
            .push(control_header_snapshot(&headers));
        axum::Json(json!({
            "code": "SUCCESS",
            "data": {
                "uploadType": 1,
                "uploadHost": state.base_url,
                "uploadFileId": "upload-file-1",
                "fileDataExists": 0
            }
        }))
    }

    async fn mock_upload_get_multi_upload_urls(
        State(state): State<MockUploadServerState>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .control_headers
            .lock()
            .expect("mock control headers poisoned")
            .push(control_header_snapshot(&headers));
        let part_number = state.next_part_number.fetch_add(1, Ordering::SeqCst);
        let part_md5_base64 = sample_upload_part_md5_base64s()
            .get(part_number.saturating_sub(1))
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        axum::Json(json!({
            "code": "SUCCESS",
            "uploadUrls": {
                format!("partNumber_{part_number}"): {
                    "requestURL": format!("{}/upload/{}", state.base_url, part_number),
                    "requestHeader": format!("Content-Type=application/octet-stream&Authorization=AWS test:key&Content-MD5={part_md5_base64}&x-amz-date=Wed, 20 May 2026 19:43:33 GMT&x-amz-limit=rate=12800")
                }
            }
        }))
    }

    async fn mock_upload_check_trans_second(
        State(state): State<MockUploadServerState>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .control_headers
            .lock()
            .expect("mock control headers poisoned")
            .push(control_header_snapshot(&headers));
        axum::Json(json!({
            "code": "SUCCESS",
            "data": {
                "uploadFileId": "upload-file-1",
                "fileDataExists": 1
            }
        }))
    }

    async fn mock_upload_commit_multi_upload_file(
        State(state): State<MockUploadServerState>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .control_headers
            .lock()
            .expect("mock control headers poisoned")
            .push(control_header_snapshot(&headers));
        axum::Json(json!({
            "code": "SUCCESS",
            "file": {
                "userFileId": "user-file-1",
                "fileMd5": "5EB63BBBE01EEED093CB22BB8F5ACDC3"
            }
        }))
    }

    async fn mock_upload_part(
        State(state): State<MockUploadServerState>,
        Path(part_number): Path<usize>,
        headers: HeaderMap,
        body: AxumBytes,
    ) -> impl IntoResponse {
        state
            .uploaded_parts
            .lock()
            .expect("mock uploaded parts poisoned")
            .push((
                part_number,
                body.to_vec(),
                header_to_string(&headers, "Content-MD5"),
            ));
        (StatusCode::OK, Body::from(Vec::<u8>::new()))
    }

    async fn mock_list_files(
        State(state): State<MockServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        let folder_id = query
            .get("folderId")
            .cloned()
            .unwrap_or_else(|| DEFAULT_ROOT_FOLDER_ID.to_string());
        let page_num = query
            .get("pageNum")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let page_size = query
            .get("pageSize")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(60);

        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "list",
                "folderId": folder_id,
                "pageNum": page_num,
                "pageSize": page_size,
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
                "accessToken": header_to_string(&headers, "AccessToken"),
                "signature": header_to_string(&headers, "Signature"),
            }));

        let entries = state
            .entries_by_parent
            .get(&folder_id)
            .cloned()
            .unwrap_or_default();
        let total_count = entries.len();
        let page_entries = entries
            .into_iter()
            .skip(page_num.saturating_sub(1) * page_size)
            .take(page_size)
            .collect::<Vec<_>>();
        let mut folder_list = Vec::new();
        let mut file_list = Vec::new();
        for entry in page_entries {
            if entry["isFolder"].as_bool().unwrap_or(false) {
                folder_list.push(entry);
            } else {
                file_list.push(entry);
            }
        }

        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileListAO": {
                "count": total_count,
                "fileList": file_list,
                "folderList": folder_list,
            },
            "lastRev": 20260426191055u64,
        }))
    }

    async fn mock_get_user_info(
        State(state): State<MockServerState>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "user_info",
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
                "accessToken": header_to_string(&headers, "AccessToken"),
                "signature": header_to_string(&headers, "Signature"),
            }));

        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "capacity": 1024u64,
            "available": 256u64,
        }))
    }

    async fn mock_get_download_url(
        State(state): State<MockServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        let file_id = query
            .get("fileId")
            .cloned()
            .expect("mock telecom download request should contain fileId");
        let dt = query.get("dt").cloned();
        let share_id = query.get("shareId").cloned();
        let access_token = header_to_string(&headers, "AccessToken");
        let timestamp = header_to_string(&headers, "Timestamp");
        let signature = header_to_string(&headers, "Signature");
        let signed = access_token.is_some() && timestamp.is_some() && signature.is_some();

        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "download_url",
                "fileId": file_id,
                "dt": dt,
                "shareId": share_id,
                "signed": signed,
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
                "accessToken": access_token,
                "timestamp": timestamp,
                "signature": signature,
            }));

        if !state.file_bodies_by_id.contains_key(&file_id) {
            return axum::Json(json!({
                "res_code": "FileNotFound",
                "res_message": "文件不存在",
            }));
        }

        if state.require_signed_download_url {
            let Some(access_token) = header_to_string(&headers, "AccessToken") else {
                return axum::Json(json!({
                    "res_code": "InvalidSignature",
                    "res_message": "signature required",
                }));
            };
            let Some(timestamp) = header_to_string(&headers, "Timestamp") else {
                return axum::Json(json!({
                    "res_code": "InvalidSignature",
                    "res_message": "timestamp required",
                }));
            };
            let Some(signature) = header_to_string(&headers, "Signature") else {
                return axum::Json(json!({
                    "res_code": "InvalidSignature",
                    "res_message": "signature required",
                }));
            };

            let expected = telecom_signature(&[
                ("AccessToken".to_string(), access_token.clone()),
                ("Timestamp".to_string(), timestamp),
                ("fileId".to_string(), file_id.clone()),
                ("dt".to_string(), "1".to_string()),
                ("shareId".to_string(), String::new()),
            ]);
            if access_token != state.access_token || signature != expected {
                return axum::Json(json!({
                    "res_code": "InvalidSignature",
                    "res_message": "signature mismatch",
                }));
            }
        }

        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileDownloadUrl": format!("{}/download/{}", state.base_url, file_id),
        }))
    }

    async fn mock_create_batch_task(
        State(state): State<MockServerState>,
        headers: HeaderMap,
        body: AxumBytes,
    ) -> axum::Json<Value> {
        let fields = parse_form_fields(body.as_ref());
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "create_batch",
                "fields": fields,
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
            }));
        axum::Json(json!({
            "res_code": 0,
            "taskId": "task-delete-1",
        }))
    }

    async fn mock_check_batch_task(
        State(state): State<MockServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "check_batch",
                "query": query,
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
            }));
        axum::Json(json!({
            "res_code": 0,
            "taskStatus": 4,
            "failedCount": 0,
            "successedCount": 1,
            "subTaskCount": 1,
        }))
    }

    async fn mock_family_list_files(
        State(state): State<MockServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        let folder_id = query
            .get("folderId")
            .cloned()
            .unwrap_or_else(|| DEFAULT_FAMILY_ROOT_FOLDER_ID.to_string());
        let page_num = query
            .get("pageNum")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let page_size = query
            .get("pageSize")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(60);
        let entries = state
            .entries_by_parent
            .get(&folder_id)
            .cloned()
            .unwrap_or_default();
        let total_count = entries.len();
        let page_entries = entries
            .into_iter()
            .skip(page_num.saturating_sub(1) * page_size)
            .take(page_size)
            .collect::<Vec<_>>();
        let mut folder_list = Vec::new();
        let mut file_list = Vec::new();
        for entry in page_entries {
            if entry["isFolder"].as_bool().unwrap_or(false) {
                folder_list.push(entry);
            } else {
                file_list.push(entry);
            }
        }

        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "family_list",
                "familyId": query.get("familyId").cloned(),
                "folderId": folder_id,
                "pageNum": page_num,
                "pageSize": page_size,
                "browserId": header_to_string(&headers, "Browser-Id"),
                "cookie": header_to_string(&headers, "Cookie"),
                "accessToken": header_to_string(&headers, "AccessToken"),
                "timestamp": header_to_string(&headers, "Timestamp"),
                "signature": header_to_string(&headers, "Signature"),
            }));

        if state.fail_family_list_with_internal_error {
            return axum::Json(json!({
                "code": "InternalError",
                "message": "系统错误",
            }));
        }

        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileListAO": {
                "count": total_count,
                "fileList": file_list,
                "folderList": folder_list,
            }
        }))
    }

    async fn mock_family_get_download_url(
        State(state): State<MockServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> axum::Json<Value> {
        let file_id = query
            .get("fileId")
            .cloned()
            .expect("mock family download request should contain fileId");
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "family_download_url",
                "fileId": file_id,
                "familyId": query.get("familyId").cloned(),
                "type": query.get("type").cloned(),
                "accessToken": header_to_string(&headers, "AccessToken"),
                "timestamp": header_to_string(&headers, "Timestamp"),
                "signature": header_to_string(&headers, "Signature"),
            }));
        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileDownloadUrl": format!("{}/download/{}", state.base_url, file_id),
        }))
    }

    async fn mock_family_create_batch_task(
        State(state): State<MockServerState>,
        headers: HeaderMap,
        body: AxumBytes,
    ) -> axum::Json<Value> {
        let fields = parse_form_fields(body.as_ref());
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "family_create_batch",
                "fields": fields,
                "accessToken": header_to_string(&headers, "AccessToken"),
                "timestamp": header_to_string(&headers, "Timestamp"),
                "signature": header_to_string(&headers, "Signature"),
            }));
        axum::Json(json!({
            "res_code": 0,
            "taskId": "family-task-delete-1",
        }))
    }

    async fn mock_family_check_batch_task(
        State(state): State<MockServerState>,
        headers: HeaderMap,
        body: AxumBytes,
    ) -> axum::Json<Value> {
        let fields = parse_form_fields(body.as_ref());
        state
            .requests
            .lock()
            .expect("record mock request")
            .push(json!({
                "kind": "family_check_batch",
                "fields": fields,
                "accessToken": header_to_string(&headers, "AccessToken"),
                "timestamp": header_to_string(&headers, "Timestamp"),
                "signature": header_to_string(&headers, "Signature"),
            }));
        axum::Json(json!({
            "res_code": 0,
            "taskStatus": 4,
            "failedCount": 0,
            "successedCount": 1,
            "subTaskCount": 1,
        }))
    }

    async fn mock_download(
        State(state): State<MockServerState>,
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

    fn header_to_string(headers: &HeaderMap, key: &str) -> Option<String> {
        headers
            .get(key)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    fn control_header_snapshot(headers: &HeaderMap) -> HashMap<String, String> {
        let mut snapshot = HashMap::new();
        for key in [
            "SessionKey",
            "Signature",
            "X-Request-Date",
            "X-Request-ID",
            "EncryptionText",
            "PkId",
        ] {
            if let Some(value) = header_to_string(headers, key) {
                snapshot.insert(key.to_string(), value);
            }
        }
        snapshot
    }

    fn parse_form_fields(body: &[u8]) -> HashMap<String, String> {
        String::from_utf8_lossy(body)
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| {
                let decode = |raw: &str| {
                    percent_decode_str(raw.replace('+', " ").as_str())
                        .decode_utf8_lossy()
                        .to_string()
                };
                (decode(key), decode(value))
            })
            .collect()
    }

    fn sample_upload_part_md5_base64s() -> Vec<String> {
        [b"hello".as_slice(), b" worl".as_slice(), b"d".as_slice()]
            .into_iter()
            .map(|chunk| BASE64_STANDARD.encode(Md5::digest(chunk)))
            .collect()
    }

    const TEST_UPLOAD_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAp4kWHd3tlFN8eN/7UySQ\n\
evD4chicCtg2JX5Nxzx0XvQAh/EWIUoxVIv0uR20AibvvyT0Fve5RFl/NH9ucPgL\n\
LlEhEXkyrMO4NW3TvuRcPiZZ5L2nYeqPNlbWHWSQRDoQ5L6mECQvKQdpC8d45Gji\n\
xMJsVFkWiEytImij397eCWa+m20V7sgNETyef6juyb1ayQT2Tj5rmOULRCb04/Sx\n\
phMsqpP32N4U1nXUE/1A5NYRCEKeUD6R3DOy5+H5n3n+AGzmhzVXszQ3Mhk5BhLx\n\
b7M846HRxQsjn/7FbjVA3Fqhui/LE3FAoE4iLwmVsICOXtZUFqcuc5iSMLqmAWAh\n\
AwIDAQAB\n\
-----END PUBLIC KEY-----";

    fn mock_telecom_adapter(
        base_url: &str,
        browser_id: &str,
        cookie_header: &str,
        token: Option<&str>,
    ) -> TelecomBlobAdapter {
        mock_telecom_adapter_with_timeout(base_url, browser_id, cookie_header, token, 10)
    }

    fn mock_telecom_adapter_with_timeout(
        base_url: &str,
        browser_id: &str,
        cookie_header: &str,
        token: Option<&str>,
        request_timeout_secs: u64,
    ) -> TelecomBlobAdapter {
        TelecomBlobAdapter::new(TelecomConfig {
            base_url: base_url.to_string(),
            token_source: token
                .map(|value| TokenSource::Static {
                    bearer: value.to_string(),
                })
                .unwrap_or(TokenSource::EnvVar {
                    key: "UNUSED_TELECOM_TOKEN".to_string(),
                }),
            outbound_ip_family: OutboundIpFamily::Auto,
            browser_id: Some(browser_id.to_string()),
            cookie_header: Some(cookie_header.to_string()),
            user_agent: "carrier-cloud-blob-gateway/test".to_string(),
            browser_profile: None,
            request_timeout_secs,
            sign_type: "1".to_string(),
            family_id: None,
            root_folder_id: DEFAULT_ROOT_FOLDER_ID.to_string(),
            page_size: 2,
            root_prefix: None,
            upload_part_size_bytes: 0,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            body_spool_dir: None,
            body_spool_observer: None,
        })
        .expect("telecom test adapter should build")
    }

    struct SlowDownloadServer {
        base_url: String,
        _task: tokio::task::JoinHandle<()>,
    }

    #[derive(Clone)]
    struct SlowDownloadServerState {
        base_url: String,
        file_size: usize,
        chunks: Arc<Vec<(u64, Vec<u8>)>>,
    }

    impl SlowDownloadServer {
        async fn start(chunks: Vec<(u64, Vec<u8>)>) -> Self {
            let file_size = chunks.iter().map(|(_, chunk)| chunk.len()).sum::<usize>();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind slow telecom listener");
            let addr = listener.local_addr().expect("slow telecom local addr");
            let base_url = format!("http://{addr}");
            let state = SlowDownloadServerState {
                base_url: base_url.clone(),
                file_size,
                chunks: Arc::new(chunks),
            };
            let app = Router::new()
                .route(
                    "/api/open/file/listFiles.action",
                    get(slow_download_list_files),
                )
                .route(
                    "/api/open/file/getFileDownloadUrl.action",
                    get(slow_download_url),
                )
                .route("/download/file-slow", get(slow_download_body))
                .with_state(state);
            let task = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("slow telecom server should stay available");
            });

            Self {
                base_url,
                _task: task,
            }
        }
    }

    async fn slow_download_list_files(
        State(state): State<SlowDownloadServerState>,
    ) -> axum::Json<Value> {
        axum::Json(json!({
            "res_code": 0,
            "res_message": "成功",
            "fileListAO": {
                "count": 1,
                "fileList": [{
                    "isFolder": false,
                    "id": "file-slow",
                    "parentId": DEFAULT_ROOT_FOLDER_ID,
                    "name": "slow.bin",
                    "fileSize": state.file_size.to_string(),
                    "createDate": "2026-05-23 07:44:56",
                    "lastOpTime": "2026-05-23 07:44:56"
                }],
                "folderList": []
            }
        }))
    }

    async fn slow_download_url(
        State(state): State<SlowDownloadServerState>,
        Query(query): Query<HashMap<String, String>>,
    ) -> axum::Json<Value> {
        let file_id = query
            .get("fileId")
            .cloned()
            .unwrap_or_else(|| "missing".to_string());
        if file_id != "file-slow" {
            return axum::Json(json!({
                "res_code": "FileNotFound",
                "res_message": "文件不存在"
            }));
        }
        axum::Json(json!({
            "res_code": 0,
            "fileDownloadUrl": format!("{}/download/{file_id}", state.base_url),
        }))
    }

    async fn slow_download_body(State(state): State<SlowDownloadServerState>) -> impl IntoResponse {
        let chunks = Arc::clone(&state.chunks);
        let body = Body::from_stream(futures_util::stream::unfold(
            (0usize, chunks),
            move |(index, chunks)| async move {
                let Some((delay_ms, chunk)) = chunks.get(index).cloned() else {
                    return None;
                };
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                Some((
                    Ok::<AxumBytes, Infallible>(AxumBytes::from(chunk)),
                    (index + 1, chunks),
                ))
            },
        ));
        (StatusCode::OK, body)
    }

    fn mock_telecom_family_adapter(
        base_url: &str,
        browser_id: &str,
        cookie_header: &str,
        token: &str,
        family_id: &str,
    ) -> TelecomBlobAdapter {
        TelecomBlobAdapter::new(TelecomConfig {
            base_url: base_url.to_string(),
            token_source: TokenSource::Static {
                bearer: token.to_string(),
            },
            outbound_ip_family: OutboundIpFamily::Auto,
            browser_id: Some(browser_id.to_string()),
            cookie_header: Some(cookie_header.to_string()),
            user_agent: "carrier-cloud-blob-gateway/test".to_string(),
            browser_profile: None,
            request_timeout_secs: 10,
            sign_type: "1".to_string(),
            family_id: Some(family_id.to_string()),
            root_folder_id: DEFAULT_ROOT_FOLDER_ID.to_string(),
            page_size: 2,
            root_prefix: None,
            upload_part_size_bytes: 0,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            body_spool_dir: None,
            body_spool_observer: None,
        })
        .expect("telecom family test adapter should build")
    }

    fn mock_telecom_adapter_with_root_prefix(
        base_url: &str,
        browser_id: &str,
        cookie_header: &str,
        root_prefix: &str,
    ) -> TelecomBlobAdapter {
        TelecomBlobAdapter::new(TelecomConfig {
            base_url: base_url.to_string(),
            token_source: TokenSource::EnvVar {
                key: "UNUSED_TELECOM_TOKEN".to_string(),
            },
            outbound_ip_family: OutboundIpFamily::Auto,
            browser_id: Some(browser_id.to_string()),
            cookie_header: Some(cookie_header.to_string()),
            user_agent: "carrier-cloud-blob-gateway/test".to_string(),
            browser_profile: None,
            request_timeout_secs: 10,
            sign_type: "1".to_string(),
            family_id: None,
            root_folder_id: DEFAULT_ROOT_FOLDER_ID.to_string(),
            page_size: 2,
            root_prefix: Some(root_prefix.to_string()),
            upload_part_size_bytes: 0,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            body_spool_dir: None,
            body_spool_observer: None,
        })
        .expect("telecom managed-root adapter should build")
    }

    fn mock_upload_adapter(
        base_url: &str,
        spool_stats: Arc<Mutex<MockSpoolStats>>,
    ) -> TelecomBlobAdapter {
        TelecomBlobAdapter::new(TelecomConfig {
            base_url: base_url.to_string(),
            token_source: TokenSource::EnvVar {
                key: "UNUSED_TELECOM_TOKEN".to_string(),
            },
            outbound_ip_family: OutboundIpFamily::Auto,
            browser_id: Some("browser-id-123".to_string()),
            cookie_header: Some("JSESSIONID=abc; COOKIE_LOGIN_USER=def".to_string()),
            user_agent: "carrier-cloud-blob-gateway/test".to_string(),
            browser_profile: None,
            request_timeout_secs: 10,
            sign_type: "1".to_string(),
            family_id: None,
            root_folder_id: DEFAULT_ROOT_FOLDER_ID.to_string(),
            page_size: 20,
            root_prefix: Some("ccbg-tests".to_string()),
            upload_part_size_bytes: 5,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            body_spool_dir: None,
            body_spool_observer: Some(Arc::new(MockSpoolObserver { stats: spool_stats })),
        })
        .expect("telecom upload adapter should build")
    }

    fn sample_entries() -> BTreeMap<String, Vec<Value>> {
        BTreeMap::from([
            (
                DEFAULT_ROOT_FOLDER_ID.to_string(),
                vec![
                    json!({
                        "isFolder": true,
                        "id": "dir-docs",
                        "parentId": DEFAULT_ROOT_FOLDER_ID,
                        "name": "docs",
                        "createDate": "2026-04-17 00:14:15",
                        "lastOpTime": "2026-04-17 00:14:15",
                        "fileCount": 3,
                    }),
                    json!({
                        "isFolder": true,
                        "id": "dir-media",
                        "parentId": DEFAULT_ROOT_FOLDER_ID,
                        "name": "media",
                        "createDate": "2026-04-17 00:14:15",
                        "lastOpTime": "2026-04-17 00:14:15",
                        "fileCount": 1,
                    }),
                    json!({
                        "isFolder": false,
                        "id": "file-root",
                        "parentId": DEFAULT_ROOT_FOLDER_ID,
                        "name": "zzz-root.txt",
                        "fileSize": "7",
                        "createDate": "2026-04-17 00:14:15",
                        "lastOpTime": "2026-04-17 00:14:15",
                        "md5": "root-md5",
                    }),
                ],
            ),
            (
                "dir-docs".to_string(),
                vec![
                    json!({
                        "isFolder": false,
                        "id": "file-alpha",
                        "parentId": "dir-docs",
                        "name": "alpha.txt",
                        "fileSize": 5,
                        "createDate": "2026-04-25 12:30:45",
                        "lastOpTime": "2026-04-25 12:30:45",
                        "md5": "md5-alpha",
                    }),
                    json!({
                        "isFolder": false,
                        "id": "file-beta",
                        "parentId": "dir-docs",
                        "fileName": "beta.json",
                        "fileSize": "6",
                        "createDate": "2026-04-25 12:31:00",
                        "lastOpTime": "2026-04-25 12:31:00",
                    }),
                    json!({
                        "isFolder": true,
                        "id": "dir-nested",
                        "parentId": "dir-docs",
                        "name": "nested",
                        "createDate": "2026-04-25 13:00:00",
                        "lastOpTime": "2026-04-25 13:00:00",
                        "fileCount": 1,
                    }),
                ],
            ),
            (
                "dir-nested".to_string(),
                vec![json!({
                    "isFolder": false,
                    "id": "file-zeta",
                    "parentId": "dir-nested",
                    "name": "zeta.log",
                    "size": 9,
                    "createDate": "2026-04-25 15:00:00",
                    "lastOpTime": "2026-04-25 15:00:00",
                })],
            ),
            (
                "dir-media".to_string(),
                vec![json!({
                    "isFolder": false,
                    "id": "file-photo",
                    "parentId": "dir-media",
                    "name": "photo.png",
                    "size": 11,
                    "createDate": "2026-04-25 14:00:00",
                    "lastOpTime": "2026-04-25 14:00:00",
                })],
            ),
        ])
    }

    fn sample_entries_under_managed_root() -> BTreeMap<String, Vec<Value>> {
        BTreeMap::from([
            (
                DEFAULT_ROOT_FOLDER_ID.to_string(),
                vec![
                    json!({
                        "isFolder": true,
                        "id": "dir-ccbg",
                        "parentId": DEFAULT_ROOT_FOLDER_ID,
                        "name": "ccbg-managed",
                        "createDate": "2026-04-17 00:14:15",
                        "lastOpTime": "2026-04-17 00:14:15",
                        "fileCount": 2,
                    }),
                    json!({
                        "isFolder": true,
                        "id": "dir-other",
                        "parentId": DEFAULT_ROOT_FOLDER_ID,
                        "name": "unrelated",
                        "createDate": "2026-04-17 00:14:15",
                        "lastOpTime": "2026-04-17 00:14:15",
                        "fileCount": 1,
                    }),
                ],
            ),
            (
                "dir-ccbg".to_string(),
                vec![
                    json!({
                        "isFolder": true,
                        "id": "dir-managed-docs",
                        "parentId": "dir-ccbg",
                        "name": "docs",
                        "createDate": "2026-04-25 13:00:00",
                        "lastOpTime": "2026-04-25 13:00:00",
                        "fileCount": 1,
                    }),
                    json!({
                        "isFolder": false,
                        "id": "file-managed-root",
                        "parentId": "dir-ccbg",
                        "name": "root-note.txt",
                        "fileSize": 10,
                        "createDate": "2026-04-25 12:30:45",
                        "lastOpTime": "2026-04-25 12:30:45",
                        "md5": "md5-root-note",
                    }),
                ],
            ),
            (
                "dir-managed-docs".to_string(),
                vec![json!({
                    "isFolder": false,
                    "id": "file-managed-alpha",
                    "parentId": "dir-managed-docs",
                    "name": "alpha.txt",
                    "fileSize": 5,
                    "createDate": "2026-04-25 12:30:45",
                    "lastOpTime": "2026-04-25 12:30:45",
                    "md5": "md5-managed-alpha",
                })],
            ),
            (
                "dir-other".to_string(),
                vec![json!({
                    "isFolder": false,
                    "id": "file-unrelated",
                    "parentId": "dir-other",
                    "name": "secret.txt",
                    "fileSize": 6,
                    "createDate": "2026-04-25 12:30:45",
                    "lastOpTime": "2026-04-25 12:30:45",
                    "md5": "md5-unrelated",
                })],
            ),
        ])
    }

    fn sample_entries_with_family() -> BTreeMap<String, Vec<Value>> {
        let mut entries = sample_entries();
        entries.insert(
            DEFAULT_FAMILY_ROOT_FOLDER_ID.to_string(),
            vec![
                json!({
                    "isFolder": true,
                    "id": "family-dir-docs",
                    "parentId": DEFAULT_FAMILY_ROOT_FOLDER_ID,
                    "fileName": "family-docs",
                    "createDate": "2026-04-28 12:00:00",
                    "lastOpTime": "2026-04-28 12:00:00",
                    "fileCount": 1,
                }),
                json!({
                    "isFolder": false,
                    "fileId": "family-file-root",
                    "parentId": DEFAULT_FAMILY_ROOT_FOLDER_ID,
                    "fileName": "family-root.txt",
                    "fileSize": 12,
                    "createDate": "2026-04-28 12:00:00",
                    "lastOpTime": "2026-04-28 12:00:00",
                }),
            ],
        );
        entries.insert(
            "family-dir-docs".to_string(),
            vec![json!({
                "isFolder": false,
                "fileId": "family-file-alpha",
                "parentId": "family-dir-docs",
                "fileName": "alpha.txt",
                "fileSize": 13,
                "createDate": "2026-04-28 12:30:00",
                "lastOpTime": "2026-04-28 12:30:00",
            })],
        );
        entries
    }

    fn sample_file_bodies() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            ("file-root".to_string(), b"root!!!".to_vec()),
            ("file-alpha".to_string(), b"alpha".to_vec()),
            ("file-beta".to_string(), b"123456".to_vec()),
            ("file-zeta".to_string(), b"123456789".to_vec()),
            ("file-photo".to_string(), b"hello-photo".to_vec()),
            ("file-managed-root".to_string(), b"root-note!".to_vec()),
            ("file-managed-alpha".to_string(), b"alpha".to_vec()),
            ("file-unrelated".to_string(), b"hidden".to_vec()),
            ("family-file-root".to_string(), b"family-root!".to_vec()),
            ("family-file-alpha".to_string(), b"family-alpha!".to_vec()),
        ])
    }

    #[test]
    fn telecom_signature_matches_frontend_formula() {
        assert_eq!(
            telecom_signature(&[
                ("AccessToken".to_string(), "abc".to_string()),
                ("Timestamp".to_string(), "123".to_string()),
                ("fileId".to_string(), "456".to_string()),
            ]),
            "6086b8dfe302cccbfc2a17e35b57b330"
        );
    }

    #[test]
    fn upload_public_key_normalization_removes_nul_padding() {
        assert_eq!(
            normalize_upload_public_key(
                "\0  -----BEGIN PUBLIC KEY-----\nabc\0\n-----END PUBLIC KEY-----  \0"
            ),
            "-----BEGIN PUBLIC KEY-----\nabc\n-----END PUBLIC KEY-----"
        );
    }

    #[test]
    fn upload_public_key_base64_body_ignores_pem_boundaries() {
        assert_eq!(
            upload_public_key_base64_body(
                "-----BEGIN PUBLIC KEY-----\nYWJj\nZGVm\n-----END PUBLIC KEY-----"
            ),
            "YWJjZGVm"
        );
    }

    #[tokio::test]
    async fn health_and_list_containers_hit_real_list_endpoint() {
        let server = MockServer::start(
            sample_entries(),
            sample_file_bodies(),
            "unused-token",
            false,
        )
        .await;
        let adapter = mock_telecom_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            None,
        );

        let health = adapter
            .health()
            .await
            .expect("telecom health should succeed");
        assert!(matches!(health.status, HealthStatus::Healthy));
        assert!(
            health
                .notes
                .iter()
                .any(|note| note.contains("root_entry_count=3"))
        );
        assert_eq!(health.scopes.len(), 1);
        assert_eq!(health.scopes[0].kind, StorageScopeKind::Personal);
        assert_eq!(
            health.scopes[0]
                .capacity
                .as_ref()
                .and_then(|capacity| capacity.total_bytes),
            Some(1024)
        );

        let containers = adapter
            .list_containers()
            .await
            .expect("telecom list_containers should succeed");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, TELECOM_ROOT_CONTAINER);

        let requests = server.requests();
        assert!(requests.iter().any(|request| request["kind"] == "list"));
        let first_list = requests
            .iter()
            .find(|request| request["kind"] == "list")
            .expect("expected at least one list request");
        assert_eq!(first_list["browserId"], "browser-id-123");
        assert_eq!(
            first_list["cookie"],
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def"
        );
        assert!(
            requests
                .iter()
                .any(|request| request["kind"] == "user_info")
        );
    }

    #[tokio::test]
    async fn health_and_list_containers_include_configured_family_scope() {
        let token = "telecom-family-access-token";
        let family_id = "family-123";
        let server = MockServer::start(
            sample_entries_with_family(),
            sample_file_bodies(),
            token,
            false,
        )
        .await;
        let adapter = mock_telecom_family_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            token,
            family_id,
        );

        let health = adapter
            .health()
            .await
            .expect("telecom family health should succeed");
        assert!(matches!(health.status, HealthStatus::Healthy));
        assert_eq!(health.scopes.len(), 2);
        let family_scope = health
            .scopes
            .iter()
            .find(|scope| scope.kind == StorageScopeKind::Family)
            .expect("family scope should be reported");
        assert_eq!(family_scope.id, family_id);
        assert_eq!(
            family_scope.container.as_deref(),
            Some(TELECOM_FAMILY_CONTAINER)
        );
        assert!(!family_scope.writable);

        let containers = adapter
            .list_containers()
            .await
            .expect("telecom family list_containers should succeed");
        let container_names = containers
            .iter()
            .map(|container| container.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            container_names,
            vec![TELECOM_ROOT_CONTAINER, TELECOM_FAMILY_CONTAINER]
        );

        let requests = server.requests();
        let family_lists = requests
            .iter()
            .filter(|request| request["kind"] == "family_list")
            .collect::<Vec<_>>();
        assert!(!family_lists.is_empty());
        assert!(family_lists.iter().all(|request| {
            request["familyId"].as_str() == Some(family_id)
                && request["accessToken"].as_str() == Some(token)
                && request["timestamp"].as_str().is_some()
                && request["signature"].as_str().is_some()
        }));
    }

    #[tokio::test]
    async fn health_stays_healthy_when_personal_scope_works_but_family_probe_fails() {
        let token = "telecom-family-access-token";
        let family_id = "family-123";
        let server = MockServer::start_with_options(
            sample_entries_with_family(),
            sample_file_bodies(),
            token,
            false,
            true,
        )
        .await;
        let adapter = mock_telecom_family_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            token,
            family_id,
        );

        let health = adapter
            .health()
            .await
            .expect("telecom health should succeed when personal scope is healthy");
        assert!(matches!(health.status, HealthStatus::Healthy));
        assert_eq!(health.scopes.len(), 1);
        assert!(
            health
                .notes
                .iter()
                .any(|note| note.contains("family_scope_probe_failed="))
        );
    }

    #[tokio::test]
    async fn list_containers_skips_family_when_family_probe_fails() {
        let token = "telecom-family-access-token";
        let family_id = "family-123";
        let server = MockServer::start_with_options(
            sample_entries_with_family(),
            sample_file_bodies(),
            token,
            false,
            true,
        )
        .await;
        let adapter = mock_telecom_family_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            token,
            family_id,
        );

        let containers = adapter
            .list_containers()
            .await
            .expect("telecom container listing should keep working without family scope");
        let container_names = containers
            .iter()
            .map(|container| container.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(container_names, vec![TELECOM_ROOT_CONTAINER]);
    }

    #[tokio::test]
    async fn list_objects_recurses_and_applies_prefix_limit() {
        let server = MockServer::start(
            sample_entries(),
            sample_file_bodies(),
            "unused-token",
            false,
        )
        .await;
        let adapter = mock_telecom_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            None,
        );

        let all_objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some(TELECOM_ROOT_CONTAINER.to_string()),
                prefix: None,
                limit: None,
            })
            .await
            .expect("telecom list_objects should succeed");
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
        assert_eq!(
            all_objects[0].last_modified.as_deref(),
            Some("2026-04-25T12:30:45.000Z")
        );
        assert_eq!(all_objects[0].content_type.as_deref(), Some("text/plain"));

        let limited = adapter
            .list_objects(ListObjectsRequest {
                container: Some(TELECOM_ROOT_CONTAINER.to_string()),
                prefix: Some("docs/".to_string()),
                limit: Some(1),
            })
            .await
            .expect("telecom limited list_objects should succeed");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].key, "docs/alpha.txt");
    }

    #[tokio::test]
    async fn head_and_get_object_use_precise_lookup_and_signed_download_retry() {
        let token = "telecom-access-token";
        let server = MockServer::start(sample_entries(), sample_file_bodies(), token, true).await;
        let adapter = mock_telecom_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            Some(token),
        );

        let head = adapter
            .head_object(TELECOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("telecom head_object should succeed");
        assert_eq!(head.key, "docs/alpha.txt");
        assert_eq!(head.size, 5);
        assert_eq!(head.etag.as_deref(), Some("md5-alpha"));

        let payload = adapter
            .get_object(TELECOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("telecom get_object should succeed");
        assert_eq!(payload.info.key, "docs/alpha.txt");
        assert!(payload.first_response_latency_ms.is_some());
        assert_eq!(
            payload
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"alpha"
        );

        let requests = server.requests();
        let download_requests = requests
            .iter()
            .filter(|request| request["kind"] == "download_url")
            .collect::<Vec<_>>();
        assert_eq!(download_requests.len(), 2);
        assert_eq!(download_requests[0]["signed"], false);
        assert_eq!(download_requests[0]["dt"], "1");
        assert_eq!(download_requests[0]["shareId"], "");
        assert_eq!(download_requests[1]["signed"], true);
        assert_eq!(download_requests[1]["dt"], "1");
        assert_eq!(download_requests[1]["shareId"], "");
        assert_eq!(download_requests[1]["accessToken"], token);
    }

    #[tokio::test]
    async fn family_list_get_and_delete_use_signed_family_endpoints() {
        let token = "telecom-family-access-token";
        let family_id = "family-123";
        let server = MockServer::start(
            sample_entries_with_family(),
            sample_file_bodies(),
            token,
            false,
        )
        .await;
        let adapter = mock_telecom_family_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            token,
            family_id,
        );

        let objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some(TELECOM_FAMILY_CONTAINER.to_string()),
                prefix: None,
                limit: None,
            })
            .await
            .expect("telecom family list_objects should succeed");
        let keys = objects
            .iter()
            .map(|object| object.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["family-docs/alpha.txt", "family-root.txt"]);

        let payload = adapter
            .get_object(TELECOM_FAMILY_CONTAINER, "family-docs/alpha.txt")
            .await
            .expect("telecom family get_object should succeed");
        assert_eq!(payload.info.key, "family-docs/alpha.txt");
        assert_eq!(
            payload
                .body
                .collect()
                .await
                .expect("family body should collect")
                .as_ref(),
            b"family-alpha!"
        );

        adapter
            .delete_object(TELECOM_FAMILY_CONTAINER, "family-docs/alpha.txt")
            .await
            .expect("telecom family delete_object should succeed");

        let requests = server.requests();
        let family_download = requests
            .iter()
            .find(|request| request["kind"] == "family_download_url")
            .expect("family download URL endpoint should be used");
        assert_eq!(family_download["fileId"], "family-file-alpha");
        assert_eq!(family_download["familyId"], family_id);
        assert_eq!(family_download["type"], "1");
        assert_eq!(family_download["accessToken"], token);
        assert!(family_download["signature"].as_str().is_some());

        let create_batch = requests
            .iter()
            .find(|request| request["kind"] == "family_create_batch")
            .expect("family delete should create a batch task");
        assert_eq!(create_batch["fields"]["type"], "DELETE");
        assert_eq!(create_batch["fields"]["familyId"], family_id);
        assert_eq!(create_batch["accessToken"], token);
        assert!(create_batch["signature"].as_str().is_some());
        let task_infos: Value =
            serde_json::from_str(create_batch["fields"]["taskInfos"].as_str().unwrap())
                .expect("family taskInfos should be JSON");
        assert_eq!(task_infos[0]["fileId"], "family-file-alpha");
        assert_eq!(task_infos[0]["fileName"], "alpha.txt");
        assert_eq!(task_infos[0]["srcParentId"], "family-dir-docs");

        let check_batch = requests
            .iter()
            .find(|request| request["kind"] == "family_check_batch")
            .expect("family delete should poll the batch task");
        assert_eq!(check_batch["fields"]["taskId"], "family-task-delete-1");
        assert_eq!(check_batch["fields"]["type"], "DELETE");
        assert_eq!(check_batch["accessToken"], token);
    }

    #[tokio::test]
    async fn delete_object_uses_personal_recycle_bin_batch_task() {
        let server = MockServer::start(
            sample_entries(),
            sample_file_bodies(),
            "unused-token",
            false,
        )
        .await;
        let adapter = mock_telecom_adapter(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            None,
        );

        adapter
            .delete_object(TELECOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("telecom personal delete_object should succeed");

        let requests = server.requests();
        let create_batch = requests
            .iter()
            .find(|request| request["kind"] == "create_batch")
            .expect("personal delete should create a batch task");
        assert_eq!(create_batch["fields"]["type"], "DELETE");
        assert_eq!(create_batch["browserId"], "browser-id-123");
        assert_eq!(
            create_batch["cookie"],
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def"
        );
        let task_infos: Value =
            serde_json::from_str(create_batch["fields"]["taskInfos"].as_str().unwrap())
                .expect("personal taskInfos should be JSON");
        assert_eq!(task_infos[0]["fileId"], "file-alpha");
        assert_eq!(task_infos[0]["fileName"], "alpha.txt");
        assert_eq!(task_infos[0]["isFolder"].as_i64(), Some(0));
        assert_eq!(task_infos[0]["srcParentId"], "dir-docs");

        let check_batch = requests
            .iter()
            .find(|request| request["kind"] == "check_batch")
            .expect("personal delete should poll the batch task");
        assert_eq!(check_batch["query"]["taskId"], "task-delete-1");
        assert_eq!(check_batch["query"]["type"], "DELETE");
    }

    #[tokio::test]
    async fn managed_root_head_list_and_get_are_scoped_under_one_provider_folder() {
        let server = MockServer::start(
            sample_entries_under_managed_root(),
            sample_file_bodies(),
            "unused-token",
            false,
        )
        .await;
        let adapter = mock_telecom_adapter_with_root_prefix(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            "ccbg-managed",
        );

        let objects = adapter
            .list_objects(ListObjectsRequest {
                container: Some(TELECOM_ROOT_CONTAINER.to_string()),
                prefix: None,
                limit: None,
            })
            .await
            .expect("managed root list should succeed");
        let keys = objects
            .iter()
            .map(|object| object.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["docs/alpha.txt", "root-note.txt"]);

        let head = adapter
            .head_object(TELECOM_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("managed root head should succeed");
        assert_eq!(head.key, "docs/alpha.txt");
        assert_eq!(head.etag.as_deref(), Some("md5-managed-alpha"));

        let payload = adapter
            .get_object(TELECOM_ROOT_CONTAINER, "root-note.txt")
            .await
            .expect("managed root get should succeed");
        assert_eq!(
            payload
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"root-note!"
        );

        assert!(
            adapter
                .head_object(TELECOM_ROOT_CONTAINER, "secret.txt")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn get_object_allows_streaming_downloads_beyond_total_timeout_when_progress_continues() {
        let server = SlowDownloadServer::start(vec![
            (0, b"abc".to_vec()),
            (600, b"def".to_vec()),
            (600, b"ghi".to_vec()),
        ])
        .await;
        let adapter = mock_telecom_adapter_with_timeout(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            None,
            1,
        );

        let payload = adapter
            .get_object(TELECOM_ROOT_CONTAINER, "slow.bin")
            .await
            .expect("slow telecom get_object should succeed");
        let body = payload
            .body
            .collect()
            .await
            .expect("slow telecom body should keep streaming");

        assert_eq!(body.as_ref(), b"abcdefghi");
        assert!(payload.first_response_latency_ms.is_some());
    }

    #[tokio::test]
    async fn get_object_times_out_when_streaming_download_makes_no_progress() {
        let server =
            SlowDownloadServer::start(vec![(0, b"abc".to_vec()), (1_200, b"def".to_vec())]).await;
        let adapter = mock_telecom_adapter_with_timeout(
            &server.base_url,
            "browser-id-123",
            "JSESSIONID=abc; COOKIE_LOGIN_USER=def",
            None,
            1,
        );

        let payload = adapter
            .get_object(TELECOM_ROOT_CONTAINER, "slow.bin")
            .await
            .expect("slow telecom get_object should return headers");
        let error = payload
            .body
            .collect()
            .await
            .expect_err("stalled telecom body should time out");

        assert!(
            error
                .to_string()
                .contains("timed out while reading response body after 1s without progress")
        );
    }

    #[tokio::test]
    async fn put_object_streams_to_spool_and_uploads_telecom_parts() {
        let server = MockUploadServer::start().await;
        let spool_stats = Arc::new(Mutex::new(MockSpoolStats::default()));
        let adapter = mock_upload_adapter(&server.base_url, Arc::clone(&spool_stats));

        let result = adapter
            .put_object(PutObjectRequest {
                container: TELECOM_ROOT_CONTAINER.to_string(),
                key: "docs/probe.bin".to_string(),
                body: ObjectBody::from_stream(futures_util::stream::iter([
                    Ok(bytes::Bytes::from_static(b"hello ")),
                    Ok(bytes::Bytes::from_static(b"world")),
                ])),
                size: Some(11),
                content_type: Some("application/octet-stream".to_string()),
                preferred_upload_part_size_bytes: None,
            })
            .await
            .expect("telecom put_object should succeed");

        assert_eq!(
            result.etag.as_deref(),
            Some("5EB63BBBE01EEED093CB22BB8F5ACDC3")
        );
        assert!(result.first_response_latency_ms.is_some());

        let created_folders = server
            .created_folders
            .lock()
            .expect("created folders poisoned")
            .clone();
        let folder_names = created_folders
            .iter()
            .filter_map(|fields| fields.get("folderName").cloned())
            .collect::<Vec<_>>();
        assert_eq!(folder_names, vec!["ccbg-tests", "docs"]);

        let mut uploaded_parts = server
            .uploaded_parts
            .lock()
            .expect("uploaded parts poisoned")
            .clone();
        uploaded_parts.sort_by_key(|(part_number, _, _)| *part_number);
        let uploaded_bodies = uploaded_parts
            .iter()
            .map(|(_, body, _)| body.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            uploaded_bodies,
            vec![b"hello".to_vec(), b" worl".to_vec(), b"d".to_vec()]
        );
        let uploaded_md5_headers = uploaded_parts
            .iter()
            .map(|(_, _, content_md5)| content_md5.clone().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(uploaded_md5_headers, sample_upload_part_md5_base64s());

        let control_headers = server
            .control_headers
            .lock()
            .expect("control headers poisoned")
            .clone();
        assert!(control_headers.iter().all(|headers| {
            headers.get("SessionKey") == Some(&"telecom-session-key".to_string())
                && headers.contains_key("Signature")
                && headers.contains_key("X-Request-Date")
                && headers.contains_key("X-Request-ID")
                && headers.get("PkId") == Some(&"pk-test-1".to_string())
                && headers.contains_key("EncryptionText")
        }));

        let spool_stats = spool_stats.lock().expect("spool stats poisoned");
        assert_eq!(spool_stats.active_files, 0);
        assert_eq!(spool_stats.active_bytes, 0);
        assert_eq!(spool_stats.peak_files, 1);
        assert_eq!(spool_stats.peak_bytes, 11);
    }
}
