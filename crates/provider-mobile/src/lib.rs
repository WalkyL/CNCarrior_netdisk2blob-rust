use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
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
use futures_util::StreamExt;
use md5::Md5;
use reqwest::{
    Method,
    header::{
        ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, HeaderName, HeaderValue, ORIGIN, REFERER,
        USER_AGENT,
    },
};
use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::{Builder as TempFileBuilder, NamedTempFile};
use tokio::{
    fs::File as TokioFile,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
};

const MOBILE_ROOT_CONTAINER: &str = "root";
const MOBILE_NATIVE_CAPABILITY_SCHEMA_VERSION: u32 = 1;
const DEFAULT_MOBILE_UPLOAD_MAX_PART_COUNT: u64 = 41;
const MOBILE_UPLOAD_STREAM_CHUNK_BYTES: usize = 1024 * 1024;
const DEFAULT_MOBILE_NATIVE_CAPABILITY_CATALOG_JSON: &str =
    include_str!("../../../config/provider-capabilities/mobile-native.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileConfig {
    pub base_url: String,
    pub token_source: TokenSource,
    pub outbound_ip_family: OutboundIpFamily,
    pub cookie_header: Option<String>,
    pub user_agent: String,
    pub browser_profile: Option<BrowserRequestProfile>,
    pub request_timeout_secs: u64,
    pub list_url: String,
    pub personal_disk_info_url: String,
    pub family_disk_info_url: String,
    pub root_folder_id: Option<String>,
    pub user_domain_id: Option<String>,
    pub page_size: usize,
    pub root_prefix: Option<String>,
    pub upload_part_size_bytes: u64,
    pub upload_max_part_count: u64,
    #[serde(default)]
    pub max_single_upload_bytes: Option<u64>,
    #[serde(default)]
    pub max_single_download_bytes: Option<u64>,
    pub body_spool_dir: Option<String>,
    #[serde(skip, default)]
    pub body_spool_observer: Option<SharedBodySpoolObserver>,
    pub native_capability_catalog_path: Option<String>,
}

pub struct MobileBlobAdapter {
    config: MobileConfig,
    client: reqwest::Client,
    native_capabilities: BTreeMap<String, MobileNativeCapabilitySpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobileNativeCapabilityCatalog {
    schema_version: u32,
    provider: String,
    #[serde(rename = "description", default)]
    _description: Option<String>,
    capabilities: Vec<MobileNativeCapabilitySpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobileNativeCapabilitySpec {
    id: String,
    method: String,
    url: String,
    #[serde(default)]
    signature_strategy: Option<String>,
    #[serde(default)]
    body_defaults: Map<String, Value>,
    #[serde(rename = "notes", default)]
    _notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MobileFileCreateResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<MobileFileCreateData>,
}

#[derive(Debug, Deserialize)]
struct MobileFileCreateData {
    #[serde(rename = "parentFileId", default)]
    _parent_file_id: Option<String>,
    #[serde(rename = "fileId", default)]
    file_id: Option<String>,
    #[serde(rename = "fileName", default)]
    _file_name: Option<String>,
    #[serde(rename = "uploadId", default)]
    upload_id: Option<String>,
    #[serde(rename = "rapidUpload", default)]
    rapid_upload: Option<bool>,
    #[serde(
        rename = "partInfos",
        default,
        deserialize_with = "deserialize_vec_or_null"
    )]
    part_infos: Vec<MobileUploadPartInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobileUploadPartInfo {
    #[serde(rename = "partNumber", default)]
    _part_number: Option<u32>,
    #[serde(rename = "partSize", default)]
    part_size: Option<u64>,
    #[serde(rename = "uploadUrl", default)]
    upload_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobileFileCompleteResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<MobileFileCompleteData>,
}

#[derive(Debug, Deserialize)]
struct MobileFileCompleteData {
    #[serde(rename = "fileId", default)]
    _file_id: Option<String>,
    #[serde(rename = "contentHash", default)]
    content_hash: Option<String>,
    #[serde(rename = "fileName", default)]
    _file_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobileFileDownloadUrlResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<MobileFileDownloadUrlData>,
}

#[derive(Debug, Deserialize)]
struct MobileFileDownloadUrlData {
    #[serde(rename = "fileId", default)]
    file_id: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    size: Option<u64>,
    #[serde(rename = "contentHash", default)]
    content_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobileMetadataActionResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug)]
struct PreparedMobileUpload {
    spool_file: NamedTempFile,
    size: u64,
    content_hash: String,
    part_size_bytes: u64,
    _spool_lease: Option<Box<dyn BodySpoolLease>>,
}

#[derive(Debug)]
struct MobileUploadPlan {
    file_id: String,
    upload_id: Option<String>,
    rapid_upload: bool,
    part_infos: Vec<MobileUploadPartInfo>,
}

#[derive(Debug)]
struct MobileFolderPlan {
    file_id: String,
}

#[derive(Debug)]
struct TimedObjectBody {
    body: ObjectBody,
    first_response_latency_ms: u64,
}

#[derive(Debug, Deserialize)]
struct MobileListResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<MobileListData>,
}

#[derive(Debug, Deserialize)]
struct MobileListData {
    #[serde(default)]
    items: Vec<MobileListItem>,
    #[serde(rename = "nextPageCursor", default)]
    next_page_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MobileListItem {
    #[serde(rename = "fileId", default)]
    file_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "type", default)]
    item_type: Option<String>,
    #[serde(rename = "fileExtension", default)]
    file_extension: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_like")]
    size: Option<u64>,
    #[serde(rename = "createdAt", default)]
    created_at: Option<String>,
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MobileDiskInfoResponse {
    success: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<MobileDiskInfoData>,
}

#[derive(Debug, Deserialize)]
struct MobileDiskInfoData {
    #[serde(
        rename = "freeDiskSize",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    free_disk_size: Option<u64>,
    #[serde(
        rename = "diskSize",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    disk_size: Option<u64>,
    #[serde(
        rename = "usedSize",
        default,
        deserialize_with = "deserialize_optional_u64_like"
    )]
    used_size: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobileScope {
    Personal,
    Family,
}

impl MobileBlobAdapter {
    pub fn new(config: MobileConfig) -> Result<Self, BlobError> {
        Ok(Self {
            client: build_http_client(&config)?,
            native_capabilities: load_mobile_native_capabilities(
                config.native_capability_catalog_path.as_deref(),
            )?,
            config,
        })
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(self.config.request_timeout_secs.max(1))
    }

    fn page_size(&self) -> usize {
        self.config.page_size.max(1)
    }

    fn upload_part_size_bytes(&self) -> u64 {
        self.config.upload_part_size_bytes.max(1)
    }

    fn upload_max_part_count(&self) -> u64 {
        if self.config.upload_max_part_count == 0 {
            DEFAULT_MOBILE_UPLOAD_MAX_PART_COUNT
        } else {
            self.config.upload_max_part_count
        }
    }

    fn effective_upload_part_size_bytes(&self, size: u64) -> u64 {
        let configured = self.upload_part_size_bytes();
        if size == 0 {
            return configured;
        }
        let min_part_size_for_count = size.div_ceil(self.upload_max_part_count());
        configured.max(min_part_size_for_count)
    }

    fn normalized_root_prefix(&self) -> Option<String> {
        self.config
            .root_prefix
            .as_deref()
            .map(normalize_object_key)
            .filter(|value| !value.is_empty())
    }

    fn managed_object_key(&self, container: &str, key: &str) -> Result<String, BlobError> {
        ensure_non_empty(container, "container")?;
        ensure_non_empty(key, "object key")?;

        let mut parts = Vec::new();
        if let Some(prefix) = self.normalized_root_prefix() {
            parts.push(prefix);
        }
        parts.push(normalize_object_key(container));
        parts.push(normalize_object_key(key));

        Ok(parts.join("/"))
    }

    fn managed_container_root(&self, container: &str) -> Result<String, BlobError> {
        ensure_non_empty(container, "container")?;

        let mut parts = Vec::new();
        if let Some(prefix) = self.normalized_root_prefix() {
            parts.push(prefix);
        }
        parts.push(normalize_object_key(container));

        Ok(parts.join("/"))
    }

    fn native_capability(&self, id: &str) -> Result<&MobileNativeCapabilitySpec, BlobError> {
        self.native_capabilities.get(id).ok_or_else(|| {
            BlobError::Configuration(format!(
                "China Mobile native capability is not configured: {id}"
            ))
        })
    }

