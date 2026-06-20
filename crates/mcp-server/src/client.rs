// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::error::ServerError;
use admin_api::{
    ADMIN_API_VERSION, AdminApiErrorCode, AdminApiErrorResponse, ROUTE_AUTH_CAPTURE_POLICY,
    ROUTE_PROVIDER_CREDENTIALS, ROUTE_REPLICATION_DLQ, ROUTE_REPLICATION_DLQ_REPLAY_JOB,
    ROUTE_REPLICATION_DLQ_REPLAY_TARGET, ROUTE_REPLICATION_RETRY_JOB, ROUTE_STATUS,
    ROUTE_TOPOLOGY_UPDATE, ReplicationDlqListPayload, ReplicationDlqReplayPayload,
    ReplicationDlqTargetReplayPayload, ReplicationRetryPayload,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const ROUTE_APPLICATIONS: &str = "/api/applications";
const ROUTE_CONTENT_POLICIES: &str = "/api/content-policies";

pub trait ControlPlaneClient: Send + Sync {
    fn provider_list(&self) -> Result<ProviderListResult, ServerError>;
    fn provider_health(&self, provider_id: &str) -> Result<ProviderHealthResult, ServerError>;
    fn replication_get_status(&self) -> Result<ReplicationStatusResult, ServerError>;
    fn replication_list_failed_jobs(&self, limit: usize) -> Result<FailedJobsResult, ServerError>;
    fn deployment_config_summary(&self) -> Result<DeploymentConfigSummary, ServerError>;
    fn s3_list_buckets(&self) -> Result<BucketListResult, ServerError>;
    fn alerts_list_recent(&self, limit: usize) -> Result<AlertListResult, ServerError>;
    fn admin_status_get(&self) -> Result<Value, ServerError>;
    fn applications_get(&self) -> Result<Value, ServerError>;
    fn applications_update(&self, payload: Value) -> Result<Value, ServerError>;
    fn content_policies_get(&self) -> Result<Value, ServerError>;
    fn content_policies_update(&self, payload: Value) -> Result<Value, ServerError>;
    fn topology_update(&self, payload: Value) -> Result<Value, ServerError>;
    fn provider_credentials_get(&self, provider_id: &str) -> Result<Value, ServerError>;
    fn provider_credentials_update(
        &self,
        provider_id: &str,
        payload: Value,
    ) -> Result<Value, ServerError>;
    fn auth_capture_policy_get(&self) -> Result<Value, ServerError>;
    fn auth_capture_policy_update(&self, payload: Value) -> Result<Value, ServerError>;
    fn replication_dlq_list(&self) -> Result<ReplicationDlqListPayload, ServerError>;
    fn replication_retry_job(&self, job_id: u64) -> Result<ReplicationRetryPayload, ServerError>;
    fn replication_dlq_replay_job(
        &self,
        job_id: u64,
    ) -> Result<ReplicationDlqReplayPayload, ServerError>;
    fn replication_dlq_replay_target(
        &self,
        target: &str,
    ) -> Result<ReplicationDlqTargetReplayPayload, ServerError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderListResult {
    pub providers: Vec<ProviderSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub provider_id: String,
    pub display_name: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderHealthResult {
    pub provider_id: String,
    pub healthy: bool,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationStatusResult {
    pub healthy: bool,
    pub pending_jobs: u64,
    pub failed_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedJobsResult {
    pub jobs: Vec<FailedJobSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedJobSummary {
    pub job_id: String,
    pub object_key: String,
    pub failure_code: String,
    pub last_attempt_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentConfigSummary {
    pub base_url: String,
    pub status_path: String,
    pub timeout_ms: u64,
    pub max_retries: usize,
    pub api_key_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketListResult {
    pub buckets: Vec<BucketSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketSummary {
    pub bucket: String,
    pub region: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertListResult {
    pub alerts: Vec<AlertSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertSummary {
    pub alert_id: String,
    pub severity: String,
    pub summary: String,
    pub created_at_unix_ms: u64,
}

#[derive(Default)]
pub struct StubControlPlaneClient;

impl ControlPlaneClient for StubControlPlaneClient {
    fn provider_list(&self) -> Result<ProviderListResult, ServerError> {
        Ok(ProviderListResult { providers: vec![] })
    }

    fn provider_health(&self, provider_id: &str) -> Result<ProviderHealthResult, ServerError> {
        Ok(ProviderHealthResult {
            provider_id: provider_id.to_string(),
            healthy: false,
            status: "not_implemented".to_string(),
        })
    }

    fn replication_get_status(&self) -> Result<ReplicationStatusResult, ServerError> {
        Ok(ReplicationStatusResult {
            healthy: false,
            pending_jobs: 0,
            failed_jobs: 0,
        })
    }

    fn replication_list_failed_jobs(&self, _limit: usize) -> Result<FailedJobsResult, ServerError> {
        Ok(FailedJobsResult { jobs: vec![] })
    }

    fn deployment_config_summary(&self) -> Result<DeploymentConfigSummary, ServerError> {
        let defaults = HttpControlPlaneClientConfig::default();
        Ok(DeploymentConfigSummary {
            base_url: defaults.base_url,
            status_path: defaults.status_path,
            timeout_ms: defaults.timeout.as_millis() as u64,
            max_retries: defaults.max_retries,
            api_key_present: defaults.api_key.is_some(),
        })
    }

    fn s3_list_buckets(&self) -> Result<BucketListResult, ServerError> {
        Ok(BucketListResult { buckets: vec![] })
    }

    fn alerts_list_recent(&self, _limit: usize) -> Result<AlertListResult, ServerError> {
        Ok(AlertListResult { alerts: vec![] })
    }

    fn admin_status_get(&self) -> Result<Value, ServerError> {
        Ok(Value::Object(Default::default()))
    }

    fn applications_get(&self) -> Result<Value, ServerError> {
        Ok(Value::Object(Default::default()))
    }

    fn applications_update(&self, payload: Value) -> Result<Value, ServerError> {
        Ok(payload)
    }

    fn content_policies_get(&self) -> Result<Value, ServerError> {
        Ok(Value::Object(Default::default()))
    }

    fn content_policies_update(&self, payload: Value) -> Result<Value, ServerError> {
        Ok(payload)
    }

    fn topology_update(&self, payload: Value) -> Result<Value, ServerError> {
        Ok(payload)
    }

    fn provider_credentials_get(&self, provider_id: &str) -> Result<Value, ServerError> {
        Ok(serde_json::json!({
            "provider": provider_id,
            "token_present": false,
        }))
    }

    fn provider_credentials_update(
        &self,
        provider_id: &str,
        payload: Value,
    ) -> Result<Value, ServerError> {
        Ok(serde_json::json!({
            "provider": provider_id,
            "payload": payload,
        }))
    }

    fn auth_capture_policy_get(&self) -> Result<Value, ServerError> {
        Ok(Value::Object(Default::default()))
    }

    fn auth_capture_policy_update(&self, payload: Value) -> Result<Value, ServerError> {
        Ok(payload)
    }

    fn replication_dlq_list(&self) -> Result<ReplicationDlqListPayload, ServerError> {
        Ok(ReplicationDlqListPayload {
            entries: vec![],
            open_count: 0,
            returned_count: 0,
        })
    }

    fn replication_retry_job(&self, job_id: u64) -> Result<ReplicationRetryPayload, ServerError> {
        Ok(ReplicationRetryPayload {
            job_id,
            status: "not_implemented".to_string(),
            target: "unknown".to_string(),
            bucket: String::new(),
            key: String::new(),
        })
    }

    fn replication_dlq_replay_job(
        &self,
        job_id: u64,
    ) -> Result<ReplicationDlqReplayPayload, ServerError> {
        Ok(ReplicationDlqReplayPayload {
            original_job_id: job_id,
            replayed_job_id: job_id,
            status: "not_implemented".to_string(),
            target: "unknown".to_string(),
            bucket: String::new(),
            key: String::new(),
        })
    }

    fn replication_dlq_replay_target(
        &self,
        target: &str,
    ) -> Result<ReplicationDlqTargetReplayPayload, ServerError> {
        Ok(ReplicationDlqTargetReplayPayload {
            target: target.to_string(),
            replayed_jobs: 0,
            jobs: vec![],
        })
    }
}

#[derive(Debug, Clone)]
pub struct HttpControlPlaneClientConfig {
    pub base_url: String,
    pub status_path: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    pub max_retries: usize,
}

impl Default for HttpControlPlaneClientConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:61081".to_string(),
            status_path: ROUTE_STATUS.to_string(),
            api_key: None,
            timeout: Duration::from_secs(2),
            max_retries: 2,
        }
    }
}

impl HttpControlPlaneClientConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_lookup(|key| std::env::var(key).ok())
    }

    fn from_env_lookup<F>(lookup: F) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        if let Some(base_url) = lookup("MCP_CONTROL_BASE_URL") {
            cfg.base_url = normalize_base_url(&base_url)?;
        }
        if let Some(status_path) = lookup("MCP_CONTROL_STATUS_PATH") {
            cfg.status_path = normalize_status_path(&status_path)?;
        }
        if let Some(api_key) =
            lookup("MCP_CONTROL_API_KEY").or_else(|| lookup("CCBG_CONTROL_API_KEY"))
        {
            cfg.api_key = normalize_api_key(&api_key);
        }
        if let Some(timeout_ms) = lookup("MCP_CONTROL_TIMEOUT_MS") {
            let parsed = timeout_ms.parse::<u64>().map_err(|err| {
                format!("invalid MCP_CONTROL_TIMEOUT_MS, expected u64 milliseconds: {err}")
            })?;
            cfg.timeout = Duration::from_millis(parsed);
        }
        if let Some(max_retries) = lookup("MCP_CONTROL_MAX_RETRIES") {
            cfg.max_retries = max_retries
                .parse::<usize>()
                .map_err(|err| format!("invalid MCP_CONTROL_MAX_RETRIES, expected usize: {err}"))?;
        }
        Ok(cfg)
    }
}

fn normalize_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return Err("MCP_CONTROL_BASE_URL cannot be empty".to_string());
    }
    let parsed = reqwest::Url::parse(trimmed)
        .map_err(|err| format!("invalid MCP_CONTROL_BASE_URL: {err}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("MCP_CONTROL_BASE_URL must use http or https".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("MCP_CONTROL_BASE_URL must not contain credentials".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("MCP_CONTROL_BASE_URL must not contain query or fragment".to_string());
    }
    Ok(trimmed.trim_end_matches('/').to_string())
}

fn normalize_api_key(api_key: &str) -> Option<String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum TransportMethod {
    Get,
    Post,
}

#[derive(Debug, Clone)]
pub struct TransportRequest {
    pub method: TransportMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub timeout: Duration,
    pub body: Option<Vec<u8>>,
    pub retryable: bool,
}

#[derive(Debug, Clone)]
pub struct TransportResponse {
    pub status_code: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum TransportError {
    Timeout,
    Unavailable(String),
    Other(String),
}

pub trait ControlPlaneTransport: Send + Sync {
    fn send(&self, request: &TransportRequest) -> Result<TransportResponse, TransportError>;
}

pub struct ReqwestControlPlaneTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestControlPlaneTransport {
    pub fn new() -> Result<Self, ServerError> {
        let client = reqwest::blocking::Client::builder()
            .build()
            .map_err(|e| ServerError::Internal(format!("failed to construct http client: {e}")))?;
        Ok(Self { client })
    }
}

impl ControlPlaneTransport for ReqwestControlPlaneTransport {
    fn send(&self, request: &TransportRequest) -> Result<TransportResponse, TransportError> {
        let mut req = match request.method {
            TransportMethod::Get => self.client.get(&request.url),
            TransportMethod::Post => self.client.post(&request.url),
        }
        .timeout(request.timeout);
        for (name, value) in &request.headers {
            req = req.header(name, value);
        }
        if let Some(body) = &request.body {
            req = req.body(body.clone());
        }
        let response = req.send().map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout
            } else if e.is_connect() || e.is_request() {
                TransportError::Unavailable(e.to_string())
            } else {
                TransportError::Other(e.to_string())
            }
        })?;
        let status_code = response.status().as_u16();
        let body = response
            .bytes()
            .map_err(|e| TransportError::Other(e.to_string()))?
            .to_vec();
        Ok(TransportResponse { status_code, body })
    }
}

pub struct HttpControlPlaneClient<T: ControlPlaneTransport> {
    config: HttpControlPlaneClientConfig,
    transport: Arc<T>,
}

impl<T: ControlPlaneTransport> HttpControlPlaneClient<T> {
    pub fn new(config: HttpControlPlaneClientConfig, transport: Arc<T>) -> Self {
        Self { config, transport }
    }

    fn fetch_status_document(&self) -> Result<AdminStatusDocument, ServerError> {
        self.get_json(&self.config.status_path, "status document")
    }

    fn admin_headers(&self, include_json_content_type: bool) -> Vec<(String, String)> {
        let mut headers = vec![(
            "x-admin-api-version".to_string(),
            ADMIN_API_VERSION.to_string(),
        )];
        if let Some(api_key) = self.config.api_key.as_deref() {
            headers.push(("x-api-key".to_string(), api_key.to_string()));
        }
        if include_json_content_type {
            headers.push(("content-type".to_string(), "application/json".to_string()));
        }
        headers
    }

    fn build_request(
        &self,
        method: TransportMethod,
        path: &str,
        body: Option<Vec<u8>>,
        retryable: bool,
    ) -> TransportRequest {
        let base = self.config.base_url.trim_end_matches('/');
        let normalized_path = path.trim_start_matches('/');
        TransportRequest {
            method,
            url: format!("{base}/{normalized_path}"),
            headers: self.admin_headers(body.is_some()),
            timeout: self.config.timeout,
            body,
            retryable,
        }
    }

    fn send_request(&self, request: TransportRequest) -> Result<TransportResponse, ServerError> {
        let mut attempts = 0usize;
        loop {
            let result = self.transport.send(&request);
            match result {
                Ok(resp) => {
                    if !(200..=299).contains(&resp.status_code) {
                        let error = map_http_error(resp.status_code, &resp.body);
                        if request.retryable
                            && is_retryable_http_status(resp.status_code)
                            && attempts < self.config.max_retries
                        {
                            attempts += 1;
                            continue;
                        }
                        return Err(error);
                    }
                    return Ok(resp);
                }
                Err(TransportError::Timeout) => {
                    if request.retryable && attempts < self.config.max_retries {
                        attempts += 1;
                        continue;
                    }
                    return Err(ServerError::Timeout("control API timeout".into()));
                }
                Err(TransportError::Unavailable(msg)) => {
                    if request.retryable && attempts < self.config.max_retries {
                        attempts += 1;
                        continue;
                    }
                    return Err(ServerError::UpstreamUnavailable(format!(
                        "control API unavailable: {msg}"
                    )));
                }
                Err(TransportError::Other(msg)) => {
                    return Err(ServerError::Internal(format!(
                        "control API transport error: {msg}"
                    )));
                }
            }
        }
    }

    fn get_json<R: DeserializeOwned>(&self, path: &str, label: &str) -> Result<R, ServerError> {
        let request = self.build_request(TransportMethod::Get, path, None, true);
        let response = self.send_request(request)?;
        serde_json::from_slice::<R>(&response.body)
            .map_err(|err| ServerError::BadRequest(format!("invalid {label} json: {err}")))
    }

    fn get_json_value(&self, path: &str, label: &str) -> Result<Value, ServerError> {
        self.get_json(path, label)
    }

    fn post_json_value(
        &self,
        path: &str,
        payload: &Value,
        label: &str,
    ) -> Result<Value, ServerError> {
        let body = serde_json::to_vec(payload).map_err(|err| {
            ServerError::Internal(format!("failed to encode {label} request: {err}"))
        })?;
        let request = self.build_request(TransportMethod::Post, path, Some(body), false);
        let response = self.send_request(request)?;
        serde_json::from_slice::<Value>(&response.body)
            .map_err(|err| ServerError::BadRequest(format!("invalid {label} json: {err}")))
    }

    fn post_empty_json<R: DeserializeOwned>(
        &self,
        path: &str,
        label: &str,
    ) -> Result<R, ServerError> {
        let request = self.build_request(TransportMethod::Post, path, None, false);
        let response = self.send_request(request)?;
        serde_json::from_slice::<R>(&response.body)
            .map_err(|err| ServerError::BadRequest(format!("invalid {label} json: {err}")))
    }
}

fn normalize_status_path(path: &str) -> Result<String, String> {
    let normalized = path.trim();
    if normalized.is_empty() {
        return Err("MCP_CONTROL_STATUS_PATH cannot be empty".to_string());
    }
    if normalized.starts_with('/') {
        Ok(normalized.to_string())
    } else {
        Ok(format!("/{}", normalized))
    }
}

fn map_http_error(status_code: u16, body: &[u8]) -> ServerError {
    let payload = serde_json::from_slice::<AdminApiErrorResponse>(body).ok();
    let message = payload
        .as_ref()
        .map(|item| item.error.trim())
        .filter(|item| !item.is_empty())
        .unwrap_or("control API request failed");
    match status_code {
        400 => ServerError::BadRequest(message.to_string()),
        401 | 403 => ServerError::Unauthorized(message.to_string()),
        404 => ServerError::NotFound(message.to_string()),
        501 if payload.as_ref().and_then(|item| item.code)
            == Some(AdminApiErrorCode::NotImplemented) =>
        {
            ServerError::NotImplemented(message.to_string())
        }
        500..=599 => ServerError::UpstreamUnavailable(message.to_string()),
        _ => ServerError::Internal(format!("unexpected control API status {status_code}")),
    }
}

fn is_retryable_http_status(status_code: u16) -> bool {
    matches!(status_code, 502 | 503 | 504)
}

impl<T: ControlPlaneTransport> ControlPlaneClient for HttpControlPlaneClient<T> {
    fn provider_list(&self) -> Result<ProviderListResult, ServerError> {
        let status = self.fetch_status_document()?;
        let providers = status
            .provider_health
            .into_iter()
            .filter_map(|entry| {
                let provider = entry.provider?;
                let health_status = entry.health.as_ref().and_then(|h| h.status.as_deref());
                let healthy = health_status
                    .map(|s| s.eq_ignore_ascii_case("healthy"))
                    .unwrap_or(false);
                let display_name = entry
                    .provider_label
                    .or_else(|| entry.health.and_then(|h| h.backend))
                    .unwrap_or_else(|| provider.clone());
                Some(ProviderSummary {
                    provider_id: provider,
                    display_name,
                    healthy,
                })
            })
            .collect();
        Ok(ProviderListResult { providers })
    }

    fn provider_health(&self, provider_id: &str) -> Result<ProviderHealthResult, ServerError> {
        let status = self.fetch_status_document()?;
        let entry = status
            .provider_health
            .into_iter()
            .find(|entry| entry.provider.as_deref() == Some(provider_id))
            .ok_or_else(|| ServerError::NotFound(format!("provider not found: {provider_id}")))?;
        let health_status = entry
            .health
            .as_ref()
            .and_then(|h| h.status.as_deref())
            .unwrap_or("unknown")
            .to_string();
        Ok(ProviderHealthResult {
            provider_id: provider_id.to_string(),
            healthy: health_status.eq_ignore_ascii_case("healthy"),
            status: health_status,
        })
    }

    fn replication_get_status(&self) -> Result<ReplicationStatusResult, ServerError> {
        let status = self.fetch_status_document()?;
        let persisted = status
            .replication_state
            .as_ref()
            .and_then(|r| r.persisted.as_ref());
        let monitoring = status
            .monitoring
            .as_ref()
            .and_then(|m| m.replication.as_ref());
        let pending_jobs = monitoring
            .and_then(|m| m.pending_jobs)
            .or_else(|| persisted.and_then(|p| p.pending_count))
            .unwrap_or(0);
        let failed_jobs = monitoring
            .and_then(|m| m.failed_jobs)
            .or_else(|| persisted.and_then(|p| p.failed_count))
            .unwrap_or(0);
        let healthy = persisted
            .and_then(|p| p.healthy)
            .or_else(|| monitoring.and_then(|m| m.healthy))
            .unwrap_or(failed_jobs == 0);
        Ok(ReplicationStatusResult {
            healthy,
            pending_jobs,
            failed_jobs,
        })
    }

    fn replication_list_failed_jobs(&self, limit: usize) -> Result<FailedJobsResult, ServerError> {
        let status = self.fetch_status_document()?;
        let jobs = status
            .replication_state
            .and_then(|r| r.latest_failed_jobs)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|job| {
                let job_id = job.job_id?;
                let object_key = match job.object {
                    Some(object) => match (object.bucket, object.key) {
                        (Some(bucket), Some(key)) if !bucket.is_empty() => {
                            format!("{bucket}/{key}")
                        }
                        (_, Some(key)) => key,
                        _ => job.target.unwrap_or_else(|| "unknown_object".to_string()),
                    },
                    None => job.target.unwrap_or_else(|| "unknown_object".to_string()),
                };
                let failure_code = job
                    .last_error
                    .unwrap_or_else(|| "unknown_failure".to_string());
                let last_attempt_unix_ms = job
                    .next_attempt_at_unix_ms
                    .or(job.enqueued_at_unix_ms)
                    .unwrap_or(0);
                Some(FailedJobSummary {
                    job_id,
                    object_key,
                    failure_code,
                    last_attempt_unix_ms,
                })
            })
            .take(limit)
            .collect();
        Ok(FailedJobsResult { jobs })
    }

    fn s3_list_buckets(&self) -> Result<BucketListResult, ServerError> {
        Ok(BucketListResult { buckets: vec![] })
    }

    fn deployment_config_summary(&self) -> Result<DeploymentConfigSummary, ServerError> {
        Ok(DeploymentConfigSummary {
            base_url: self.config.base_url.clone(),
            status_path: self.config.status_path.clone(),
            timeout_ms: self.config.timeout.as_millis() as u64,
            max_retries: self.config.max_retries,
            api_key_present: self.config.api_key.is_some(),
        })
    }

    fn alerts_list_recent(&self, limit: usize) -> Result<AlertListResult, ServerError> {
        let status = self.fetch_status_document()?;
        let alerts = status
            .alerts
            .into_iter()
            .map(|alert| {
                let title = alert.title.unwrap_or_default();
                let detail = alert.detail.unwrap_or_default();
                let summary = if !title.is_empty() && !detail.is_empty() {
                    format!("{title}: {detail}")
                } else if !title.is_empty() {
                    title
                } else {
                    detail
                };
                AlertSummary {
                    alert_id: alert.id.unwrap_or_default(),
                    severity: alert.severity.unwrap_or_else(|| "unknown".to_string()),
                    summary,
                    created_at_unix_ms: alert.created_at_unix_ms.unwrap_or(0),
                }
            })
            .take(limit)
            .collect();
        Ok(AlertListResult { alerts })
    }

    fn admin_status_get(&self) -> Result<Value, ServerError> {
        self.get_json_value(&self.config.status_path, "admin status")
    }

    fn applications_get(&self) -> Result<Value, ServerError> {
        self.get_json_value(ROUTE_APPLICATIONS, "applications")
    }

    fn applications_update(&self, payload: Value) -> Result<Value, ServerError> {
        self.post_json_value(ROUTE_APPLICATIONS, &payload, "applications")
    }

    fn content_policies_get(&self) -> Result<Value, ServerError> {
        self.get_json_value(ROUTE_CONTENT_POLICIES, "content policies")
    }

    fn content_policies_update(&self, payload: Value) -> Result<Value, ServerError> {
        self.post_json_value(ROUTE_CONTENT_POLICIES, &payload, "content policies")
    }

    fn topology_update(&self, payload: Value) -> Result<Value, ServerError> {
        self.post_json_value(ROUTE_TOPOLOGY_UPDATE, &payload, "topology update")
    }

    fn provider_credentials_get(&self, provider_id: &str) -> Result<Value, ServerError> {
        let path = route_with_param(ROUTE_PROVIDER_CREDENTIALS, "{provider}", provider_id);
        self.get_json_value(&path, "provider credentials")
    }

    fn provider_credentials_update(
        &self,
        provider_id: &str,
        payload: Value,
    ) -> Result<Value, ServerError> {
        let path = route_with_param(ROUTE_PROVIDER_CREDENTIALS, "{provider}", provider_id);
        self.post_json_value(&path, &payload, "provider credentials")
    }

    fn auth_capture_policy_get(&self) -> Result<Value, ServerError> {
        self.get_json_value(ROUTE_AUTH_CAPTURE_POLICY, "auth capture policy")
    }

    fn auth_capture_policy_update(&self, payload: Value) -> Result<Value, ServerError> {
        self.post_json_value(ROUTE_AUTH_CAPTURE_POLICY, &payload, "auth capture policy")
    }

    fn replication_dlq_list(&self) -> Result<ReplicationDlqListPayload, ServerError> {
        self.get_json(ROUTE_REPLICATION_DLQ, "replication dlq")
    }

    fn replication_retry_job(&self, job_id: u64) -> Result<ReplicationRetryPayload, ServerError> {
        let path = route_with_param(ROUTE_REPLICATION_RETRY_JOB, "{job_id}", &job_id.to_string());
        self.post_empty_json(&path, "replication retry job")
    }

    fn replication_dlq_replay_job(
        &self,
        job_id: u64,
    ) -> Result<ReplicationDlqReplayPayload, ServerError> {
        let path = route_with_param(
            ROUTE_REPLICATION_DLQ_REPLAY_JOB,
            "{job_id}",
            &job_id.to_string(),
        );
        self.post_empty_json(&path, "replication dlq replay job")
    }

    fn replication_dlq_replay_target(
        &self,
        target: &str,
    ) -> Result<ReplicationDlqTargetReplayPayload, ServerError> {
        let path = route_with_param(ROUTE_REPLICATION_DLQ_REPLAY_TARGET, "{target}", target);
        self.post_empty_json(&path, "replication dlq replay target")
    }
}

fn route_with_param(template: &str, placeholder: &str, value: &str) -> String {
    let encoded = utf8_percent_encode(value, NON_ALPHANUMERIC).to_string();
    template.replace(placeholder, &encoded)
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminStatusDocument {
    #[serde(default)]
    provider_health: Vec<AdminProviderHealthEntry>,
    #[serde(default)]
    replication_state: Option<AdminReplicationState>,
    #[serde(default)]
    monitoring: Option<AdminMonitoring>,
    #[serde(default)]
    alerts: Vec<AdminAlertPayload>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminProviderHealthEntry {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    provider_label: Option<String>,
    #[serde(default)]
    health: Option<AdminProviderHealth>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminProviderHealth {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    backend: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminReplicationState {
    #[serde(default)]
    persisted: Option<AdminReplicationPersisted>,
    #[serde(default)]
    latest_failed_jobs: Option<Vec<AdminReplicationJob>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminReplicationPersisted {
    #[serde(default)]
    pending_count: Option<u64>,
    #[serde(default)]
    failed_count: Option<u64>,
    #[serde(default)]
    healthy: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminReplicationJob {
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    object: Option<AdminReplicationObjectRef>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    enqueued_at_unix_ms: Option<u64>,
    #[serde(default)]
    next_attempt_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminReplicationObjectRef {
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminMonitoring {
    #[serde(default)]
    replication: Option<AdminMonitoringReplication>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminMonitoringReplication {
    #[serde(default)]
    pending_jobs: Option<u64>,
    #[serde(default)]
    failed_jobs: Option<u64>,
    #[serde(default)]
    healthy: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct AdminAlertPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    created_at_unix_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::{
        AlertListResult, BucketListResult, ControlPlaneClient, ControlPlaneTransport,
        DeploymentConfigSummary, FailedJobsResult, HttpControlPlaneClient,
        HttpControlPlaneClientConfig, ProviderListResult, ReplicationStatusResult,
        ReqwestControlPlaneTransport, TransportError, TransportMethod, TransportRequest,
        TransportResponse,
    };
    use crate::error::{ErrorCode, ServerError};
    use admin_api::{ADMIN_API_VERSION, ROUTE_STATUS};
    use serde_json::{Value, json};
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    #[derive(Default)]
    struct MockTransport {
        calls: Mutex<Vec<TransportRequest>>,
        responses: Mutex<VecDeque<Result<TransportResponse, TransportError>>>,
    }

    impl MockTransport {
        fn with_responses(responses: Vec<Result<TransportResponse, TransportError>>) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(VecDeque::from(responses)),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().expect("calls lock").len()
        }
    }

    impl ControlPlaneTransport for MockTransport {
        fn send(&self, request: &TransportRequest) -> Result<TransportResponse, TransportError> {
            self.calls.lock().expect("calls lock").push(request.clone());
            self.responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .expect("scripted response")
        }
    }

    fn ok_response(value: serde_json::Value) -> Result<TransportResponse, TransportError> {
        Ok(TransportResponse {
            status_code: 200,
            body: serde_json::to_vec(&value).expect("json body"),
        })
    }

    fn client_with_transport(
        transport: Arc<MockTransport>,
        max_retries: usize,
    ) -> HttpControlPlaneClient<MockTransport> {
        HttpControlPlaneClient::new(
            HttpControlPlaneClientConfig {
                base_url: "http://control.example:8080".to_string(),
                status_path: ROUTE_STATUS.to_string(),
                api_key: Some("test-key".to_string()),
                timeout: Duration::from_millis(750),
                max_retries,
            },
            transport,
        )
    }

    fn request_header<'a>(request: &'a TransportRequest, name: &str) -> Option<&'a str> {
        request
            .headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn transport_receives_status_path_and_api_key() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "provider_health": []
        }))]));
        let client = client_with_transport(Arc::clone(&transport), 0);
        let _: ProviderListResult = client.provider_list().expect("provider_list");
        let call = transport.calls.lock().expect("calls lock")[0].clone();
        assert!(matches!(call.method, TransportMethod::Get));
        assert_eq!(call.url, "http://control.example:8080/api/status");
        assert_eq!(request_header(&call, "x-api-key"), Some("test-key"));
        assert_eq!(
            request_header(&call, "x-admin-api-version"),
            Some(ADMIN_API_VERSION)
        );
        assert_eq!(call.timeout, Duration::from_millis(750));
        assert!(call.body.is_none());
        assert!(call.retryable);
    }

    #[test]
    fn reqwest_transport_sends_contract_headers_to_mock_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buf = [0u8; 4096];
            let read = stream.read(&mut buf).expect("read request");
            let raw_request = String::from_utf8_lossy(&buf[..read]).to_string();
            let body = r#"{"provider_health":[{"provider":"mobile","provider_label":"CMCC","health":{"status":"Healthy"}}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
            raw_request
        });

        let transport = Arc::new(ReqwestControlPlaneTransport::new().expect("reqwest transport"));
        let client = HttpControlPlaneClient::new(
            HttpControlPlaneClientConfig {
                base_url: format!("http://{addr}"),
                status_path: ROUTE_STATUS.to_string(),
                api_key: Some("test-key".to_string()),
                timeout: Duration::from_secs(2),
                max_retries: 0,
            },
            transport,
        );

        let result: ProviderListResult = client.provider_list().expect("provider list");
        assert_eq!(result.providers.len(), 1);
        let raw_request = server.join().expect("mock server thread");
        let lower = raw_request.to_ascii_lowercase();
        assert!(lower.starts_with("get /api/status http/1.1"));
        assert!(lower.contains("x-api-key: test-key"));
        assert!(lower.contains(&format!("x-admin-api-version: {ADMIN_API_VERSION}")));
    }

    #[test]
    fn provider_list_maps_typed_nested_health() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "provider_health": [
                {"provider":"mobile","provider_label":"CMCC","health":{"status":"Healthy","backend":"mobile"}},
                {"provider":"telecom","health":{"status":"Degraded","backend":"telecom"}}
            ]
        }))]));
        let client = client_with_transport(transport, 0);
        let result: ProviderListResult = client.provider_list().expect("provider_list");
        assert_eq!(result.providers[0].provider_id, "mobile");
        assert_eq!(result.providers[0].display_name, "CMCC");
        assert!(result.providers[0].healthy);
        assert_eq!(result.providers[1].provider_id, "telecom");
        assert_eq!(result.providers[1].display_name, "telecom");
        assert!(!result.providers[1].healthy);
    }

    #[test]
    fn provider_list_does_not_expose_sensitive_payload_fields() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "provider_health": [
                {"provider":"mobile","provider_label":"CMCC","health":{"status":"Healthy"}},
            ],
            "credentials": {"token":"never-expose-this","password":"never-expose-this"}
        }))]));
        let client = client_with_transport(transport, 0);
        let result: ProviderListResult = client.provider_list().expect("provider_list");
        let encoded = serde_json::to_string(&result).expect("json");
        assert!(!encoded.contains("never-expose-this"));
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("password"));
    }

    #[test]
    fn provider_health_missing_is_not_found() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "provider_health": [{"provider":"mobile","health":{"status":"Healthy"}}]
        }))]));
        let client = client_with_transport(transport, 0);
        let error = client
            .provider_health("telecom")
            .expect_err("expected not found");
        assert!(matches!(error, ServerError::NotFound(_)));
    }

    #[test]
    fn replication_get_status_prefers_monitoring_counters() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "replication_state": {"persisted": {"healthy": true, "pending_count": 9, "failed_count": 4}},
            "monitoring": {"replication": {"pending_jobs": 2, "failed_jobs": 1}}
        }))]));
        let client = client_with_transport(transport, 0);
        let result: ReplicationStatusResult =
            client.replication_get_status().expect("replication status");
        assert_eq!(result.pending_jobs, 2);
        assert_eq!(result.failed_jobs, 1);
        assert!(result.healthy);
    }

    #[test]
    fn replication_list_failed_jobs_respects_limit_and_sanitizes() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "replication_state": {
                "latest_failed_jobs": [
                    {"job_id":"j1","object":{"bucket":"b1","key":"k1"},"last_error":"network","enqueued_at_unix_ms":100},
                    {"job_id":"j2","object":{"key":"k2"}},
                    {"job_id":"j3","target":"legacy-target","last_error":"quota"}
                ]
            }
        }))]));
        let client = client_with_transport(transport, 0);
        let result: FailedJobsResult = client.replication_list_failed_jobs(2).expect("failed jobs");
        assert_eq!(result.jobs.len(), 2);
        assert_eq!(result.jobs[0].job_id, "j1");
        assert_eq!(result.jobs[0].object_key, "b1/k1");
        assert_eq!(result.jobs[0].failure_code, "network");
        assert_eq!(result.jobs[0].last_attempt_unix_ms, 100);
        assert_eq!(result.jobs[1].object_key, "k2");
    }

    #[test]
    fn alerts_list_recent_respects_limit() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "alerts": [
                {"id":"a1","severity":"warn","title":"Latency","detail":"Provider mobile slow"},
                {"id":"a2","severity":"error","title":"Replication","detail":"Queue stalled"}
            ]
        }))]));
        let client = client_with_transport(transport, 0);
        let result: AlertListResult = client.alerts_list_recent(1).expect("alerts");
        assert_eq!(result.alerts.len(), 1);
        assert_eq!(result.alerts[0].alert_id, "a1");
        assert_eq!(result.alerts[0].severity, "warn");
        assert_eq!(result.alerts[0].summary, "Latency: Provider mobile slow");
        assert_eq!(result.alerts[0].created_at_unix_ms, 0);
    }

    #[test]
    fn applications_update_uses_post_json_body_without_retry() {
        let transport = Arc::new(MockTransport::with_responses(vec![ok_response(json!({
            "applications": []
        }))]));
        let client = client_with_transport(Arc::clone(&transport), 3);
        let payload = json!({"applications": [{"id": "product-manager-agent"}]});
        let result = client
            .applications_update(payload.clone())
            .expect("applications update");
        assert_eq!(result, json!({"applications": []}));

        let call = transport.calls.lock().expect("calls lock")[0].clone();
        assert!(matches!(call.method, TransportMethod::Post));
        assert_eq!(call.url, "http://control.example:8080/api/applications");
        assert!(!call.retryable);
        let body = call.body.as_ref().expect("body");
        let decoded: Value = serde_json::from_slice(body).expect("json body");
        assert_eq!(decoded, payload);
        assert_eq!(
            request_header(&call, "content-type"),
            Some("application/json")
        );
    }

    #[test]
    fn mutating_post_is_not_retried_on_timeout() {
        let transport = Arc::new(MockTransport::with_responses(vec![Err(
            TransportError::Timeout,
        )]));
        let client = client_with_transport(Arc::clone(&transport), 3);
        let error = client
            .applications_update(json!({"applications": []}))
            .expect_err("timeout");
        assert!(matches!(error, ServerError::Timeout(_)));
        assert_eq!(transport.call_count(), 1);
    }

    #[test]
    fn s3_list_buckets_safe_fallback_is_empty_and_no_transport_call() {
        let transport = Arc::new(MockTransport::with_responses(vec![]));
        let client = client_with_transport(Arc::clone(&transport), 0);
        let result: BucketListResult = client.s3_list_buckets().expect("s3 list");
        assert!(result.buckets.is_empty());
        assert_eq!(transport.call_count(), 0);
    }

    #[test]
    fn unauthorized_is_not_retried() {
        let transport = Arc::new(MockTransport::with_responses(vec![Ok(TransportResponse {
            status_code: 401,
            body: b"{}".to_vec(),
        })]));
        let client = client_with_transport(Arc::clone(&transport), 3);
        let error = client.provider_list().expect_err("unauthorized");
        assert!(matches!(error, ServerError::Unauthorized(_)));
        assert_eq!(transport.call_count(), 1);
        assert_eq!(error.to_payload().code, ErrorCode::Unauthorized);
    }

    #[test]
    fn service_unavailable_is_retried() {
        let transport = Arc::new(MockTransport::with_responses(vec![
            Ok(TransportResponse {
                status_code: 503,
                body: b"{}".to_vec(),
            }),
            ok_response(json!({"provider_health": []})),
        ]));
        let client = client_with_transport(Arc::clone(&transport), 1);
        let result: ProviderListResult = client.provider_list().expect("provider list");
        assert_eq!(transport.call_count(), 2);
        assert!(result.providers.is_empty());
    }

    #[test]
    fn service_unavailable_stops_after_retry_budget() {
        let transport = Arc::new(MockTransport::with_responses(vec![
            Ok(TransportResponse {
                status_code: 503,
                body: serde_json::to_vec(&json!({
                    "error": "temporarily unavailable",
                    "code": "service_unavailable",
                    "api_version": ADMIN_API_VERSION
                }))
                .expect("json"),
            }),
            Ok(TransportResponse {
                status_code: 503,
                body: b"{}".to_vec(),
            }),
        ]));
        let client = client_with_transport(Arc::clone(&transport), 1);
        let error = client.provider_list().expect_err("503 after retries");
        assert!(matches!(error, ServerError::UpstreamUnavailable(_)));
        assert_eq!(transport.call_count(), 2);
    }

    #[test]
    fn timeout_is_retried_then_timeout_error() {
        let transport = Arc::new(MockTransport::with_responses(vec![
            Err(TransportError::Timeout),
            Err(TransportError::Timeout),
        ]));
        let client = client_with_transport(Arc::clone(&transport), 1);
        let error = client.provider_list().expect_err("timeout");
        assert!(matches!(error, ServerError::Timeout(_)));
        assert_eq!(transport.call_count(), 2);
    }

    #[test]
    fn malformed_json_maps_to_bad_request_non_retryable() {
        let transport = Arc::new(MockTransport::with_responses(vec![Ok(TransportResponse {
            status_code: 200,
            body: b"{not-json".to_vec(),
        })]));
        let client = client_with_transport(transport, 0);
        let error = client.provider_list().expect_err("invalid json");
        assert!(matches!(error, ServerError::BadRequest(_)));
        let payload = error.to_payload();
        assert_eq!(payload.code, ErrorCode::BadRequest);
        assert!(!payload.retryable);
    }

    #[test]
    fn deployment_config_summary_is_sanitized() {
        let transport = Arc::new(MockTransport::with_responses(vec![]));
        let client = client_with_transport(transport, 3);
        let summary: DeploymentConfigSummary = client
            .deployment_config_summary()
            .expect("deployment config summary");
        assert_eq!(summary.base_url, "http://control.example:8080");
        assert_eq!(summary.status_path, "/api/status");
        assert_eq!(summary.timeout_ms, 750);
        assert_eq!(summary.max_retries, 3);
        assert!(summary.api_key_present);

        let encoded = serde_json::to_string(&summary).expect("serialize");
        assert!(!encoded.contains("test-key"));
    }

    #[test]
    fn from_env_uses_defaults_and_overrides() {
        let defaults = HttpControlPlaneClientConfig::from_env_lookup(|_| None).expect("defaults");
        assert_eq!(defaults.base_url, "http://127.0.0.1:61081");
        assert_eq!(defaults.status_path, ROUTE_STATUS);
        assert_eq!(defaults.timeout, Duration::from_secs(2));
        assert_eq!(defaults.max_retries, 2);
        assert!(defaults.api_key.is_none());

        let cfg = HttpControlPlaneClientConfig::from_env_lookup(|key| match key {
            "MCP_CONTROL_BASE_URL" => Some("http://localhost:19000/".to_string()),
            "MCP_CONTROL_STATUS_PATH" => Some("statusz".to_string()),
            "MCP_CONTROL_API_KEY" => Some("secret-key".to_string()),
            "MCP_CONTROL_TIMEOUT_MS" => Some("1200".to_string()),
            "MCP_CONTROL_MAX_RETRIES" => Some("5".to_string()),
            _ => None,
        })
        .expect("env parse");
        assert_eq!(cfg.base_url, "http://localhost:19000");
        assert_eq!(cfg.status_path, "/statusz");
        assert_eq!(cfg.api_key.as_deref(), Some("secret-key"));
        assert_eq!(cfg.timeout, Duration::from_millis(1200));
        assert_eq!(cfg.max_retries, 5);

        let fallback_key = HttpControlPlaneClientConfig::from_env_lookup(|key| match key {
            "CCBG_CONTROL_API_KEY" => Some("gateway-control-key".to_string()),
            _ => None,
        })
        .expect("fallback env parse");
        assert_eq!(fallback_key.api_key.as_deref(), Some("gateway-control-key"));
    }

    #[test]
    fn from_env_rejects_invalid_values() {
        let timeout_err = HttpControlPlaneClientConfig::from_env_lookup(|key| match key {
            "MCP_CONTROL_TIMEOUT_MS" => Some("abc".to_string()),
            _ => None,
        })
        .expect_err("invalid timeout");
        assert!(timeout_err.contains("MCP_CONTROL_TIMEOUT_MS"));

        let path_err = HttpControlPlaneClientConfig::from_env_lookup(|key| match key {
            "MCP_CONTROL_STATUS_PATH" => Some("  ".to_string()),
            _ => None,
        })
        .expect_err("invalid path");
        assert!(path_err.contains("MCP_CONTROL_STATUS_PATH"));

        let base_err = HttpControlPlaneClientConfig::from_env_lookup(|key| match key {
            "MCP_CONTROL_BASE_URL" => Some("http://user:pass@localhost:61081".to_string()),
            _ => None,
        })
        .expect_err("base url credentials");
        assert!(base_err.contains("MCP_CONTROL_BASE_URL"));
    }

    #[test]
    fn unauthorized_error_message_uses_admin_error_payload() {
        let transport = Arc::new(MockTransport::with_responses(vec![Ok(TransportResponse {
            status_code: 401,
            body: serde_json::to_vec(&json!({
                "error": "login required",
                "code": "unauthorized",
                "api_version": ADMIN_API_VERSION
            }))
            .expect("json"),
        })]));
        let client = client_with_transport(transport, 1);
        let error = client.provider_list().expect_err("unauthorized");
        match error {
            ServerError::Unauthorized(msg) => assert_eq!(msg, "login required"),
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn forbidden_maps_to_non_retryable_auth_error() {
        let transport = Arc::new(MockTransport::with_responses(vec![Ok(TransportResponse {
            status_code: 403,
            body: serde_json::to_vec(&json!({
                "error": "operator access denied",
                "code": "forbidden",
                "api_version": ADMIN_API_VERSION
            }))
            .expect("json"),
        })]));
        let client = client_with_transport(transport, 1);
        let error = client.provider_list().expect_err("forbidden");
        let payload = error.to_payload();
        match error {
            ServerError::Unauthorized(msg) => assert_eq!(msg, "operator access denied"),
            other => panic!("unexpected error {other:?}"),
        }
        assert!(!payload.retryable);
    }

    #[test]
    fn server_error_status_uses_admin_error_payload_as_upstream() {
        let transport = Arc::new(MockTransport::with_responses(vec![Ok(TransportResponse {
            status_code: 500,
            body: serde_json::to_vec(&json!({
                "error": "replication monitor unavailable",
                "code": "internal",
                "api_version": ADMIN_API_VERSION
            }))
            .expect("json"),
        })]));
        let client = client_with_transport(transport, 0);
        let error = client.provider_list().expect_err("server error");
        let payload = error.to_payload();
        match error {
            ServerError::UpstreamUnavailable(msg) => {
                assert_eq!(msg, "replication monitor unavailable")
            }
            other => panic!("unexpected error {other:?}"),
        }
        assert!(payload.retryable);
    }
}
