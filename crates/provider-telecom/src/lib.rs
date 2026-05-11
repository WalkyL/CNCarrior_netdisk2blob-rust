use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, ContainerInfo, HealthStatus, ListObjectsRequest,
    ObjectInfo, ObjectPayload, OutboundIpFamily, ServiceHealth, StorageCapacity,
    StorageScopeHealth, StorageScopeKind, TokenSource,
};
use md5::{Digest, Md5};
use reqwest::{
    Method, Response, StatusCode,
    header::{ACCEPT, COOKIE, HeaderValue, REFERER, USER_AGENT},
};
use serde::{Deserialize, Deserializer, Serialize, de, de::DeserializeOwned};

const TELECOM_ROOT_CONTAINER: &str = "root";
const DEFAULT_ROOT_FOLDER_ID: &str = "-11";
const DEFAULT_PAGE_SIZE: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelecomConfig {
    pub base_url: String,
    pub token_source: TokenSource,
    pub outbound_ip_family: OutboundIpFamily,
    pub browser_id: Option<String>,
    pub cookie_header: Option<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
    pub sign_type: String,
    pub root_folder_id: String,
    pub page_size: usize,
}

pub struct TelecomBlobAdapter {
    config: TelecomConfig,
    client: reqwest::Client,
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

    fn sign_type(&self) -> &str {
        let sign_type = self.config.sign_type.trim();
        if sign_type.is_empty() { "1" } else { sign_type }
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

    fn browser_id(&self) -> Result<&str, BlobError> {
        self.config
            .browser_id
            .as_deref()
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
            .header(USER_AGENT, self.config.user_agent.as_str())
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

        if let Some(referer) = referer.map(str::trim).filter(|value| !value.is_empty()) {
            request = request.header(
                REFERER,
                HeaderValue::from_str(referer).map_err(|error| {
                    BlobError::Configuration(format!(
                        "invalid China Telecom Referer header value: {error}"
                    ))
                })?,
            );
        }

        Ok(request)
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

    async fn download_url_with_signature(
        &self,
        file_id: &str,
        signed_token: Option<&str>,
    ) -> Result<String, BlobError> {
        let query = vec![("fileId".to_string(), file_id.to_string())];
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
            writable: false,
            root: Some(self.root_folder_id().to_string()),
            container: Some(TELECOM_ROOT_CONTAINER.to_string()),
            object_count,
            capacity,
            notes: vec!["backed by cloud.189.cn web session".to_string()],
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
        }
    }

    async fn find_child_folder(
        &self,
        parent_folder_id: &str,
        child_name: &str,
    ) -> Result<TelecomFolderEntry, BlobError> {
        let mut page_num = 1;
        loop {
            let page = self.list_files_page(parent_folder_id, page_num).await?;
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

    async fn find_child_file(
        &self,
        parent_folder_id: &str,
        child_name: &str,
    ) -> Result<TelecomFileEntry, BlobError> {
        let mut page_num = 1;
        loop {
            let page = self.list_files_page(parent_folder_id, page_num).await?;
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

    async fn resolve_file_entry(&self, key: &str) -> Result<(TelecomFileEntry, String), BlobError> {
        let normalized_key = normalize_object_key(key);
        if normalized_key.is_empty() {
            return Err(BlobError::NotFound("object key is empty".to_string()));
        }

        let segments = normalized_key.split('/').collect::<Vec<_>>();
        let mut parent_folder_id = self.root_folder_id().to_string();

        for segment in &segments[..segments.len().saturating_sub(1)] {
            let folder = self.find_child_folder(&parent_folder_id, segment).await?;
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
        let file = self.find_child_file(&parent_folder_id, file_name).await?;
        Ok((file, normalized_key))
    }

    async fn get_bytes(&self, url: &str, action: &str) -> Result<Vec<u8>, BlobError> {
        let response = self
            .client
            .request(Method::GET, url)
            .header(USER_AGENT, self.config.user_agent.as_str())
            .timeout(self.timeout())
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
        if normalize_object_key(container) == TELECOM_ROOT_CONTAINER {
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
        let mut stack = vec![(self.root_folder_id().to_string(), String::new())];

        while let Some((folder_id, folder_prefix)) = stack.pop() {
            let mut page_num = 1;
            loop {
                let page = self.list_files_page(&folder_id, page_num).await?;
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
            write: false,
            delete: false,
            multipart_upload: false,
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
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

                notes.push("family_scope_detection=not_confirmed_for_telecom_web_api".to_string());
                HealthStatus::Healthy
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
        Ok(vec![ContainerInfo {
            name: TELECOM_ROOT_CONTAINER.to_string(),
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

    async fn head_object(&self, container: &str, key: &str) -> Result<ObjectInfo, BlobError> {
        self.validate_container(container)?;
        let (entry, normalized_key) = self.resolve_file_entry(key).await?;
        Ok(entry.to_object_info(normalized_key))
    }

    async fn get_object(&self, container: &str, key: &str) -> Result<ObjectPayload, BlobError> {
        self.validate_container(container)?;
        let (entry, normalized_key) = self.resolve_file_entry(key).await?;
        let download_url = entry
            .download_url
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| normalize_remote_url(self.trimmed_base_url(), value))
            .unwrap_or_else(|| String::new());
        let download_url = if download_url.is_empty() {
            let file_id = entry.id().ok_or_else(|| {
                BlobError::Upstream(
                    "getFileDownloadUrl.action cannot run because the file id is missing"
                        .to_string(),
                )
            })?;
            self.download_url_for_file(file_id).await?
        } else {
            download_url
        };
        let info = entry.to_object_info(normalized_key);
        let body = self
            .get_bytes(&download_url, "telecom object download")
            .await?;
        Ok(ObjectPayload { info, body })
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
    if prefix.is_empty() {
        child_name.to_string()
    } else {
        format!("{prefix}/{child_name}")
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
        sync::{Arc, Mutex},
    };

    use super::{
        DEFAULT_ROOT_FOLDER_ID, ListObjectsRequest, TELECOM_ROOT_CONTAINER, TelecomBlobAdapter,
        TelecomConfig, TokenSource, telecom_signature,
    };
    use axum::{
        Router,
        body::Body,
        extract::{Path, Query, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use blob_core::{BlobBackend, HealthStatus, OutboundIpFamily, StorageScopeKind};
    use serde_json::{Value, json};

    #[derive(Clone)]
    struct MockServerState {
        base_url: String,
        access_token: String,
        require_signed_download_url: bool,
        entries_by_parent: Arc<BTreeMap<String, Vec<Value>>>,
        file_bodies_by_id: Arc<BTreeMap<String, Vec<u8>>>,
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
            access_token: &str,
            require_signed_download_url: bool,
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

    fn mock_telecom_adapter(
        base_url: &str,
        browser_id: &str,
        cookie_header: &str,
        token: Option<&str>,
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
            request_timeout_secs: 10,
            sign_type: "1".to_string(),
            root_folder_id: DEFAULT_ROOT_FOLDER_ID.to_string(),
            page_size: 2,
        })
        .expect("telecom test adapter should build")
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
        assert_eq!(payload.body, b"alpha");

        let requests = server.requests();
        let download_requests = requests
            .iter()
            .filter(|request| request["kind"] == "download_url")
            .collect::<Vec<_>>();
        assert_eq!(download_requests.len(), 2);
        assert_eq!(download_requests[0]["signed"], false);
        assert_eq!(download_requests[1]["signed"], true);
        assert_eq!(download_requests[1]["accessToken"], token);
    }
}