    fn base_headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, BlobError> {
        let mut headers = Vec::new();
        headers.push((
            USER_AGENT,
            HeaderValue::from_str(self.effective_user_agent()).map_err(|error| {
                BlobError::Configuration(format!("invalid China Mobile user agent: {error}"))
            })?,
        ));
        headers.push((
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        ));
        headers.push((
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=UTF-8"),
        ));
        headers.push((
            AUTHORIZATION,
            HeaderValue::from_str(self.authorization_header()?.as_str()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid China Mobile Authorization header: {error}"
                ))
            })?,
        ));

        if let Some(cookie_header) = self
            .config
            .cookie_header
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            headers.push((
                COOKIE,
                HeaderValue::from_str(cookie_header).map_err(|error| {
                    BlobError::Configuration(format!("invalid China Mobile Cookie Header: {error}"))
                })?,
            ));
        }

        let origin = self
            .profile_header("origin")
            .unwrap_or(self.config.base_url.trim_end_matches('/'));
        headers.push((
            ORIGIN,
            HeaderValue::from_str(origin).map_err(|error| {
                BlobError::Configuration(format!("invalid China Mobile Origin header: {error}"))
            })?,
        ));

        let referer = self
            .profile_header("referer")
            .unwrap_or("https://yun.139.com/");
        headers.push((
            REFERER,
            HeaderValue::from_str(referer).map_err(|error| {
                BlobError::Configuration(format!("invalid China Mobile Referer header: {error}"))
            })?,
        ));

        if let Some(profile) = self.config.browser_profile.as_ref() {
            for (name, value) in profile.forwarded_headers(&[
                "accept",
                "authorization",
                "content-type",
                "cookie",
                "origin",
                "referer",
                "user-agent",
                "mcloud-sign",
            ]) {
                headers.push((
                    HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                        BlobError::Configuration(format!(
                            "invalid forwarded China Mobile browser profile header name {name}: {error}"
                        ))
                    })?,
                    HeaderValue::from_str(value.as_str()).map_err(|error| {
                        BlobError::Configuration(format!(
                            "invalid forwarded China Mobile browser profile header {name}: {error}"
                        ))
                    })?,
                ));
            }
        }

        Ok(headers)
    }

    fn request_with_headers(
        &self,
        method: Method,
        url: &str,
        body: Option<&[u8]>,
        signature_strategy: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, BlobError> {
        let mut request = self.client.request(method, url).timeout(self.timeout());
        for (name, value) in self.base_headers()? {
            request = request.header(name, value);
        }
        if let Some(strategy) = signature_strategy {
            let signature = self.signature_header(strategy, body.unwrap_or(&[]))?;
            request = request.header("mcloud-sign", signature);
        }
        Ok(request)
    }

    fn signature_header(&self, strategy: &str, body: &[u8]) -> Result<String, BlobError> {
        match strategy.trim() {
            "mcloud_md5_v1" => Ok(build_mcloud_md5_v1_signature(body)),
            other => Err(BlobError::Configuration(format!(
                "unsupported China Mobile signature strategy: {other}"
            ))),
        }
    }

    async fn send_capability_json<T: for<'de> Deserialize<'de>>(
        &self,
        capability_id: &str,
        body: &Value,
        operation: &str,
    ) -> Result<T, BlobError> {
        let capability = self.native_capability(capability_id)?;
        let encoded = serde_json::to_vec(body).map_err(|error| {
            BlobError::Configuration(format!(
                "failed to encode China Mobile request body for {capability_id}: {error}"
            ))
        })?;
        let response = self
            .request_with_headers(
                Method::POST,
                capability.url.as_str(),
                Some(encoded.as_slice()),
                capability.signature_strategy.as_deref(),
            )?
            .body(encoded)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{operation} failed: {error}")))?;
        decode_success_json(response, operation).await
    }

    fn create_upload_spool_file(&self) -> Result<NamedTempFile, BlobError> {
        let mut builder = TempFileBuilder::new();
        builder.prefix("ccbg-mobile-upload-").suffix(".spool");
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
                        "failed to create China Mobile upload spool directory {path}: {error}"
                    ))
                })?;
                builder.tempfile_in(Path::new(path)).map_err(|error| {
                    BlobError::Upstream(format!(
                        "failed to create China Mobile upload spool file in {path}: {error}"
                    ))
                })
            }
            None => builder.tempfile().map_err(|error| {
                BlobError::Upstream(format!(
                    "failed to create China Mobile upload spool file in system temp directory: {error}"
                ))
            }),
        }
    }

    async fn prepare_upload_body(
        &self,
        body: ObjectBody,
        declared_size: Option<u64>,
        preferred_part_size_bytes: Option<u64>,
    ) -> Result<PreparedMobileUpload, BlobError> {
        let spool_file = self.create_upload_spool_file()?;
        let mut spool_lease = self
            .config
            .body_spool_observer
            .as_ref()
            .map(|observer| observer.start_tracking());
        let async_file = spool_file.reopen().map_err(|error| {
            BlobError::Upstream(format!(
                "failed to reopen China Mobile upload spool file for writing: {error}"
            ))
        })?;
        let mut file = TokioFile::from_std(async_file);
        let mut stream = body.into_stream();
        let mut total_len = 0u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            total_len = total_len.saturating_add(chunk.len() as u64);
            hasher.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|error| BlobError::BodyStream(error.to_string()))?;
            if let Some(lease) = spool_lease.as_mut() {
                lease.update_tracked_bytes(total_len);
            }
        }
        if let Some(expected) = declared_size {
            if expected != total_len {
                return Err(BlobError::BodyStream(format!(
                    "object body size mismatch: declared {expected} bytes, received {total_len}"
                )));
            }
        }
        file.flush()
            .await
            .map_err(|error| BlobError::BodyStream(error.to_string()))?;
        let part_size_bytes = match preferred_part_size_bytes.filter(|value| *value > 0) {
            Some(preferred) if total_len == 0 => preferred,
            Some(preferred) => preferred.max(total_len.div_ceil(self.upload_max_part_count())),
            None => self.effective_upload_part_size_bytes(total_len),
        }
        .max(1);
        Ok(PreparedMobileUpload {
            spool_file,
            size: total_len,
            content_hash: hex::encode(hasher.finalize()),
            part_size_bytes,
            _spool_lease: spool_lease,
        })
    }

    fn upload_part_infos(size: u64, part_size_bytes: u64) -> Vec<Value> {
        let part_size_bytes = part_size_bytes.max(1);
        if size == 0 {
            return vec![json!({
                "parallelHashCtx": {
                    "partOffset": 0
                },
                "partNumber": 1,
                "partSize": 0
            })];
        }

        let mut part_infos = Vec::new();
        let mut part_offset = 0u64;
        let mut part_number = 1u64;
        while part_offset < size {
            let current_size = (size - part_offset).min(part_size_bytes);
            part_infos.push(json!({
                "parallelHashCtx": {
                    "partOffset": part_offset
                },
                "partNumber": part_number,
                "partSize": current_size
            }));
            part_offset = part_offset.saturating_add(current_size);
            part_number = part_number.saturating_add(1);
        }
        part_infos
    }

    fn build_file_create_body(
        &self,
        capability: &MobileNativeCapabilitySpec,
        file_name: &str,
        parent_file_id: &str,
        content_type: &str,
        upload: &PreparedMobileUpload,
    ) -> Value {
        let mut body = capability.body_defaults.clone();
        body.insert("name".to_string(), Value::String(file_name.to_string()));
        body.insert("size".to_string(), Value::Number(upload.size.into()));
        body.insert(
            "contentType".to_string(),
            Value::String(content_type.to_string()),
        );
        body.insert(
            "contentHash".to_string(),
            Value::String(upload.content_hash.clone()),
        );
        body.insert(
            "partInfos".to_string(),
            Value::Array(Self::upload_part_infos(upload.size, upload.part_size_bytes)),
        );
        body.insert(
            "parentFileId".to_string(),
            Value::String(parent_file_id.to_string()),
        );
        body.insert(
            "commonAccountInfo".to_string(),
            json!({
                "account": mobile_account_from_authorization(self.authorization_header().ok().as_deref()),
                "accountType": 1
            }),
        );
        Value::Object(body)
    }

    fn build_folder_create_body(
        &self,
        capability: &MobileNativeCapabilitySpec,
        folder_name: &str,
        parent_file_id: &str,
    ) -> Value {
        let mut body = capability.body_defaults.clone();
        body.insert("type".to_string(), Value::String("folder".to_string()));
        body.insert("name".to_string(), Value::String(folder_name.to_string()));
        body.insert(
            "parentFileId".to_string(),
            Value::String(parent_file_id.to_string()),
        );
        body.insert(
            "commonAccountInfo".to_string(),
            json!({
                "account": mobile_account_from_authorization(self.authorization_header().ok().as_deref()),
                "accountType": 1
            }),
        );
        body.remove("fileRenameMode");
        body.remove("contentHashAlgorithm");
        Value::Object(body)
    }

    async fn create_mobile_upload(
        &self,
        file_name: &str,
        parent_file_id: &str,
        content_type: &str,
        upload: &PreparedMobileUpload,
    ) -> Result<MobileUploadPlan, BlobError> {
        let capability = self.native_capability("file_create")?;
        let body = self.build_file_create_body(
            capability,
            file_name,
            parent_file_id,
            content_type,
            upload,
        );
        let response = self
            .send_capability_json::<MobileFileCreateResponse>(
                "file_create",
                &body,
                "China Mobile file/create",
            )
            .await?;
        if !response.success {
            return Err(BlobError::Upstream(format!(
                "China Mobile file/create rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }
        let data = response.data.ok_or_else(|| {
            BlobError::Upstream("China Mobile file/create returned no data payload".to_string())
        })?;
        Ok(MobileUploadPlan {
            file_id: ensure_present_string(data.file_id, "China Mobile file/create fileId")?,
            rapid_upload: data.rapid_upload.unwrap_or(false),
            upload_id: if data.rapid_upload.unwrap_or(false) {
                data.upload_id
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            } else {
                Some(ensure_present_string(
                    data.upload_id,
                    "China Mobile file/create uploadId",
                )?)
            },
            part_infos: data.part_infos,
        })
    }

    async fn create_mobile_folder(
        &self,
        folder_name: &str,
        parent_file_id: &str,
    ) -> Result<MobileFolderPlan, BlobError> {
        let capability = self.native_capability("file_create")?;
        let body = self.build_folder_create_body(capability, folder_name, parent_file_id);
        let response = self
            .send_capability_json::<MobileFileCreateResponse>(
                "file_create",
                &body,
                "China Mobile folder create",
            )
            .await?;
        if !response.success {
            return Err(BlobError::Upstream(format!(
                "China Mobile folder create rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }
        let data = response.data.ok_or_else(|| {
            BlobError::Upstream("China Mobile folder create returned no data payload".to_string())
        })?;
        Ok(MobileFolderPlan {
            file_id: ensure_present_string(data.file_id, "China Mobile folder create fileId")?,
        })
    }

    async fn upload_mobile_parts(
        &self,
        upload_started_at: Instant,
        upload: &PreparedMobileUpload,
        plan: &MobileUploadPlan,
    ) -> Result<Option<u64>, BlobError> {
        if plan.rapid_upload {
            return Ok(None);
        }
        if plan.part_infos.is_empty() && upload.size > 0 {
            return Err(BlobError::Upstream(
                "China Mobile upload plan returned no part upload instructions".to_string(),
            ));
        }

        let mut part_offset = 0u64;
        let first_upload_progress_ms = Arc::new(AtomicU64::new(0));

        for (index, part) in plan.part_infos.iter().enumerate() {
            let upload_url = ensure_present_string(
                part.upload_url.clone(),
                "China Mobile upload part uploadUrl",
            )?;
            let part_number = part._part_number.unwrap_or((index + 1) as u32);
            let part_size = part.part_size.unwrap_or_else(|| {
                upload
                    .size
                    .saturating_sub(part_offset)
                    .min(upload.part_size_bytes)
            });
            let async_file = upload.spool_file.reopen().map_err(|error| {
                BlobError::Upstream(format!(
                    "failed to reopen China Mobile upload spool file for part {part_number}: {error}"
                ))
            })?;
            let mut file = TokioFile::from_std(async_file);
            if part_size > 0 {
                file.seek(SeekFrom::Start(part_offset))
                    .await
                    .map_err(|error| {
                        BlobError::Upstream(format!(
                            "failed to seek China Mobile upload spool file for part {part_number}: {error}"
                        ))
                    })?;
            }
            let part_body = if part_size == 0 {
                reqwest::Body::from(Vec::<u8>::new())
            } else {
                let progress_observer = StreamFirstProgressObserver::new({
                    let first_upload_progress_ms = Arc::clone(&first_upload_progress_ms);
                    move || {
                        first_upload_progress_ms
                            .store(elapsed_millis(upload_started_at).max(1), Ordering::SeqCst);
                    }
                });
                reqwest::Body::wrap_stream(futures_util::stream::try_unfold(
                    (file, part_size, progress_observer),
                    move |(mut file, remaining, progress_observer)| async move {
                        if remaining == 0 {
                            return Ok(None);
                        }
                        let next_len =
                            remaining.min(MOBILE_UPLOAD_STREAM_CHUNK_BYTES as u64) as usize;
                        let mut chunk = vec![0u8; next_len];
                        let read = file.read(&mut chunk).await?;
                        if read == 0 {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                format!(
                                    "China Mobile upload spool file ended early while streaming part {part_number}"
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
            let response = self
                .client
                .request(Method::PUT, upload_url.as_str())
                .header("content-length", part_size.to_string())
                .body(part_body)
                .timeout(self.timeout())
                .send()
                .await
                .map_err(|error| {
                    BlobError::Upstream(format!("China Mobile part upload failed: {error}"))
                })?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|error| format!("<failed to read body: {error}>"));
                return Err(BlobError::Upstream(format!(
                    "China Mobile part upload failed with HTTP {status}: {body}"
                )));
            }
            part_offset = part_offset.saturating_add(part_size);
        }

        if part_offset < upload.size {
            return Err(BlobError::Upstream(format!(
                "China Mobile upload plan covered only {part_offset} of {} bytes",
                upload.size
            )));
        }
        Ok(match first_upload_progress_ms.load(Ordering::SeqCst) {
            0 => None,
            value => Some(value),
        })
    }

    async fn complete_mobile_upload(
        &self,
        plan: &MobileUploadPlan,
        upload: &PreparedMobileUpload,
    ) -> Result<MobileFileCompleteData, BlobError> {
        let upload_id = plan.upload_id.clone().ok_or_else(|| {
            BlobError::Upstream(
                "China Mobile file/complete requires an uploadId for non-rapid uploads".to_string(),
            )
        })?;
        let mut body = self
            .native_capability("file_complete")?
            .body_defaults
            .clone();
        body.insert("fileId".to_string(), Value::String(plan.file_id.clone()));
        body.insert("uploadId".to_string(), Value::String(upload_id));
        body.insert(
            "contentHash".to_string(),
            Value::String(upload.content_hash.clone()),
        );
        body.insert(
            "contentHashAlgorithm".to_string(),
            Value::String("SHA256".to_string()),
        );
        let response = self
            .send_capability_json::<MobileFileCompleteResponse>(
                "file_complete",
                &Value::Object(body),
                "China Mobile file/complete",
            )
            .await?;
        if !response.success {
            return Err(BlobError::Upstream(format!(
                "China Mobile file/complete rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }
        response.data.ok_or_else(|| {
            BlobError::Upstream("China Mobile file/complete returned no data payload".to_string())
        })
    }

    async fn find_child_folder_id(
        &self,
        parent_file_id: &str,
        folder_name: &str,
    ) -> Result<Option<String>, BlobError> {
        let expected_name = folder_name.trim();
        if expected_name.is_empty() {
            return Ok(None);
        }

        let mut page_cursor = None;
        loop {
            let page = self
                .list_page(parent_file_id, page_cursor.as_deref())
                .await?;
            for item in page.items {
                if item.is_folder() && item.display_name() == Some(expected_name) {
                    return Ok(Some(
                        item.file_id()
                            .ok_or_else(|| {
                                BlobError::Upstream(
                                    "China Mobile file/list returned a folder without an id"
                                        .to_string(),
                                )
                            })?
                            .to_string(),
                    ));
                }
            }
            match page
                .next_page_cursor
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(cursor) => page_cursor = Some(cursor.to_string()),
                None => return Ok(None),
            }
        }
    }

    async fn ensure_folder(
        &self,
        parent_file_id: &str,
        folder_name: &str,
    ) -> Result<String, BlobError> {
        if let Some(existing) = self
            .find_child_folder_id(parent_file_id, folder_name)
            .await?
        {
            return Ok(existing);
        }

        let created = self
            .create_mobile_folder(folder_name, parent_file_id)
            .await?;
        Ok(created.file_id)
    }

    async fn resolve_child_directory_path_if_exists(
        &self,
        parent_file_id: &str,
        directory_path: &str,
    ) -> Result<Option<String>, BlobError> {
        let normalized = normalize_object_key(directory_path);
        if normalized.is_empty() {
            return Ok(Some(parent_file_id.to_string()));
        }

        let mut current_id = parent_file_id.to_string();
        for segment in normalized
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
        {
            let Some(child_id) = self
                .find_child_folder_id(current_id.as_str(), segment)
                .await?
            else {
                return Ok(None);
            };
            current_id = child_id;
        }

        Ok(Some(current_id))
    }

    async fn resolve_directory_path_if_exists(
        &self,
        directory_path: &str,
    ) -> Result<Option<String>, BlobError> {
        self.resolve_child_directory_path_if_exists(self.root_folder_id()?, directory_path)
            .await
    }

    async fn resolve_container_root_folder_id(
        &self,
        container: &str,
    ) -> Result<Option<String>, BlobError> {
        let managed_root = self.managed_container_root(container)?;
        self.resolve_directory_path_if_exists(managed_root.as_str())
            .await
    }

    async fn resolve_object_entry(
        &self,
        container: &str,
        key: &str,
    ) -> Result<(MobileListItem, String), BlobError> {
        let normalized_key = normalize_object_key(key);
        ensure_non_empty(normalized_key.as_str(), "object key")?;

        let Some(container_root_id) = self.resolve_container_root_folder_id(container).await?
        else {
            return Err(BlobError::NotFound(format!(
                "object not found: {container}/{normalized_key}"
            )));
        };

        let (parent_path, file_name) = match normalized_key.rsplit_once('/') {
            Some((parent_path, file_name)) => (Some(parent_path), file_name),
            None => (None, normalized_key.as_str()),
        };

        let parent_id = match parent_path {
            Some(path) => self
                .resolve_child_directory_path_if_exists(container_root_id.as_str(), path)
                .await?
                .ok_or_else(|| {
                    BlobError::NotFound(format!("object not found: {container}/{normalized_key}"))
                })?,
            None => container_root_id,
        };

        let mut page_cursor = None;
        loop {
            let page = self
                .list_page(parent_id.as_str(), page_cursor.as_deref())
                .await?;
            if let Some(entry) = page
                .items
                .into_iter()
                .find(|item| !item.is_folder() && item.display_name() == Some(file_name))
            {
                return Ok((entry, normalized_key));
            }

            match page
                .next_page_cursor
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(cursor) => page_cursor = Some(cursor.to_string()),
                None => {
                    return Err(BlobError::NotFound(format!(
                        "object not found: {container}/{normalized_key}"
                    )));
                }
            }
        }
    }

    async fn ensure_managed_parent_folder_id(
        &self,
        container: &str,
        key: &str,
    ) -> Result<(String, String), BlobError> {
        let managed_object_key = self.managed_object_key(container, key)?;
        let (parent_path, file_name) = match managed_object_key.rsplit_once('/') {
            Some((parent_path, file_name)) => (parent_path, file_name.to_string()),
            None => {
                return Ok((self.root_folder_id()?.to_string(), managed_object_key));
            }
        };

        let mut parent_id = self.root_folder_id()?.to_string();
        for segment in parent_path
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
        {
            parent_id = self.ensure_folder(parent_id.as_str(), segment).await?;
        }

        Ok((parent_id, file_name))
    }

    async fn download_url_for_file(&self, file_id: &str) -> Result<String, BlobError> {
        let response = self
            .send_capability_json::<MobileFileDownloadUrlResponse>(
                "file_get_download_url",
                &json!({ "fileId": file_id }),
                "China Mobile file/getDownloadUrl",
            )
            .await?;
        if !response.success {
            return Err(BlobError::Upstream(format!(
                "China Mobile file/getDownloadUrl rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }
        let data = response.data.ok_or_else(|| {
            BlobError::Upstream(
                "China Mobile file/getDownloadUrl returned no data payload".to_string(),
            )
        })?;
        let _ = data.file_id;
        let _ = data.size;
        let _ = data.content_hash;
        ensure_present_string(data.url, "China Mobile file/getDownloadUrl url")
    }

    async fn get_bytes(&self, url: &str, operation: &str) -> Result<TimedObjectBody, BlobError> {
        let request_started_at = Instant::now();
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{operation} failed: {error}")))?;
        let first_response_latency_ms = elapsed_millis(request_started_at);
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| format!("<failed to read body: {error}>"));
            return Err(BlobError::Upstream(format!(
                "{operation} failed with HTTP {status}: {body}"
            )));
        }

        let operation = operation.to_string();
        Ok(TimedObjectBody {
            body: ObjectBody::from_stream(futures_util::stream::try_unfold(
                response,
                move |mut response| {
                    let operation = operation.clone();
                    async move {
                        let chunk = response.chunk().await.map_err(|error| {
                            BlobError::Upstream(format!(
                                "{operation} returned invalid bytes: {error}"
                            ))
                        })?;
                        Ok(chunk.map(|chunk| (chunk, response)))
                    }
                },
            )),
            first_response_latency_ms,
        })
    }

    async fn update_file_name(&self, file_id: &str, new_name: &str) -> Result<(), BlobError> {
        ensure_non_empty(file_id, "China Mobile file id")?;
        ensure_non_empty(new_name, "China Mobile target file name")?;

        let mut body = self.native_capability("file_update")?.body_defaults.clone();
        body.insert("fileId".to_string(), Value::String(file_id.to_string()));
        body.insert("name".to_string(), Value::String(new_name.to_string()));
        self.send_metadata_action(
            "file_update",
            Value::Object(body),
            "China Mobile file/update",
        )
        .await
    }

    async fn delete_file_id(&self, file_id: &str) -> Result<(), BlobError> {
        ensure_non_empty(file_id, "China Mobile file id")?;
        self.send_metadata_action(
            "file_batch_delete",
            json!({ "fileIds": [file_id] }),
            "China Mobile file/batchDelete",
        )
        .await
    }

    async fn move_file_id(
        &self,
        file_id: &str,
        target_parent_file_id: &str,
    ) -> Result<(), BlobError> {
        ensure_non_empty(file_id, "China Mobile file id")?;
        ensure_non_empty(target_parent_file_id, "China Mobile target parent file id")?;
        self.send_metadata_action(
            "file_batch_move",
            json!({
                "fileIds": [file_id],
                "toParentFileId": target_parent_file_id,
            }),
            "China Mobile file/batchMove",
        )
        .await
    }

    async fn send_metadata_action(
        &self,
        capability_id: &str,
        body: Value,
        operation: &str,
    ) -> Result<(), BlobError> {
        let response = self
            .send_capability_json::<MobileMetadataActionResponse>(capability_id, &body, operation)
            .await?;
        if response.success {
            return Ok(());
        }
        Err(BlobError::Upstream(format!(
            "{operation} rejected the request: code={} message={}",
            response.code.unwrap_or_else(|| "unknown".to_string()),
            response.message.unwrap_or_else(|| "unknown".to_string())
        )))
    }

    fn root_folder_id(&self) -> Result<&str, BlobError> {
        self.config
            .root_folder_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "missing China Mobile Root Folder ID; capture the current Mobile session from the logged-in file-list page".to_string(),
                )
            })
    }

    fn user_domain_id(&self) -> Result<&str, BlobError> {
        self.config
            .user_domain_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                BlobError::Configuration(
                    "missing China Mobile User Domain ID; capture the current Mobile session from the logged-in file-list page".to_string(),
                )
            })
    }

    fn authorization_header(&self) -> Result<String, BlobError> {
        let token = self.config.token_source.load().map_err(|error| {
            BlobError::Configuration(format!(
                "missing China Mobile Authorization header: {error}"
            ))
        })?;
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(BlobError::Configuration(
                "missing China Mobile Authorization header".to_string(),
            ));
        }
        Ok(trimmed.to_string())
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

    fn request(&self, url: &str) -> Result<reqwest::RequestBuilder, BlobError> {
        let mut request = self
            .client
            .request(Method::POST, url)
            .header(USER_AGENT, self.effective_user_agent())
            .header(ACCEPT, "application/json, text/plain, */*")
            .header(CONTENT_TYPE, "application/json;charset=UTF-8")
            .timeout(self.timeout());

        request = request.header(
            AUTHORIZATION,
            HeaderValue::from_str(self.authorization_header()?.as_str()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid China Mobile Authorization header: {error}"
                ))
            })?,
        );

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
                    BlobError::Configuration(format!("invalid China Mobile Cookie Header: {error}"))
                })?,
            );
        }

        let origin = self
            .profile_header("origin")
            .unwrap_or(self.config.base_url.trim_end_matches('/'));
        request = request.header(
            ORIGIN,
            HeaderValue::from_str(origin).map_err(|error| {
                BlobError::Configuration(format!("invalid China Mobile Origin header: {error}"))
            })?,
        );

        let referer = self
            .profile_header("referer")
            .unwrap_or("https://yun.139.com/");
        request = request.header(
            REFERER,
            HeaderValue::from_str(referer).map_err(|error| {
                BlobError::Configuration(format!("invalid China Mobile Referer header: {error}"))
            })?,
        );

        self.apply_browser_profile_headers(request)
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
            "authorization",
            "content-type",
            "cookie",
            "origin",
            "referer",
            "user-agent",
        ]) {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid forwarded China Mobile browser profile header name {name}: {error}"
                ))
            })?;
            let header_value = HeaderValue::from_str(value.as_str()).map_err(|error| {
                BlobError::Configuration(format!(
                    "invalid forwarded China Mobile browser profile header {name}: {error}"
                ))
            })?;
            request = request.header(header_name, header_value);
        }
        Ok(request)
    }

    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: Value,
        operation: &str,
    ) -> Result<T, BlobError> {
        let response = self
            .request(url)?
            .json(&body)
            .send()
            .await
            .map_err(|error| BlobError::Upstream(format!("{operation} failed: {error}")))?;
        decode_success_json(response, operation).await
    }

    async fn list_page(
        &self,
        parent_file_id: &str,
        page_cursor: Option<&str>,
    ) -> Result<MobileListData, BlobError> {
        let capability = self.native_capability("file_list")?;
        let mut body = capability.body_defaults.clone();
        body.insert(
            "pageInfo".to_string(),
            json!({
                "pageSize": self.page_size(),
                "pageCursor": page_cursor,
            }),
        );
        body.insert(
            "parentFileId".to_string(),
            Value::String(parent_file_id.to_string()),
        );
        let response = self
            .send_capability_json::<MobileListResponse>(
                "file_list",
                &Value::Object(body),
                "China Mobile file/list",
            )
            .await?;

        if !response.success {
            return Err(BlobError::Upstream(format!(
                "China Mobile file/list rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }

        response.data.ok_or_else(|| {
            BlobError::Upstream("China Mobile file/list returned no data payload".to_string())
        })
    }

    async fn disk_info(&self, scope: MobileScope) -> Result<MobileDiskInfoData, BlobError> {
        let url = match scope {
            MobileScope::Personal => self.config.personal_disk_info_url.as_str(),
            MobileScope::Family => self.config.family_disk_info_url.as_str(),
        };
        let operation = match scope {
            MobileScope::Personal => "China Mobile getPersonalDiskInfo",
            MobileScope::Family => "China Mobile getFamilyDiskInfo",
        };
        let response = self
            .send_json::<MobileDiskInfoResponse>(
                url,
                json!({
                    "userDomainId": self.user_domain_id()?,
                }),
                operation,
            )
            .await?;

        if !response.success {
            return Err(BlobError::Upstream(format!(
                "{operation} rejected the request: code={} message={}",
                response.code.unwrap_or_else(|| "unknown".to_string()),
                response.message.unwrap_or_else(|| "unknown".to_string())
            )));
        }

        response
            .data
            .ok_or_else(|| BlobError::Upstream(format!("{operation} returned no data payload")))
    }

    fn disk_capacity(data: &MobileDiskInfoData) -> Option<StorageCapacity> {
        let total = data.disk_size.map(mebibytes_to_bytes);
        let free = data.free_disk_size.map(mebibytes_to_bytes);
        let used = data
            .used_size
            .map(mebibytes_to_bytes)
            .or_else(|| match (total, free) {
                (Some(total), Some(free)) if total >= free => Some(total - free),
                _ => None,
            });

        if total.is_none() && free.is_none() && used.is_none() {
            None
        } else {
            Some(StorageCapacity {
                total_bytes: total,
                used_bytes: used,
                free_bytes: free,
            })
        }
    }

    fn personal_scope_health(
        &self,
        page: &MobileListData,
        capacity: Option<StorageCapacity>,
    ) -> StorageScopeHealth {
        StorageScopeHealth {
            id: "personal".to_string(),
            label: "Personal Cloud".to_string(),
            kind: StorageScopeKind::Personal,
            writable: true,
            root: self.config.root_folder_id.clone(),
            container: Some(MOBILE_ROOT_CONTAINER.to_string()),
            object_count: if page.next_page_cursor.is_none() {
                Some(page.items.len() as u64)
            } else {
                None
            },
            capacity,
            notes: vec![
                format!("page_size={}", self.page_size()),
                format!(
                    "managed_root={}",
                    self.normalized_root_prefix()
                        .unwrap_or_else(|| "<provider-root>".to_string())
                ),
                if page.next_page_cursor.is_some() {
                    "root_page_incomplete=true".to_string()
                } else {
                    "root_page_incomplete=false".to_string()
                },
            ],
        }
    }

    fn family_scope_health(&self, capacity: Option<StorageCapacity>) -> StorageScopeHealth {
        StorageScopeHealth {
            id: "family".to_string(),
            label: "Family Cloud".to_string(),
            kind: StorageScopeKind::Family,
            writable: false,
            root: None,
            container: None,
            object_count: None,
            capacity,
            notes: vec![
                "family quota is available".to_string(),
                "family file listing is not mapped yet".to_string(),
            ],
        }
    }

    async fn list_scope_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        let container = request
            .container
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(MOBILE_ROOT_CONTAINER);
        if container != MOBILE_ROOT_CONTAINER {
            return Err(BlobError::NotFound(format!(
                "container not found: {container}"
            )));
        }

        let normalized_prefix = request.prefix.as_deref().map(normalize_object_key);
        let Some(container_root_id) = self.resolve_container_root_folder_id(container).await?
        else {
            return Ok(Vec::new());
        };
        let mut objects = BTreeMap::new();
        let mut stack = vec![(container_root_id, String::new())];

        while let Some((folder_id, folder_prefix)) = stack.pop() {
            let mut page_cursor = None;
            loop {
                let page = self
                    .list_page(folder_id.as_str(), page_cursor.as_deref())
                    .await?;

                for item in &page.items {
                    if item.is_folder() {
                        let child_name = item.display_name().ok_or_else(|| {
                            BlobError::Upstream(
                                "China Mobile file/list returned a folder without a name"
                                    .to_string(),
                            )
                        })?;
                        let child_id = item.file_id().ok_or_else(|| {
                            BlobError::Upstream(
                                "China Mobile file/list returned a folder without an id"
                                    .to_string(),
                            )
                        })?;
                        let child_prefix = join_relative_key(&folder_prefix, child_name);
                        if normalized_prefix.as_deref().is_none_or(|prefix| {
                            directory_may_contain_prefix(&child_prefix, prefix)
                        }) {
                            stack.push((child_id.to_string(), child_prefix));
                        }
                        continue;
                    }

                    let object_key = item.object_key(&folder_prefix)?;
                    if normalized_prefix
                        .as_deref()
                        .is_none_or(|prefix| object_key.starts_with(prefix))
                    {
                        objects.insert(object_key.clone(), item.to_object_info(object_key));
                        if let Some(limit) = request.limit {
                            trim_objects_to_limit(&mut objects, limit);
                        }
                    }
                }

                if let Some(cursor) = page
                    .next_page_cursor
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    page_cursor = Some(cursor.to_string());
                    continue;
                }
                break;
            }
        }

        Ok(objects.into_values().collect())
    }
}

#[async_trait]
impl BlobBackend for MobileBlobAdapter {
    fn name(&self) -> &'static str {
        "mobile-cloud-drive"
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
        let mut notes = vec![
            format!("base_url={}", self.config.base_url),
            format!("auth_source={}", self.config.token_source.describe()),
            format!(
                "outbound_ip_family={}",
                self.config.outbound_ip_family.as_str()
            ),
            format!("list_url={}", self.config.list_url),
            format!("page_size={}", self.page_size()),
            format!(
                "managed_root={}",
                self.normalized_root_prefix()
                    .unwrap_or_else(|| "<provider-root>".to_string())
            ),
            format!("native_capability_count={}", self.native_capabilities.len()),
            format!(
                "root_folder_id_present={}",
                self.config
                    .root_folder_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
            ),
            format!(
                "user_domain_id_present={}",
                self.config
                    .user_domain_id
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
            ),
        ];
        let mut scopes = Vec::new();

        let status = match self.list_page(self.root_folder_id()?, None).await {
            Ok(page) => {
                let personal_capacity = match self.disk_info(MobileScope::Personal).await {
                    Ok(info) => Self::disk_capacity(&info),
                    Err(error) => {
                        notes.push(format!("personal_capacity_error={error}"));
                        None
                    }
                };
                scopes.push(self.personal_scope_health(&page, personal_capacity));

                match self.disk_info(MobileScope::Family).await {
                    Ok(info) => scopes.push(self.family_scope_health(Self::disk_capacity(&info))),
                    Err(error) => notes.push(format!("family_capacity_error={error}")),
                }

                if self.config.user_domain_id.is_some() {
                    HealthStatus::Healthy
                } else {
                    notes.push(
                        "remediation=capture the current Mobile session again so User Domain ID is saved for quota discovery".to_string(),
                    );
                    HealthStatus::Degraded
                }
            }
            Err(error) => {
                notes.push(error.to_string());
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
        Ok(vec![ContainerInfo {
            name: MOBILE_ROOT_CONTAINER.to_string(),
            object_count: None,
        }])
    }

    async fn list_objects(
        &self,
        request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        self.list_scope_objects(request).await
    }

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        let (entry, normalized_key) = self.resolve_object_entry(container, key).await?;
        Ok(entry.to_object_info(normalized_key))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        let (entry, normalized_key) = self.resolve_object_entry(container, key).await?;
        let file_id = entry.file_id().ok_or_else(|| {
            BlobError::Upstream("China Mobile file/list returned a file without an id".to_string())
        })?;
        let download_url = self.download_url_for_file(file_id).await?;
        let info = entry.to_object_info(normalized_key);
        let downloaded = self
            .get_bytes(&download_url, &format!("download object {container}/{key}"))
            .await?;
        Ok(ObjectPayload {
            info,
            body: downloaded.body,
            first_response_latency_ms: Some(downloaded.first_response_latency_ms),
        })
    }

    async fn put_object(&self, request: PutObjectRequest) -> Result<PutObjectResult, BlobError> {
        let (parent_file_id, file_name) = self
            .ensure_managed_parent_folder_id(&request.container, &request.key)
            .await?;
        let content_type = request
            .content_type
            .clone()
            .or_else(|| guess_content_type(file_name.as_str()).map(str::to_string))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let upload = self
            .prepare_upload_body(
                request.body,
                request.size,
                request.preferred_upload_part_size_bytes,
            )
            .await?;
        let create_started_at = Instant::now();
        let plan = self
            .create_mobile_upload(
                file_name.as_str(),
                parent_file_id.as_str(),
                content_type.as_str(),
                &upload,
            )
            .await?;
        let create_latency_ms = elapsed_millis(create_started_at);
        if plan.rapid_upload {
            return Ok(PutObjectResult {
                etag: Some(plan.file_id),
                first_response_latency_ms: Some(create_latency_ms),
            });
        }
        let upload_started_at = Instant::now();
        let first_response_latency_ms = self
            .upload_mobile_parts(upload_started_at, &upload, &plan)
            .await?
            .or(Some(elapsed_millis(upload_started_at).max(1)));
        let completed = self.complete_mobile_upload(&plan, &upload).await?;
        Ok(PutObjectResult {
            etag: completed
                .content_hash
                .or(Some(upload.content_hash.clone()))
                .or(Some(plan.file_id)),
            first_response_latency_ms,
        })
    }

    async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
        let (entry, _) = self.resolve_object_entry(container, key).await?;
        let file_id = entry.file_id().ok_or_else(|| {
            BlobError::Upstream("China Mobile file/list returned a file without an id".to_string())
        })?;
        self.delete_file_id(file_id).await
    }

    async fn rename_object(&self, request: RenameObjectRequest) -> Result<(), BlobError> {
        let (entry, _) = self
            .resolve_object_entry(&request.container, &request.key)
            .await?;
        let file_id = entry
            .file_id()
            .ok_or_else(|| {
                BlobError::Upstream(
                    "China Mobile file/list returned a file without an id".to_string(),
                )
            })?
            .to_string();
        let source_name = entry.display_name().map(str::to_string);
        let (source_parent, _) = split_parent_and_name(&request.key)?;
        let (target_parent, target_name) = split_parent_and_name(&request.new_key)?;
        if source_parent != target_parent {
            let (target_parent_file_id, _) = self
                .ensure_managed_parent_folder_id(&request.container, &request.new_key)
                .await?;
            self.move_file_id(file_id.as_str(), target_parent_file_id.as_str())
                .await?;
        }
        if source_name.as_deref() != Some(target_name.as_str()) {
            self.update_file_name(file_id.as_str(), target_name.as_str())
                .await?;
        }
        Ok(())
    }

    async fn copy_object(&self, request: CopyObjectRequest) -> Result<(), BlobError> {
        let source = self.managed_object_key(&request.source_container, &request.source_key)?;
        let destination =
            self.managed_object_key(&request.destination_container, &request.destination_key)?;
        Err(BlobError::NotImplemented(format!(
            "China Mobile native copy is not completed yet; source={source} destination={destination}"
        )))
    }

    async fn move_object(&self, request: MoveObjectRequest) -> Result<(), BlobError> {
        let (entry, _) = self
            .resolve_object_entry(&request.source_container, &request.source_key)
            .await?;
        let file_id = entry
            .file_id()
            .ok_or_else(|| {
                BlobError::Upstream(
                    "China Mobile file/list returned a file without an id".to_string(),
                )
            })?
            .to_string();
        let source_name = entry.display_name().map(str::to_string);
        let (source_parent, _) = split_parent_and_name(&request.source_key)?;
        let (target_parent, target_name) = split_parent_and_name(&request.destination_key)?;
        if request.source_container != request.destination_container
            || source_parent != target_parent
        {
            let (target_parent_file_id, _) = self
                .ensure_managed_parent_folder_id(
                    &request.destination_container,
                    &request.destination_key,
                )
                .await?;
            self.move_file_id(file_id.as_str(), target_parent_file_id.as_str())
                .await?;
        }
        if source_name.as_deref() != Some(target_name.as_str()) {
            self.update_file_name(file_id.as_str(), target_name.as_str())
                .await?;
        }
        Ok(())
    }
}

impl MobileListItem {
    fn is_folder(&self) -> bool {
        self.item_type.as_deref() == Some("folder")
    }

    fn file_id(&self) -> Option<&str> {
        self.file_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn display_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    fn resolved_last_modified(&self) -> Option<String> {
        self.updated_at
            .as_deref()
            .or(self.created_at.as_deref())
            .map(normalize_timestamp)
    }

    fn resolved_content_type(&self) -> Option<String> {
        self.display_name()
            .and_then(|name| guess_content_type(name).map(str::to_string))
            .or_else(|| {
                self.file_extension
                    .as_deref()
                    .and_then(|suffix| guess_content_type_from_suffix(suffix).map(str::to_string))
            })
    }

    fn object_key(&self, prefix: &str) -> Result<String, BlobError> {
        let name = self.display_name().ok_or_else(|| {
            BlobError::Upstream("China Mobile file/list returned a file without a name".to_string())
        })?;
        Ok(join_relative_key(prefix, name))
    }

    fn to_object_info(&self, key: String) -> ObjectInfo {
        ObjectInfo {
            key,
            size: self.size.unwrap_or(0),
            etag: self.file_id.clone(),
            content_type: self.resolved_content_type(),
            last_modified: self.resolved_last_modified(),
        }
    }
}

async fn decode_success_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    operation: &str,
) -> Result<T, BlobError> {
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("<failed to read body: {error}>"));
        return Err(BlobError::Upstream(format!(
            "{operation} failed with HTTP {status}: {body}"
        )));
    }

    response
        .json::<T>()
        .await
        .map_err(|error| BlobError::Upstream(format!("{operation} returned invalid JSON: {error}")))
}

fn elapsed_millis(started_at: Instant) -> u64 {
    started_at
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn build_http_client(config: &MobileConfig) -> Result<reqwest::Client, BlobError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(config.request_timeout_secs.max(1)));
    if let Some(local_address) = config.outbound_ip_family.local_address() {
        builder = builder.local_address(local_address);
    }
    builder.build().map_err(|error| {
        BlobError::Configuration(format!(
            "failed to construct China Mobile HTTP client: {error}"
        ))
    })
}

fn join_relative_key(prefix: &str, child_name: &str) -> String {
    let normalized_prefix = prefix.trim_matches('/');
    if normalized_prefix.is_empty() {
        child_name.trim_matches('/').to_string()
    } else {
        format!("{normalized_prefix}/{}", child_name.trim_matches('/'))
    }
}

fn normalize_object_key(key: &str) -> String {
    key.trim_matches('/').to_string()
}

fn split_parent_and_name(key: &str) -> Result<(Option<String>, String), BlobError> {
    let normalized = normalize_object_key(key);
    if normalized.is_empty() {
        return Err(BlobError::NotFound("object key is empty".to_string()));
    }
    match normalized.rsplit_once('/') {
        Some((parent, name)) if !name.trim().is_empty() => {
            Ok((Some(parent.to_string()), name.to_string()))
        }
        Some(_) => Err(BlobError::NotFound("object key is empty".to_string())),
        None => Ok((None, normalized)),
    }
}

fn ensure_present_string(value: Option<String>, label: &str) -> Result<String, BlobError> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| BlobError::Upstream(format!("{label} was missing")))
}

fn build_mcloud_md5_v1_signature(body: &[u8]) -> String {
    let timestamp = mobile_signature_timestamp();
    let nonce = mobile_signature_nonce();
    let body_text = String::from_utf8_lossy(body);
    let compact = body_text.split_whitespace().collect::<String>();
    let encoded = percent_encode_mobile_signature_body(compact.as_str());
    let mut chars = encoded.chars().collect::<Vec<_>>();
    chars.sort_unstable();
    let sorted = chars.into_iter().collect::<String>();
    let left = md5_hex(BASE64_STANDARD.encode(sorted.as_bytes()).as_bytes());
    let right = md5_hex(format!("{timestamp}:{nonce}").as_bytes());
    let signature = md5_hex(format!("{left}{right}").as_bytes()).to_uppercase();
    format!("{timestamp},{nonce},{signature}")
}

fn mobile_signature_timestamp() -> String {
    chrono_like_now_utc8()
}

fn mobile_signature_nonce() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKMNPQRSTWXYZabcdefhijkmnprstwxyz2345678";
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut output = String::with_capacity(16);
    for _ in 0..16 {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed ^= seed << 8;
        let index = (seed as usize) % CHARSET.len();
        output.push(CHARSET[index] as char);
    }
    output
}

fn chrono_like_now_utc8() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        + 8 * 3600;
    let days = now.div_euclid(86_400);
    let secs = now.rem_euclid(86_400);
    let hour = secs / 3600;
    let min = (secs % 3600) / 60;
    let sec = secs % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
}

fn percent_encode_mobile_signature_body(value: &str) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
    const SAFE: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'~')
        .remove(b'(')
        .remove(b')')
        .remove(b'*')
        .remove(b'!')
        .remove(b'\'')
        .remove(b'.');
    utf8_percent_encode(value, SAFE).to_string()
}

fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn mobile_account_from_authorization(value: Option<&str>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let Some(encoded) = value.trim().strip_prefix("Basic ") else {
        return String::new();
    };
    let Ok(decoded) = BASE64_STANDARD.decode(encoded) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&decoded);
    text.split(':').nth(1).unwrap_or_default().to_string()
}

fn directory_may_contain_prefix(directory_key: &str, prefix: &str) -> bool {
    directory_key == prefix
        || prefix
            .strip_prefix(directory_key)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn trim_objects_to_limit(objects: &mut BTreeMap<String, ObjectInfo>, limit: usize) {
    if objects.len() <= limit {
        return;
    }
    let overflow = objects.len() - limit;
    let to_remove: Vec<String> = objects.keys().rev().take(overflow).cloned().collect();
    for key in to_remove {
        objects.remove(key.as_str());
    }
}

fn normalize_timestamp(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return raw.to_string();
    }
    if let Some((date, time)) = trimmed.split_once('T') {
        return format!("{} {}", date.trim(), time.trim_end_matches('Z').trim());
    }
    trimmed.to_string()
}

fn guess_content_type(name: &str) -> Option<&'static str> {
    name.rsplit_once('.')
        .and_then(|(_, suffix)| guess_content_type_from_suffix(suffix))
}

fn guess_content_type_from_suffix(suffix: &str) -> Option<&'static str> {
    match suffix.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "heic" => Some("image/heic"),
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "wav" => Some("audio/wav"),
        "flac" => Some("audio/flac"),
        "aac" => Some("audio/aac"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "mkv" => Some("video/x-matroska"),
        "avi" => Some("video/x-msvideo"),
        "pdf" => Some("application/pdf"),
        "txt" => Some("text/plain"),
        "md" => Some("text/markdown"),
        "csv" => Some("text/csv"),
        "json" => Some("application/json"),
        "doc" => Some("application/msword"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xls" => Some("application/vnd.ms-excel"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "ppt" => Some("application/vnd.ms-powerpoint"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        _ => None,
    }
}

fn mebibytes_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024 * 1024)
}

fn ensure_non_empty(value: &str, label: &str) -> Result<(), BlobError> {
    if value.trim().is_empty() {
        Err(BlobError::Configuration(format!("{label} cannot be empty")))
    } else {
        Ok(())
    }
}

fn load_mobile_native_capabilities(
    path: Option<&str>,
) -> Result<BTreeMap<String, MobileNativeCapabilitySpec>, BlobError> {
    let path = path.map(str::trim).filter(|value| !value.is_empty());
    let (raw, source_label) = match path {
        Some(path) => (
            fs::read_to_string(path).map_err(|error| {
                BlobError::Configuration(format!(
                    "failed to read China Mobile native capability catalog {path}: {error}"
                ))
            })?,
            path.to_string(),
        ),
        None => (
            DEFAULT_MOBILE_NATIVE_CAPABILITY_CATALOG_JSON.to_string(),
            "embedded default".to_string(),
        ),
    };

    let catalog = serde_json::from_str::<MobileNativeCapabilityCatalog>(&raw).map_err(|error| {
        BlobError::Configuration(format!(
            "invalid China Mobile native capability catalog {source_label}: {error}"
        ))
    })?;

    if catalog.schema_version != MOBILE_NATIVE_CAPABILITY_SCHEMA_VERSION {
        return Err(BlobError::Configuration(format!(
            "unsupported China Mobile native capability catalog schema_version={} from {source_label}",
            catalog.schema_version
        )));
    }

    if catalog.provider.trim() != "mobile" {
        return Err(BlobError::Configuration(format!(
            "unexpected China Mobile native capability catalog provider={} from {source_label}",
            catalog.provider
        )));
    }

    let mut capabilities = BTreeMap::new();
    for capability in catalog.capabilities {
        let capability_id = capability.id.trim().to_string();
        if capability_id.is_empty() {
            return Err(BlobError::Configuration(format!(
                "China Mobile native capability catalog {source_label} contains an empty capability id"
            )));
        }
        if capability.method.trim().is_empty() {
            return Err(BlobError::Configuration(format!(
                "China Mobile native capability catalog {source_label} contains an empty method for capability {capability_id}"
            )));
        }
        if capability.url.trim().is_empty() {
            return Err(BlobError::Configuration(format!(
                "China Mobile native capability catalog {source_label} contains an empty url for capability {capability_id}"
            )));
        }
        if capabilities
            .insert(capability_id.clone(), capability)
            .is_some()
        {
            return Err(BlobError::Configuration(format!(
                "duplicate China Mobile native capability id in catalog {source_label}: {capability_id}"
            )));
        }
    }

    Ok(capabilities)
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
            .ok_or_else(|| de::Error::custom("expected unsigned integer"))
            .map(Some),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|error| de::Error::custom(format!("invalid integer: {error}")))
            }
        }
        Some(other) => Err(de::Error::custom(format!(
            "expected integer or string, found {other}"
        ))),
    }
}

fn deserialize_vec_or_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value = Option::<Vec<T>>::deserialize(deserializer)?;
    Ok(value.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Path, State},
        http::StatusCode,
        routing::{get, post, put},
    };
    use bytes::Bytes;
    use futures_util::stream;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tempfile::NamedTempFile;
    use tokio::net::TcpListener;

    fn sample_config() -> MobileConfig {
        MobileConfig {
            base_url: "https://yun.139.com".to_string(),
            token_source: TokenSource::Static {
                bearer: "Basic mobile-session".to_string(),
            },
            outbound_ip_family: OutboundIpFamily::Auto,
            cookie_header: None,
            user_agent: "Mozilla/5.0".to_string(),
            browser_profile: None,
            request_timeout_secs: 30,
            list_url: "https://personal-kd-njs.yun.139.com/hcy/file/list".to_string(),
            personal_disk_info_url: "https://user-njs.yun.139.com/user/disk/getPersonalDiskInfo"
                .to_string(),
            family_disk_info_url: "https://user-njs.yun.139.com/user/disk/getFamilyDiskInfo"
                .to_string(),
            root_folder_id: Some("/".to_string()),
            user_domain_id: Some("123".to_string()),
            page_size: 100,
            root_prefix: Some("ccbg-managed".to_string()),
            upload_part_size_bytes: 20 * 1024 * 1024,
            upload_max_part_count: DEFAULT_MOBILE_UPLOAD_MAX_PART_COUNT,
            max_single_upload_bytes: None,
            max_single_download_bytes: None,
            body_spool_dir: None,
            body_spool_observer: None,
            native_capability_catalog_path: None,
        }
    }

    #[test]
    fn managed_object_key_uses_single_provider_root() {
        let adapter = MobileBlobAdapter::new(sample_config()).expect("adapter");
        assert_eq!(
            adapter
                .managed_object_key("bucket-a", "folder/file.txt")
                .expect("object path"),
            "ccbg-managed/bucket-a/folder/file.txt"
        );
    }

    #[test]
    fn managed_object_key_preserves_parent_path_segments() {
        let adapter = MobileBlobAdapter::new(sample_config()).expect("adapter");
        let managed = adapter
            .managed_object_key("bucket-a", "nested/inner/file.txt")
            .expect("object path");
        let (parent, file_name) = managed
            .rsplit_once('/')
            .expect("managed path should contain a filename");
        assert_eq!(parent, "ccbg-managed/bucket-a/nested/inner");
        assert_eq!(file_name, "file.txt");
    }

    #[test]
    fn managed_container_root_stays_scoped_to_one_folder_per_container() {
        let adapter = MobileBlobAdapter::new(sample_config()).expect("adapter");
        assert_eq!(
            adapter
                .managed_container_root("root")
                .expect("managed container root"),
            "ccbg-managed/root"
        );
    }

    #[test]
    fn loads_default_mobile_native_capabilities() {
        let capabilities = load_mobile_native_capabilities(None).expect("default capabilities");
        assert!(capabilities.contains_key("file_create"));
        assert!(capabilities.contains_key("file_update"));
        assert!(capabilities.contains_key("file_batch_delete"));
        assert!(capabilities.contains_key("file_batch_move"));
    }

    #[test]
    fn rejects_duplicate_capability_ids() {
        let temp = NamedTempFile::new().expect("temp file");
        fs::write(
            temp.path(),
            r#"{
              "schema_version": 1,
              "provider": "mobile",
              "capabilities": [
                { "id": "x", "method": "POST", "url": "https://example.com/a" },
                { "id": "x", "method": "POST", "url": "https://example.com/b" }
              ]
            }"#,
        )
        .expect("write temp catalog");
        let error = load_mobile_native_capabilities(temp.path().to_str())
            .expect_err("duplicate id should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate China Mobile native capability id")
        );
    }

    #[test]
    fn capability_catalog_fields_are_available_for_runtime_execution() {
        let capabilities = load_mobile_native_capabilities(None).expect("default capabilities");
        let create = capabilities
            .get("file_create")
            .expect("file_create capability");
        assert_eq!(create.method, "POST");
        assert_eq!(create.signature_strategy.as_deref(), Some("mcloud_md5_v1"));
        assert_eq!(
            create
                .body_defaults
                .get("contentHashAlgorithm")
                .and_then(Value::as_str),
            Some("SHA256")
        );
        let download = capabilities
            .get("file_get_download_url")
            .expect("file_get_download_url capability");
        assert_eq!(download.method, "POST");
        assert_eq!(
            download.signature_strategy.as_deref(),
            Some("mcloud_md5_v1")
        );
        let update = capabilities
            .get("file_update")
            .expect("file_update capability");
        assert_eq!(update.method, "POST");
        assert_eq!(update.signature_strategy.as_deref(), Some("mcloud_md5_v1"));
        assert_eq!(
            update
                .body_defaults
                .get("description")
                .and_then(Value::as_str),
            Some("")
        );
    }

    #[test]
    fn effective_upload_part_size_respects_max_part_count() {
        let adapter = MobileBlobAdapter::new(sample_config()).expect("adapter");
        let size = 2 * 1024 * 1024 * 1024u64;
        let part_size = adapter.effective_upload_part_size_bytes(size);
        let parts = MobileBlobAdapter::upload_part_infos(size, part_size);
        assert_eq!(parts.len() as u64, DEFAULT_MOBILE_UPLOAD_MAX_PART_COUNT);
        assert!(part_size > adapter.upload_part_size_bytes());
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
    struct MockSpoolLease {
        stats: Arc<Mutex<MockSpoolStats>>,
        tracked_bytes: u64,
    }

    impl blob_core::BodySpoolObserver for MockSpoolObserver {
        fn start_tracking(&self) -> Box<dyn blob_core::BodySpoolLease> {
            let mut stats = self.stats.lock().expect("spool stats poisoned");
            stats.active_files = stats.active_files.saturating_add(1);
            stats.peak_files = stats.peak_files.max(stats.active_files);
            Box::new(MockSpoolLease {
                stats: Arc::clone(&self.stats),
                tracked_bytes: 0,
            })
        }
    }

    impl blob_core::BodySpoolLease for MockSpoolLease {
        fn update_tracked_bytes(&mut self, next_bytes: u64) {
            let mut stats = self.stats.lock().expect("spool stats poisoned");
            if next_bytes >= self.tracked_bytes {
                stats.active_bytes = stats
                    .active_bytes
                    .saturating_add(next_bytes.saturating_sub(self.tracked_bytes));
            } else {
                stats.active_bytes = stats
                    .active_bytes
                    .saturating_sub(self.tracked_bytes.saturating_sub(next_bytes));
            }
            self.tracked_bytes = next_bytes;
            stats.peak_bytes = stats.peak_bytes.max(stats.active_bytes);
        }
    }

    impl Drop for MockSpoolLease {
        fn drop(&mut self) {
            let mut stats = self.stats.lock().expect("spool stats poisoned");
            stats.active_files = stats.active_files.saturating_sub(1);
            stats.active_bytes = stats.active_bytes.saturating_sub(self.tracked_bytes);
        }
    }

    #[derive(Clone)]
    struct MockMobileState {
        base_url: String,
        items_by_parent: BTreeMap<String, Vec<Value>>,
        downloads: BTreeMap<String, Bytes>,
    }

    async fn mock_file_list(
        State(state): State<Arc<MockMobileState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let parent_file_id = body
            .get("parentFileId")
            .and_then(Value::as_str)
            .unwrap_or("/");
        let items = state
            .items_by_parent
            .get(parent_file_id)
            .cloned()
            .unwrap_or_default();
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "items": items,
                "nextPageCursor": null
            }
        }))
    }

    async fn mock_get_download_url(
        State(state): State<Arc<MockMobileState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let file_id = body
            .get("fileId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "fileId": file_id,
                "url": format!("{}/download/{}", state.base_url, file_id)
            }
        }))
    }

    async fn mock_download(
        State(state): State<Arc<MockMobileState>>,
        Path(file_id): Path<String>,
    ) -> (StatusCode, Bytes) {
        let body = state
            .downloads
            .get(file_id.as_str())
            .cloned()
            .unwrap_or_default();
        (StatusCode::OK, body)
    }

    fn write_mobile_capability_catalog(base_url: &str) -> NamedTempFile {
        let temp = NamedTempFile::new().expect("temp capability catalog");
        fs::write(
            temp.path(),
            serde_json::to_string_pretty(&json!({
                "schema_version": 1,
                "provider": "mobile",
                "capabilities": [
                    {
                        "id": "file_list",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/list"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {
                            "orderBy": "updated_at",
                            "orderDirection": "DESC",
                            "imageThumbnailStyleList": ["Small", "Large"]
                        }
                    },
                    {
                        "id": "file_get_download_url",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/getDownloadUrl"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {}
                    }
                ]
            }))
            .expect("serialize mock mobile capability catalog"),
        )
        .expect("write mock mobile capability catalog");
        temp
    }

    #[derive(Clone)]
    struct MockMobileUploadState {
        base_url: String,
        items_by_parent: BTreeMap<String, Vec<Value>>,
        create_requests: Arc<Mutex<Vec<Value>>>,
        complete_requests: Arc<Mutex<Vec<Value>>>,
        upload_parts: Arc<Mutex<Vec<(u32, Bytes)>>>,
        rapid_upload: bool,
    }

    async fn mock_file_list_upload(
        State(state): State<Arc<MockMobileUploadState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let parent_file_id = body
            .get("parentFileId")
            .and_then(Value::as_str)
            .unwrap_or("/");
        let items = state
            .items_by_parent
            .get(parent_file_id)
            .cloned()
            .unwrap_or_default();
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "items": items,
                "nextPageCursor": null
            }
        }))
    }

    async fn mock_file_create_upload(
        State(state): State<Arc<MockMobileUploadState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .create_requests
            .lock()
            .expect("create requests poisoned")
            .push(body.clone());

        let requested_parts = body
            .get("partInfos")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let response_parts = requested_parts
            .into_iter()
            .map(|part| {
                let part_number =
                    part.get("partNumber").and_then(Value::as_u64).unwrap_or(1) as u32;
                let part_size = part.get("partSize").and_then(Value::as_u64).unwrap_or(0);
                json!({
                    "partNumber": part_number,
                    "partSize": part_size,
                    "uploadUrl": format!("{}/upload/{}", state.base_url, part_number)
                })
            })
            .collect::<Vec<_>>();
        let rapid_upload = state.rapid_upload;

        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "fileId": "file-uploaded",
                "uploadId": if rapid_upload { Value::Null } else { Value::String("upload-001".to_string()) },
                "rapidUpload": rapid_upload,
                "partInfos": if rapid_upload { Value::Array(Vec::new()) } else { Value::Array(response_parts) }
            }
        }))
    }

    async fn mock_file_complete_upload(
        State(state): State<Arc<MockMobileUploadState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .complete_requests
            .lock()
            .expect("complete requests poisoned")
            .push(body.clone());
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "fileId": "file-uploaded",
                "contentHash": body.get("contentHash").cloned().unwrap_or(Value::Null),
                "fileName": "probe.bin"
            }
        }))
    }

    async fn mock_upload_part(
        State(state): State<Arc<MockMobileUploadState>>,
        Path(part_number): Path<u32>,
        body: Bytes,
    ) -> StatusCode {
        state
            .upload_parts
            .lock()
            .expect("upload parts poisoned")
            .push((part_number, body));
        StatusCode::OK
    }

    fn write_mobile_upload_capability_catalog(base_url: &str) -> NamedTempFile {
        let temp = NamedTempFile::new().expect("temp capability catalog");
        fs::write(
            temp.path(),
            serde_json::to_string_pretty(&json!({
                "schema_version": 1,
                "provider": "mobile",
                "capabilities": [
                    {
                        "id": "file_list",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/list"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {
                            "orderBy": "updated_at",
                            "orderDirection": "DESC",
                            "imageThumbnailStyleList": ["Small", "Large"]
                        }
                    },
                    {
                        "id": "file_create",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/create"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {
                            "fileRenameMode": "auto_rename",
                            "type": "file",
                            "contentHashAlgorithm": "SHA256"
                        }
                    },
                    {
                        "id": "file_complete",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/complete"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {}
                    }
                ]
            }))
            .expect("serialize mock mobile upload capability catalog"),
        )
        .expect("write mock mobile upload capability catalog");
        temp
    }

    #[derive(Clone)]
    struct MockMobileMetadataState {
        items_by_parent: BTreeMap<String, Vec<Value>>,
        update_requests: Arc<Mutex<Vec<Value>>>,
        delete_requests: Arc<Mutex<Vec<Value>>>,
        move_requests: Arc<Mutex<Vec<Value>>>,
    }

    async fn mock_file_list_metadata(
        State(state): State<Arc<MockMobileMetadataState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let parent_file_id = body
            .get("parentFileId")
            .and_then(Value::as_str)
            .unwrap_or("/");
        let items = state
            .items_by_parent
            .get(parent_file_id)
            .cloned()
            .unwrap_or_default();
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok",
            "data": {
                "items": items,
                "nextPageCursor": null
            }
        }))
    }

    async fn mock_file_update_metadata(
        State(state): State<Arc<MockMobileMetadataState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .update_requests
            .lock()
            .expect("update requests poisoned")
            .push(body);
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok"
        }))
    }

    async fn mock_file_delete_metadata(
        State(state): State<Arc<MockMobileMetadataState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .delete_requests
            .lock()
            .expect("delete requests poisoned")
            .push(body);
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok"
        }))
    }

    async fn mock_file_move_metadata(
        State(state): State<Arc<MockMobileMetadataState>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        state
            .move_requests
            .lock()
            .expect("move requests poisoned")
            .push(body);
        Json(json!({
            "success": true,
            "code": "0000",
            "message": "ok"
        }))
    }

    fn write_mobile_metadata_capability_catalog(base_url: &str) -> NamedTempFile {
        let temp = NamedTempFile::new().expect("temp metadata capability catalog");
        fs::write(
            temp.path(),
            serde_json::to_string_pretty(&json!({
                "schema_version": 1,
                "provider": "mobile",
                "capabilities": [
                    {
                        "id": "file_list",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/list"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {
                            "orderBy": "updated_at",
                            "orderDirection": "DESC",
                            "imageThumbnailStyleList": ["Small", "Large"]
                        }
                    },
                    {
                        "id": "file_update",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/update"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {
                            "description": ""
                        }
                    },
                    {
                        "id": "file_batch_delete",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/batchDelete"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {}
                    },
                    {
                        "id": "file_batch_move",
                        "method": "POST",
                        "url": format!("{base_url}/hcy/file/batchMove"),
                        "signature_strategy": "mcloud_md5_v1",
                        "body_defaults": {}
                    }
                ]
            }))
            .expect("serialize mock mobile metadata capability catalog"),
        )
        .expect("write mock mobile metadata capability catalog");
        temp
    }

    fn sample_folder_item(file_id: &str, name: &str) -> Value {
        json!({
            "fileId": file_id,
            "name": name,
            "type": "folder",
            "createdAt": "2026-05-20T06:00:00.000+08:00",
            "updatedAt": "2026-05-20T06:00:00.000+08:00"
        })
    }

    fn sample_file_item(file_id: &str, name: &str, size: u64) -> Value {
        json!({
            "fileId": file_id,
            "name": name,
            "type": "file",
            "fileExtension": name.rsplit_once('.').map(|(_, suffix)| suffix).unwrap_or("txt"),
            "size": size,
            "createdAt": "2026-05-20T06:00:00.000+08:00",
            "updatedAt": "2026-05-20T06:00:00.000+08:00"
        })
    }

    #[tokio::test]
    async fn managed_root_head_list_and_get_are_scoped_under_one_provider_folder() {
        let flat_file_id = "file-flat";
        let nested_file_id = "file-nested";
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock mobile server");
        let address = listener.local_addr().expect("mock mobile local addr");
        let base_url = format!("http://{address}");
        let mut items_by_parent = BTreeMap::new();
        items_by_parent.insert(
            "/".to_string(),
            vec![
                sample_folder_item("folder-managed", "ccbg-managed"),
                sample_folder_item("folder-other", "other-top-level"),
            ],
        );
        items_by_parent.insert(
            "folder-managed".to_string(),
            vec![sample_folder_item("folder-root", "root")],
        );
        items_by_parent.insert(
            "folder-root".to_string(),
            vec![
                sample_folder_item("folder-verification", "ccbg-verification"),
                sample_file_item(flat_file_id, "diag-unicom-flat-20260520-v2.txt", 39),
            ],
        );
        items_by_parent.insert(
            "folder-verification".to_string(),
            vec![sample_file_item(
                nested_file_id,
                "diag-unicom-nested-20260520.txt",
                41,
            )],
        );
        let mut downloads = BTreeMap::new();
        downloads.insert(
            flat_file_id.to_string(),
            Bytes::from_static(b"unicom flat probe via default topology\n"),
        );
        downloads.insert(
            nested_file_id.to_string(),
            Bytes::from_static(b"unicom nested probe via default topology\n"),
        );
        let state = MockMobileState {
            base_url: base_url.clone(),
            items_by_parent,
            downloads,
        };
        let app = Router::new()
            .route("/hcy/file/list", post(mock_file_list))
            .route("/hcy/file/getDownloadUrl", post(mock_get_download_url))
            .route("/download/{file_id}", get(mock_download))
            .with_state(Arc::new(state));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock mobile server");
        });

        let catalog = write_mobile_capability_catalog(base_url.as_str());
        let mut config = sample_config();
        config.native_capability_catalog_path =
            Some(catalog.path().to_str().expect("catalog path").to_string());
        let adapter = MobileBlobAdapter::new(config).expect("adapter");

        let head = adapter
            .head_object(MOBILE_ROOT_CONTAINER, "diag-unicom-flat-20260520-v2.txt")
            .await
            .expect("head flat file");
        assert_eq!(head.key, "diag-unicom-flat-20260520-v2.txt");
        assert_eq!(head.size, 39);

        let nested = adapter
            .head_object(
                MOBILE_ROOT_CONTAINER,
                "ccbg-verification/diag-unicom-nested-20260520.txt",
            )
            .await
            .expect("head nested file");
        assert_eq!(
            nested.key,
            "ccbg-verification/diag-unicom-nested-20260520.txt"
        );
        assert_eq!(nested.size, 41);

        let listed = adapter
            .list_objects(ListObjectsRequest {
                container: Some(MOBILE_ROOT_CONTAINER.to_string()),
                prefix: Some("diag-unicom-flat-20260520-v2.txt".to_string()),
                limit: None,
            })
            .await
            .expect("list flat file by prefix");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "diag-unicom-flat-20260520-v2.txt");

        let payload = adapter
            .get_object(
                MOBILE_ROOT_CONTAINER,
                "ccbg-verification/diag-unicom-nested-20260520.txt",
            )
            .await
            .expect("download nested file");
        let body = payload.body.collect().await.expect("collect nested body");
        assert_eq!(
            body,
            Bytes::from_static(b"unicom nested probe via default topology\n")
        );
    }

    #[tokio::test]
    async fn put_object_streams_to_spool_and_uploads_mobile_parts() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock mobile upload server");
        let address = listener
            .local_addr()
            .expect("mock mobile upload local addr");
        let base_url = format!("http://{address}");

        let upload_state = MockMobileUploadState {
            base_url: base_url.clone(),
            items_by_parent: BTreeMap::from([(
                "/".to_string(),
                vec![sample_folder_item("folder-root", "root")],
            )]),
            create_requests: Arc::new(Mutex::new(Vec::new())),
            complete_requests: Arc::new(Mutex::new(Vec::new())),
            upload_parts: Arc::new(Mutex::new(Vec::new())),
            rapid_upload: false,
        };
        let app = Router::new()
            .route("/hcy/file/list", post(mock_file_list_upload))
            .route("/hcy/file/create", post(mock_file_create_upload))
            .route("/hcy/file/complete", post(mock_file_complete_upload))
            .route("/upload/{part_number}", put(mock_upload_part))
            .with_state(Arc::new(upload_state.clone()));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock mobile upload server");
        });

        let catalog = write_mobile_upload_capability_catalog(base_url.as_str());
        let mut config = sample_config();
        let spool_stats = Arc::new(Mutex::new(MockSpoolStats::default()));
        config.upload_part_size_bytes = 5;
        config.root_prefix = None;
        config.body_spool_observer = Some(Arc::new(MockSpoolObserver {
            stats: Arc::clone(&spool_stats),
        }));
        config.native_capability_catalog_path =
            Some(catalog.path().to_str().expect("catalog path").to_string());
        let adapter = MobileBlobAdapter::new(config).expect("adapter");

        let request = PutObjectRequest {
            container: MOBILE_ROOT_CONTAINER.to_string(),
            key: "probe.bin".to_string(),
            body: ObjectBody::from_stream(stream::iter([
                Ok(Bytes::from_static(b"hello ")),
                Ok(Bytes::from_static(b"world")),
            ])),
            size: Some(11),
            content_type: Some("application/octet-stream".to_string()),
            preferred_upload_part_size_bytes: None,
        };
        let result = adapter.put_object(request).await.expect("put object");
        assert!(result.first_response_latency_ms.is_some());
        assert!(result.etag.is_some());

        let create_requests = upload_state
            .create_requests
            .lock()
            .expect("create requests poisoned")
            .clone();
        assert_eq!(create_requests.len(), 1);
        let part_infos = create_requests[0]
            .get("partInfos")
            .and_then(Value::as_array)
            .expect("partInfos array");
        let part_sizes = part_infos
            .iter()
            .map(|part| part.get("partSize").and_then(Value::as_u64).unwrap_or(0))
            .collect::<Vec<_>>();
        assert_eq!(part_sizes, vec![5, 5, 1]);

        let mut uploaded_parts = upload_state
            .upload_parts
            .lock()
            .expect("upload parts poisoned")
            .clone();
        uploaded_parts.sort_by_key(|(part_number, _)| *part_number);
        let uploaded_bodies = uploaded_parts
            .into_iter()
            .map(|(_, body)| body)
            .collect::<Vec<_>>();
        assert_eq!(
            uploaded_bodies,
            vec![
                Bytes::from_static(b"hello"),
                Bytes::from_static(b" worl"),
                Bytes::from_static(b"d"),
            ]
        );

        let complete_requests = upload_state
            .complete_requests
            .lock()
            .expect("complete requests poisoned")
            .clone();
        assert_eq!(complete_requests.len(), 1);
        assert_eq!(
            complete_requests[0]
                .get("contentHashAlgorithm")
                .and_then(Value::as_str),
            Some("SHA256")
        );

        let spool_stats = spool_stats.lock().expect("spool stats poisoned");
        assert_eq!(spool_stats.active_files, 0);
        assert_eq!(spool_stats.active_bytes, 0);
        assert_eq!(spool_stats.peak_files, 1);
        assert_eq!(spool_stats.peak_bytes, 11);
    }

    #[tokio::test]
    async fn put_object_treats_rapid_upload_as_success_without_complete() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock mobile rapid upload server");
        let address = listener
            .local_addr()
            .expect("mock mobile rapid upload local addr");
        let base_url = format!("http://{address}");

        let upload_state = MockMobileUploadState {
            base_url: base_url.clone(),
            items_by_parent: BTreeMap::from([(
                "/".to_string(),
                vec![sample_folder_item("folder-root", "root")],
            )]),
            create_requests: Arc::new(Mutex::new(Vec::new())),
            complete_requests: Arc::new(Mutex::new(Vec::new())),
            upload_parts: Arc::new(Mutex::new(Vec::new())),
            rapid_upload: true,
        };
        let app = Router::new()
            .route("/hcy/file/list", post(mock_file_list_upload))
            .route("/hcy/file/create", post(mock_file_create_upload))
            .route("/hcy/file/complete", post(mock_file_complete_upload))
            .route("/upload/{part_number}", put(mock_upload_part))
            .with_state(Arc::new(upload_state.clone()));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock mobile rapid upload server");
        });

        let catalog = write_mobile_upload_capability_catalog(base_url.as_str());
        let mut config = sample_config();
        config.root_prefix = None;
        config.native_capability_catalog_path =
            Some(catalog.path().to_str().expect("catalog path").to_string());
        let adapter = MobileBlobAdapter::new(config).expect("adapter");

        let request = PutObjectRequest {
            container: MOBILE_ROOT_CONTAINER.to_string(),
            key: "rapid.bin".to_string(),
            body: ObjectBody::from_bytes(Bytes::from_static(b"rapid-upload-probe")),
            size: Some(18),
            content_type: Some("application/octet-stream".to_string()),
            preferred_upload_part_size_bytes: None,
        };
        let result = adapter.put_object(request).await.expect("put object");
        assert!(result.first_response_latency_ms.is_some());
        assert_eq!(result.etag.as_deref(), Some("file-uploaded"));

        let upload_parts = upload_state
            .upload_parts
            .lock()
            .expect("upload parts poisoned");
        assert!(upload_parts.is_empty());
        drop(upload_parts);

        let complete_requests = upload_state
            .complete_requests
            .lock()
            .expect("complete requests poisoned");
        assert!(complete_requests.is_empty());
    }

    #[tokio::test]
    async fn metadata_object_actions_use_native_mobile_file_apis() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock mobile metadata server");
        let address = listener
            .local_addr()
            .expect("mock mobile metadata local addr");
        let base_url = format!("http://{address}");

        let state = MockMobileMetadataState {
            items_by_parent: BTreeMap::from([
                (
                    "/".to_string(),
                    vec![sample_folder_item("folder-managed", "ccbg-managed")],
                ),
                (
                    "folder-managed".to_string(),
                    vec![sample_folder_item("folder-root", "root")],
                ),
                (
                    "folder-root".to_string(),
                    vec![
                        sample_folder_item("folder-docs", "docs"),
                        sample_folder_item("folder-media", "media"),
                    ],
                ),
                (
                    "folder-docs".to_string(),
                    vec![sample_file_item("file-alpha", "alpha.txt", 5)],
                ),
                ("folder-media".to_string(), vec![]),
            ]),
            update_requests: Arc::new(Mutex::new(Vec::new())),
            delete_requests: Arc::new(Mutex::new(Vec::new())),
            move_requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/hcy/file/list", post(mock_file_list_metadata))
            .route("/hcy/file/update", post(mock_file_update_metadata))
            .route("/hcy/file/batchDelete", post(mock_file_delete_metadata))
            .route("/hcy/file/batchMove", post(mock_file_move_metadata))
            .with_state(Arc::new(state.clone()));
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve mock mobile metadata server");
        });

        let catalog = write_mobile_metadata_capability_catalog(base_url.as_str());
        let mut config = sample_config();
        config.native_capability_catalog_path =
            Some(catalog.path().to_str().expect("catalog path").to_string());
        let adapter = MobileBlobAdapter::new(config).expect("adapter");
        assert!(adapter.capabilities().delete);

        adapter
            .delete_object(MOBILE_ROOT_CONTAINER, "docs/alpha.txt")
            .await
            .expect("delete should use native metadata api");
        adapter
            .rename_object(RenameObjectRequest {
                container: MOBILE_ROOT_CONTAINER.to_string(),
                key: "docs/alpha.txt".to_string(),
                new_key: "docs/bravo.txt".to_string(),
            })
            .await
            .expect("rename should use native metadata api");
        adapter
            .move_object(MoveObjectRequest {
                source_container: MOBILE_ROOT_CONTAINER.to_string(),
                source_key: "docs/alpha.txt".to_string(),
                destination_container: MOBILE_ROOT_CONTAINER.to_string(),
                destination_key: "media/charlie.txt".to_string(),
            })
            .await
            .expect("move should use native metadata api");

        let delete_requests = state
            .delete_requests
            .lock()
            .expect("delete requests poisoned")
            .clone();
        assert_eq!(delete_requests, vec![json!({ "fileIds": ["file-alpha"] })]);

        let update_requests = state
            .update_requests
            .lock()
            .expect("update requests poisoned")
            .clone();
        assert_eq!(
            update_requests,
            vec![
                json!({
                    "description": "",
                    "fileId": "file-alpha",
                    "name": "bravo.txt"
                }),
                json!({
                    "description": "",
                    "fileId": "file-alpha",
                    "name": "charlie.txt"
                })
            ]
        );

        let move_requests = state
            .move_requests
            .lock()
            .expect("move requests poisoned")
            .clone();
        assert_eq!(
            move_requests,
            vec![json!({
                "fileIds": ["file-alpha"],
                "toParentFileId": "folder-media"
            })]
        );
    }
}
