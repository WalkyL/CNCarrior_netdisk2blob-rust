use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env, fs,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
#[cfg(test)]
use axum::body::Bytes;
#[cfg(test)]
use axum::http::Request;
#[cfg(test)]
use axum::routing::any;
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, OriginalUri, Path, Query, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode, Uri,
        header::{
            AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HOST, LAST_MODIFIED, LOCATION,
        },
    },
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
#[cfg(test)]
use blob_core::BrowserFlowSession;
use blob_core::{
    BlobBackend, BlobError, BrowserFlow, BrowserFlowBindingContext, BrowserFlowCatalog,
    BrowserFlowCatalogCollection, BrowserFlowExecutionReport, BrowserFlowExecutor,
    BrowserFlowInput, BrowserFlowInputKind, BrowserFlowOutputKind, BrowserFlowSessionExecutor,
    CopyObjectRequest, DryRunBrowserFlowExecutor, ListObjectsRequest, MoveObjectRequest,
    ObjectBody, OutboundIpFamily, PutObjectRequest, RenameObjectRequest, StubBackend, TokenSource,
};
use browser_cdp::{CdpBrowserFlowSession, CdpConnectionConfig};
use futures_util::{StreamExt, TryStreamExt};
#[cfg(test)]
use futures_util::stream;
use hmac::{Hmac, Mac};
use metadata_store::{
    MetadataRetentionPolicy, MetadataSnapshot, MetadataStore, MetadataStoreOptions,
    MetadataTargetStatus,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use policy_engine::{
    ProviderId, ReplicationMode, TopologyInput, TopologyPolicy, parse_provider_list,
};
use provider_mobile::{MobileBlobAdapter, MobileConfig};
use provider_onedrive::{
    DEFAULT_ONEDRIVE_AUTH_BASE_URL, DEFAULT_ONEDRIVE_SCOPES, OneDriveBlobAdapter, OneDriveConfig,
    OneDriveOAuthSession, decode_stored_oauth_session, persist_oauth_session,
};
use provider_telecom::{TelecomBlobAdapter, TelecomConfig};
use provider_unicom::{UnicomBlobAdapter, UnicomConfig};
use rand::{RngCore, rngs::OsRng};
use replication_engine::{
    ReplicationEngine, ReplicationJob, ReplicationOperation, ReplicationSnapshot, ReplicationStatus,
};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Duration, sleep};
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

type DynBackend = Arc<dyn BlobBackend>;
type HmacSha256 = Hmac<Sha256>;

const S3_NS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";
const REQUEST_ID: &str = "ccbg-local";
const DEFAULT_TIMESTAMP: &str = "1970-01-01T00:00:00.000Z";
const SOURCE_PROVIDER_HEADER: &str = "x-ccbg-source-provider";
const FALLBACK_FROM_HEADER: &str = "x-ccbg-fallback-from";
const DEFAULT_OBJECT_ACTION_HISTORY_LIMIT: usize = 12;
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const BROWSER_FLOW_AUTH_SESSION_STATUS_PENDING: &str = "pending";
const BROWSER_FLOW_AUTH_SESSION_STATUS_AWAITING_INPUT: &str = "awaiting_input";
const BROWSER_FLOW_AUTH_SESSION_STATUS_ANSWERED: &str = "answered";
const BROWSER_FLOW_AUTH_SESSION_STATUS_RESUMED: &str = "resumed";
const BROWSER_FLOW_AUTH_SESSION_STATUS_COMPLETED: &str = "completed";
const BROWSER_FLOW_AUTH_SESSION_STATUS_FAILED: &str = "failed";
const NOTIFY_SIGNATURE_VERSION_HEADER: &str = "x-ccbg-notify-signature-version";
const NOTIFY_SIGNATURE_HEADER: &str = "x-ccbg-notify-signature";
const NOTIFY_TIMESTAMP_HEADER: &str = "x-ccbg-notify-timestamp";
const NOTIFY_EVENT_ID_HEADER: &str = "x-ccbg-notify-event-id";

#[derive(Clone)]
struct AppState {
    config: Arc<AppConfig>,
    backends: Arc<Mutex<Vec<ConfiguredBackend>>>,
    replication: Arc<ReplicationEngine>,
    metadata_store: Arc<MetadataStore>,
    auth: Arc<AuthBrokerState>,
    control_plane: Arc<Mutex<ControlPlaneState>>,
    notify_state: Arc<Mutex<NotifyState>>,
    browser_flow_catalogs: Arc<BrowserFlowCatalogCollection>,
    data_plane_concurrency: Arc<DataPlaneConcurrencyState>,
    started_at_unix_ms: u64,
}

struct DataPlaneConcurrencyState {
    semaphore: Arc<Semaphore>,
}

#[derive(Clone)]
struct ConfiguredBackend {
    provider: ProviderId,
    backend: DynBackend,
}

#[derive(Debug, Clone, Copy)]
struct ReadSource {
    provider: ProviderId,
    fallback_from: Option<ProviderId>,
}

#[derive(Clone)]
struct ResolvedReadBackend {
    source: ReadSource,
    backend: DynBackend,
}

#[derive(Clone)]
struct ResolvedObjectRead {
    source: ReadSource,
    backend: DynBackend,
    object: blob_core::ObjectInfo,
}

struct ReadResult<T> {
    source: ReadSource,
    value: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FallbackObjectGate {
    Allowed,
    MissingMetadata,
    PendingPut,
    Deleted,
    PolicyBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminMode {
    Off,
    Web,
    Terminal,
}

#[derive(Clone)]
struct AuthBrokerState {
    http_client: HttpClient,
    pending_pkce: Arc<Mutex<HashMap<String, PendingPkceLogin>>>,
    device_flows: Arc<Mutex<HashMap<String, DeviceFlowRecord>>>,
    capture_prompts: Arc<Mutex<HashMap<String, AuthCapturePrompt>>>,
    browser_flow_sessions: Arc<Mutex<HashMap<String, BrowserFlowAuthSession>>>,
}

#[derive(Debug, Clone)]
struct PendingPkceLogin {
    code_verifier: String,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct OneDriveDeviceFlowPayload {
    flow_id: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    user_code: String,
    message: String,
    expires_in: u64,
    interval: u64,
    status: &'static str,
    error: Option<String>,
    completed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct DeviceFlowRecord {
    device_code: String,
    interval_secs: u64,
    expires_at_unix_ms: u64,
    payload: OneDriveDeviceFlowPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthPromptFieldKind {
    Text,
    PhoneNumber,
    SmsCode,
    Password,
    Captcha,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthCapturePrompt {
    prompt_id: String,
    provider: String,
    session_id: Option<String>,
    flow_id: Option<String>,
    input_id: Option<String>,
    title: String,
    message: String,
    field_label: String,
    field_kind: AuthPromptFieldKind,
    placeholder: Option<String>,
    status: String,
    created_at_unix_ms: u64,
    answered_at_unix_ms: Option<u64>,
    answer_present: bool,
    answer_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AuthCapturePromptCreateInput {
    provider: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    flow_id: Option<String>,
    #[serde(default)]
    input_id: Option<String>,
    title: String,
    message: String,
    field_label: String,
    field_kind: AuthPromptFieldKind,
    placeholder: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AuthCapturePromptReplyInput {
    value: String,
}

#[derive(Debug, Clone, Serialize)]
struct OneDriveAuthStatusPayload {
    enabled: bool,
    preferred_mode: &'static str,
    client_id_configured: bool,
    redirect_url: Option<String>,
    session_file: Option<String>,
    auth_base_url: String,
    scopes: String,
    token_state: &'static str,
    expires_at_unix: Option<u64>,
    has_refresh_token: bool,
    pending_pkce_logins: usize,
    pending_device_flows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OnedriveScopeMode {
    All,
    MemoryOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OnedrivePolicy {
    replication_enabled: bool,
    fallback_enabled: bool,
    scope_mode: OnedriveScopeMode,
    memory_buckets: Vec<String>,
    memory_prefixes: Vec<String>,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct OnedrivePolicyInput {
    replication_enabled: bool,
    fallback_enabled: bool,
    scope_mode: OnedriveScopeMode,
    #[serde(default)]
    memory_buckets: Vec<String>,
    #[serde(default)]
    memory_prefixes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthCapturePolicy {
    enabled: bool,
    broker_url: Option<String>,
    cdp_endpoint_url: Option<String>,
    cdp_target_selector: Option<String>,
    cdp_target_timeout_ms: Option<u64>,
    llm_analysis_enabled: bool,
    llm_endpoint: Option<String>,
    llm_model_id: Option<String>,
    llm_api_key: Option<String>,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
struct AuthCapturePolicyPayload {
    enabled: bool,
    broker_url: Option<String>,
    cdp_endpoint_url: Option<String>,
    cdp_target_selector: Option<String>,
    cdp_target_timeout_ms: Option<u64>,
    llm_analysis_enabled: bool,
    llm_endpoint: Option<String>,
    llm_model_id: Option<String>,
    llm_api_key_present: bool,
    updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct AuthCapturePolicyInput {
    enabled: bool,
    broker_url: Option<String>,
    cdp_endpoint_url: Option<String>,
    cdp_target_selector: Option<String>,
    cdp_target_timeout_ms: Option<u64>,
    llm_analysis_enabled: bool,
    llm_endpoint: Option<String>,
    llm_model_id: Option<String>,
    #[serde(default)]
    llm_api_key: Option<String>,
    #[serde(default)]
    clear_llm_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ControlPlaneState {
    topology: TopologyPolicy,
    onedrive_policy: OnedrivePolicy,
    #[serde(default = "AuthCapturePolicy::from_env_defaults")]
    auth_capture_policy: AuthCapturePolicy,
    #[serde(default)]
    object_action_history: Vec<ObjectActionHistoryEntryPayload>,
}

#[derive(Debug, Clone, Deserialize)]
struct TopologyUpdateInput {
    primary_provider: ProviderId,
    #[serde(default)]
    sync_targets: Vec<ProviderId>,
    #[serde(default)]
    fallback_read_order: Vec<ProviderId>,
}

#[derive(Debug, Serialize)]
struct RuntimeTopologyPayload {
    primary_provider: &'static str,
    sync_targets: Vec<&'static str>,
    fallback_read_order: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct DesiredTopologyPayload {
    primary_provider: &'static str,
    sync_targets: Vec<&'static str>,
    fallback_read_order: Vec<&'static str>,
    restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TopologyProviderOptionPayload {
    provider: &'static str,
    label: &'static str,
    can_be_primary: bool,
    can_be_sync_target: bool,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct AdminStatusPayload {
    runtime: RuntimeStatusPayload,
    monitoring: MonitoringSummaryPayload,
    operations_overview: OperationsOverviewPayload,
    notify: NotifyStatusPayload,
    runtime_topology: RuntimeTopologyPayload,
    desired_topology: DesiredTopologyPayload,
    replication: ReplicationQueueSummary,
    replication_state: ReplicationStatePayload,
    object_action_history: Vec<ObjectActionHistoryEntryPayload>,
    object_action_history_limit: usize,
    provider_health: Vec<BackendPayload>,
    alerts: Vec<AdminAlertPayload>,
    onedrive_auth: OneDriveAuthStatusPayload,
    onedrive_policy: OnedrivePolicy,
    auth_capture_policy: AuthCapturePolicyPayload,
    browser_flow_catalogs: Vec<BrowserFlowCatalogSummaryPayload>,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeStatusPayload {
    started_at_unix_ms: u64,
    uptime_ms: u64,
    bind_addr: String,
    admin_mode: &'static str,
    admin_bind_addr: String,
    auth_callback_bind_addr: String,
    metrics_bind_addr: String,
    control_plane_file: String,
    metadata_db_path: String,
    credentials_dir: String,
    browser_flow_catalog_dir: String,
    provider_capability_catalog_dir: String,
    replication_workers: usize,
    data_plane_max_in_flight: usize,
    object_action_history_limit: usize,
}

#[derive(Debug, Clone)]
struct NotifyState {
    last_alert_hash: Option<String>,
    last_attempt_at_unix_ms: Option<u64>,
    last_success_at_unix_ms: Option<u64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct NotifyStatusPayload {
    webhook_enabled: bool,
    webhook_url_present: bool,
    signature_enabled: bool,
    poll_interval_seconds: u64,
    last_alert_hash: Option<String>,
    last_attempt_at_unix_ms: Option<u64>,
    last_success_at_unix_ms: Option<u64>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct NotifyWebhookPayload {
    event_id: String,
    service: &'static str,
    emitted_at_unix_ms: u64,
    runtime: RuntimeStatusPayload,
    monitoring: MonitoringSummaryPayload,
    alerts: Vec<AdminAlertPayload>,
}

#[derive(Debug, Clone, Serialize)]
struct MonitoringSummaryPayload {
    open_alert_count: usize,
    provider_summary: MonitoringProviderSummaryPayload,
    replication: MonitoringReplicationSummaryPayload,
    object_actions: MonitoringObjectActionSummaryPayload,
    latest_failed_objects: Vec<MonitoringFailurePayload>,
    recent_failures: Vec<MonitoringFailurePayload>,
}

#[derive(Debug, Clone, Serialize)]
struct OperationsOverviewPayload {
    primary_provider: &'static str,
    sync_targets: Vec<&'static str>,
    fallback_read_order: Vec<&'static str>,
    replication_mode: &'static str,
    onedrive_async_backup_enabled: bool,
    onedrive_fallback_enabled: bool,
    replication_workers: usize,
    data_plane_max_in_flight: usize,
    data_plane_permits_available: usize,
    pending_jobs: usize,
    retry_scheduled_jobs: usize,
    latest_failed_objects: usize,
    oldest_pending_job_age_ms: Option<u64>,
    oldest_retry_scheduled_job_age_ms: Option<u64>,
    oldest_latest_failed_object_age_ms: Option<u64>,
    latest_object_action_age_ms: Option<u64>,
    notify_webhook_enabled: bool,
    notify_last_success_age_ms: Option<u64>,
    notify_last_error: Option<String>,
    replication_failed_alert_threshold: usize,
    replication_failed_alert_min_age_ms: u64,
    data_plane_loopback_only: bool,
    admin_loopback_only: bool,
    auth_callback_loopback_only: bool,
    metrics_loopback_only: bool,
    s3_secret_uses_default: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MonitoringProviderSummaryPayload {
    total: usize,
    healthy: usize,
    degraded: usize,
    unavailable: usize,
}

#[derive(Debug, Clone, Serialize)]
struct MonitoringReplicationSummaryPayload {
    pending_jobs: usize,
    retry_scheduled_jobs: usize,
    failed_jobs: usize,
    completed_jobs: usize,
}

#[derive(Debug, Clone, Serialize)]
struct MonitoringObjectActionSummaryPayload {
    total_entries: usize,
    successful_entries: usize,
    failed_entries: usize,
    unique_operators: usize,
    last_action_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct MonitoringFailurePayload {
    kind: &'static str,
    provider: Option<String>,
    action: Option<String>,
    target: Option<String>,
    object: Option<String>,
    occurred_at_unix_ms: Option<u64>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowCatalogSummaryPayload {
    provider: String,
    surface: String,
    flow_count: usize,
    source_path: String,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowCatalogPayload {
    provider: String,
    surface: String,
    source_path: String,
    catalog: BrowserFlowCatalog,
}

fn runtime_status_payload(state: &AppState) -> RuntimeStatusPayload {
    RuntimeStatusPayload {
        started_at_unix_ms: state.started_at_unix_ms,
        uptime_ms: current_unix_ms().saturating_sub(state.started_at_unix_ms),
        bind_addr: state.config.bind_addr.to_string(),
        admin_mode: match state.config.admin_mode {
            AdminMode::Off => "off",
            AdminMode::Web => "web",
            AdminMode::Terminal => "terminal",
        },
        admin_bind_addr: state.config.admin_bind_addr.to_string(),
        auth_callback_bind_addr: state.config.auth_callback_bind_addr.to_string(),
        metrics_bind_addr: state.config.metrics_bind_addr.to_string(),
        control_plane_file: state.config.control_plane_file.clone(),
        metadata_db_path: state.config.metadata_db_path.clone(),
        credentials_dir: state.config.credentials_dir.clone(),
        browser_flow_catalog_dir: state.config.browser_flow_catalog_dir.clone(),
        provider_capability_catalog_dir: state.config.provider_capability_catalog_dir.clone(),
        replication_workers: state.config.replication_workers,
        data_plane_max_in_flight: state.config.data_plane_max_in_flight,
        object_action_history_limit: state.config.object_action_history_limit,
    }
}

fn current_notify_status_payload(state: &AppState) -> NotifyStatusPayload {
    let snapshot = state
        .notify_state
        .lock()
        .expect("notify state poisoned")
        .clone();
    NotifyStatusPayload {
        webhook_enabled: state.config.notify_webhook_url.is_some(),
        webhook_url_present: state.config.notify_webhook_url.is_some(),
        signature_enabled: state.config.notify_webhook_signing_secret.is_some(),
        poll_interval_seconds: state.config.notify_poll_interval_seconds,
        last_alert_hash: snapshot.last_alert_hash,
        last_attempt_at_unix_ms: snapshot.last_attempt_at_unix_ms,
        last_success_at_unix_ms: snapshot.last_success_at_unix_ms,
        last_error: snapshot.last_error,
    }
}

fn replication_mode_name(mode: ReplicationMode) -> &'static str {
    match mode {
        ReplicationMode::AsyncBackup => "async_backup",
    }
}

fn socket_addr_is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

fn try_acquire_data_plane_permit(state: &AppState) -> Result<OwnedSemaphorePermit, S3Error> {
    state
        .data_plane_concurrency
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            S3Error::new(
                "ServiceUnavailable",
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "too many concurrent data-plane requests; try again later (limit={})",
                    state.config.data_plane_max_in_flight
                ),
            )
        })
}

fn oldest_job_age_ms<'a>(
    now_unix_ms: u64,
    jobs: impl Iterator<Item = &'a ReplicationJob>,
    predicate: impl Fn(&ReplicationJob) -> bool,
) -> Option<u64> {
    jobs.filter(|job| predicate(job))
        .map(|job| now_unix_ms.saturating_sub(job.enqueued_at_unix_ms as u64))
        .max()
}

fn operations_overview_payload(
    state: &AppState,
    replication_state: &ReplicationStatePayload,
    monitoring: &MonitoringSummaryPayload,
    notify: &NotifyStatusPayload,
) -> OperationsOverviewPayload {
    let now_unix_ms = current_unix_ms();
    let topology = runtime_topology(state);
    let onedrive_policy = current_onedrive_policy(state);
    let data_plane_permits_available = state.data_plane_concurrency.semaphore.available_permits();

    OperationsOverviewPayload {
        primary_provider: topology.primary_provider_name(),
        sync_targets: topology.sync_target_names(),
        fallback_read_order: topology.fallback_read_order_names(),
        replication_mode: replication_mode_name(topology.replication_mode),
        onedrive_async_backup_enabled: onedrive_policy.replication_enabled,
        onedrive_fallback_enabled: onedrive_policy.fallback_enabled,
        replication_workers: state.config.replication_workers,
        data_plane_max_in_flight: state.config.data_plane_max_in_flight,
        data_plane_permits_available,
        pending_jobs: replication_state.persisted.pending_count,
        retry_scheduled_jobs: replication_state.persisted.retry_scheduled_count,
        latest_failed_objects: replication_state.latest_failed_jobs.len(),
        oldest_pending_job_age_ms: oldest_job_age_ms(
            now_unix_ms,
            replication_state.in_memory.pending_jobs.iter(),
            |job| matches!(job.status, ReplicationStatus::Pending),
        ),
        oldest_retry_scheduled_job_age_ms: oldest_job_age_ms(
            now_unix_ms,
            replication_state.in_memory.pending_jobs.iter(),
            |job| matches!(job.status, ReplicationStatus::RetryScheduled),
        ),
        oldest_latest_failed_object_age_ms: oldest_job_age_ms(
            now_unix_ms,
            replication_state.latest_failed_jobs.iter(),
            |_| true,
        ),
        latest_object_action_age_ms: monitoring
            .object_actions
            .last_action_at_unix_ms
            .map(|value| now_unix_ms.saturating_sub(value)),
        notify_webhook_enabled: notify.webhook_enabled,
        notify_last_success_age_ms: notify
            .last_success_at_unix_ms
            .map(|value| now_unix_ms.saturating_sub(value)),
        notify_last_error: notify.last_error.clone(),
        replication_failed_alert_threshold: state.config.replication_failed_alert_threshold,
        replication_failed_alert_min_age_ms: state.config.replication_failed_alert_min_age_ms,
        data_plane_loopback_only: socket_addr_is_loopback(&state.config.bind_addr),
        admin_loopback_only: socket_addr_is_loopback(&state.config.admin_bind_addr),
        auth_callback_loopback_only: socket_addr_is_loopback(&state.config.auth_callback_bind_addr),
        metrics_loopback_only: socket_addr_is_loopback(&state.config.metrics_bind_addr),
        s3_secret_uses_default: state.config.s3_secret_access_key == "change-me",
    }
}

fn monitoring_summary_payload(
    provider_health: &[BackendPayload],
    replication_state: &ReplicationStatePayload,
    object_action_history: &[ObjectActionHistoryEntryPayload],
    alerts: &[AdminAlertPayload],
) -> MonitoringSummaryPayload {
    let mut healthy = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for provider in provider_health {
        match provider.health.status {
            blob_core::HealthStatus::Healthy => healthy += 1,
            blob_core::HealthStatus::Degraded => degraded += 1,
            blob_core::HealthStatus::Unavailable => unavailable += 1,
        }
    }

    let mut successful_entries = 0;
    let mut failed_entries = 0;
    let mut operators = BTreeSet::new();
    let mut last_action_at_unix_ms: Option<u64> = None;
    let mut recent_failures = Vec::new();
    let mut latest_failed_objects = Vec::new();

    for entry in object_action_history {
        if entry.outcome == "success" {
            successful_entries += 1;
        } else {
            failed_entries += 1;
            recent_failures.push(MonitoringFailurePayload {
                kind: "object_action",
                provider: Some(entry.primary_provider.clone()),
                action: Some(entry.action.clone()),
                target: None,
                object: Some(entry.description.clone()),
                occurred_at_unix_ms: Some(entry.executed_at_unix_ms),
                message: entry.message.clone(),
            });
        }
        if let Some(operator) = entry.operator.as_deref() {
            let trimmed = operator.trim();
            if !trimmed.is_empty() {
                operators.insert(trimmed.to_string());
            }
        }
        last_action_at_unix_ms = Some(
            last_action_at_unix_ms
                .map(|current| current.max(entry.executed_at_unix_ms))
                .unwrap_or(entry.executed_at_unix_ms),
        );
    }

    for job in &replication_state.persisted.recent_jobs {
        if !matches!(job.status, ReplicationStatus::Failed) {
            continue;
        }
        recent_failures.push(MonitoringFailurePayload {
            kind: "replication_job",
            provider: job.source_provider.clone(),
            action: Some(job.operation.as_str().to_string()),
            target: Some(job.target.clone()),
            object: Some(format!("{}/{}", job.object.bucket, job.object.key)),
            occurred_at_unix_ms: Some(job.enqueued_at_unix_ms as u64),
            message: job
                .last_error
                .clone()
                .unwrap_or_else(|| "replication job failed without error detail".to_string()),
        });
    }

    for job in &replication_state.latest_failed_jobs {
        latest_failed_objects.push(MonitoringFailurePayload {
            kind: "replication_job",
            provider: job.source_provider.clone(),
            action: Some(job.operation.as_str().to_string()),
            target: Some(job.target.clone()),
            object: Some(format!("{}/{}", job.object.bucket, job.object.key)),
            occurred_at_unix_ms: Some(job.enqueued_at_unix_ms as u64),
            message: job
                .last_error
                .clone()
                .unwrap_or_else(|| "replication job failed without error detail".to_string()),
        });
    }

    latest_failed_objects.sort_by(|left, right| {
        right
            .occurred_at_unix_ms
            .unwrap_or(0)
            .cmp(&left.occurred_at_unix_ms.unwrap_or(0))
    });
    latest_failed_objects.truncate(8);

    recent_failures.sort_by(|left, right| {
        right
            .occurred_at_unix_ms
            .unwrap_or(0)
            .cmp(&left.occurred_at_unix_ms.unwrap_or(0))
    });
    recent_failures.truncate(8);

    MonitoringSummaryPayload {
        open_alert_count: alerts.len(),
        provider_summary: MonitoringProviderSummaryPayload {
            total: provider_health.len(),
            healthy,
            degraded,
            unavailable,
        },
        replication: MonitoringReplicationSummaryPayload {
            pending_jobs: replication_state.persisted.pending_count,
            retry_scheduled_jobs: replication_state.persisted.retry_scheduled_count,
            failed_jobs: replication_state.persisted.failed_count,
            completed_jobs: replication_state.persisted.completed_count,
        },
        object_actions: MonitoringObjectActionSummaryPayload {
            total_entries: object_action_history.len(),
            successful_entries,
            failed_entries,
            unique_operators: operators.len(),
            last_action_at_unix_ms,
        },
        latest_failed_objects,
        recent_failures,
    }
}

fn object_action_audit_fields(input: &ObjectActionInput) -> ObjectActionAuditFields {
    match input {
        ObjectActionInput::Rename {
            operator,
            ticket,
            notes,
            ..
        }
        | ObjectActionInput::Copy {
            operator,
            ticket,
            notes,
            ..
        }
        | ObjectActionInput::Move {
            operator,
            ticket,
            notes,
            ..
        } => ObjectActionAuditFields {
            operator: sanitize_optional_text(operator.clone()),
            ticket: sanitize_optional_text(ticket.clone()),
            notes: sanitize_optional_text(notes.clone()),
        },
    }
}

#[derive(Debug, Clone)]
struct ObjectActionAuditFields {
    operator: Option<String>,
    ticket: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowPayload {
    provider: String,
    surface: String,
    flow: BrowserFlow,
}

#[derive(Debug, Clone, Deserialize)]
struct BrowserFlowDryRunInput {
    provider: String,
    surface: String,
    flow_id: String,
    #[serde(default)]
    inputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    runtime: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowDryRunPayload {
    provider: String,
    surface: String,
    flow_id: String,
    report: BrowserFlowExecutionReport,
}

#[derive(Debug, Clone, Deserialize)]
struct BrowserFlowSessionRunInput {
    provider: String,
    surface: String,
    flow_id: String,
    #[serde(default)]
    auth_session_id: Option<String>,
    #[serde(default)]
    inputs: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    runtime: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    cdp_endpoint_url: Option<String>,
    #[serde(default)]
    cdp_target_selector: Option<String>,
    #[serde(default)]
    cdp_target_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowSessionRunPayload {
    provider: String,
    surface: String,
    flow_id: String,
    auth_session_id: Option<String>,
    status: String,
    #[serde(default)]
    prompts: Vec<AuthCapturePrompt>,
    cdp_endpoint_url: String,
    cdp_target_selector: Option<String>,
    cdp_target_timeout_ms: Option<u64>,
    report: Option<BrowserFlowExecutionReport>,
}

#[derive(Debug, Clone)]
struct BrowserFlowAuthSession {
    session_id: String,
    provider: String,
    surface: String,
    flow_id: String,
    status: String,
    inputs: BTreeMap<String, serde_json::Value>,
    runtime: BTreeMap<String, serde_json::Value>,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    report: Option<BrowserFlowExecutionReport>,
    last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct BrowserFlowAuthSessionPayload {
    session_id: String,
    provider: String,
    surface: String,
    flow_id: String,
    status: String,
    created_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    #[serde(default)]
    prompts: Vec<AuthCapturePrompt>,
    report: Option<BrowserFlowExecutionReport>,
    last_error: Option<String>,
}

#[async_trait::async_trait]
trait BrowserFlowOutputReader {
    async fn evaluate_output_script(
        &self,
        expression: &str,
    ) -> Result<serde_json::Value, BlobError>;
    async fn read_current_url(&self) -> Result<Option<String>, BlobError>;
}

#[derive(Debug, Clone, Deserialize)]
struct BrowserFlowCatalogQuery {
    provider: String,
    surface: String,
}

#[derive(Debug, Clone, Serialize)]
struct AdminAlertPayload {
    severity: &'static str,
    title: String,
    detail: String,
}

#[derive(Debug, Deserialize)]
struct OneDriveCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
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

#[derive(Debug, Deserialize)]
struct DeviceCodeStartResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCodePollErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Clone)]
struct AppConfig {
    bind_addr: SocketAddr,
    admin_mode: AdminMode,
    admin_bind_addr: SocketAddr,
    auth_callback_bind_addr: SocketAddr,
    metrics_bind_addr: SocketAddr,
    notify_webhook_url: Option<String>,
    notify_webhook_signing_secret: Option<String>,
    notify_poll_interval_seconds: u64,
    replication_failed_alert_threshold: usize,
    replication_failed_alert_min_age_ms: u64,
    control_plane_file: String,
    credentials_dir: String,
    browser_flow_catalog_dir: String,
    provider_capability_catalog_dir: String,
    topology: TopologyPolicy,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_region: String,
    metadata_db_path: String,
    metadata_snapshot_recent_limit: usize,
    metadata_retention: MetadataRetentionPolicy,
    replication_workers: usize,
    data_plane_max_in_flight: usize,
    replication_recent_limit: usize,
    replication_max_attempts: u32,
    replication_base_retry_delay_ms: u64,
    replication_max_retry_delay_ms: u64,
    object_action_history_limit: usize,
    max_in_memory_object_bytes: usize,
    onedrive: OneDriveConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProviderCredentialRecord {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    browser_id: Option<String>,
    #[serde(default)]
    cookie_header: Option<String>,
    #[serde(default)]
    family_id: Option<String>,
    #[serde(default)]
    root_folder_id: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    drive_id: Option<String>,
    #[serde(default)]
    redirect_url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProviderCredentialInput {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    browser_id: Option<String>,
    #[serde(default)]
    cookie_header: Option<String>,
    #[serde(default)]
    family_id: Option<String>,
    #[serde(default)]
    root_folder_id: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    drive_id: Option<String>,
    #[serde(default)]
    redirect_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderCredentialPayload {
    provider: &'static str,
    label: &'static str,
    storage_path: String,
    token: Option<String>,
    browser_id: Option<String>,
    cookie_header: Option<String>,
    family_id: Option<String>,
    root_folder_id: Option<String>,
    client_id: Option<String>,
    tenant: Option<String>,
    drive_id: Option<String>,
    redirect_url: Option<String>,
    session_file: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct ReplicationRetryPolicy {
    max_attempts: u32,
    base_delay_ms: u64,
    max_delay_ms: u64,
}

impl ReplicationRetryPolicy {
    fn delay_for_attempt(self, attempts: u32) -> u64 {
        if self.base_delay_ms == 0 {
            return 0;
        }

        let shift = attempts.saturating_sub(1).min(20);
        let multiplier = 1_u64 << shift;
        self.base_delay_ms
            .saturating_mul(multiplier)
            .min(self.max_delay_ms.max(self.base_delay_ms))
    }
}

impl AdminMode {
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "web" => Ok(Self::Web),
            "terminal" | "tui" => Ok(Self::Terminal),
            "off" | "disabled" | "none" => Ok(Self::Off),
            other => anyhow::bail!("unsupported CCBG_ADMIN_MODE: {other}"),
        }
    }
}

impl AuthBrokerState {
    fn new() -> Self {
        Self {
            http_client: HttpClient::new(),
            pending_pkce: Arc::new(Mutex::new(HashMap::new())),
            device_flows: Arc::new(Mutex::new(HashMap::new())),
            capture_prompts: Arc::new(Mutex::new(HashMap::new())),
            browser_flow_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl OnedriveScopeMode {
    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "memory" | "memory_only" | "agent_memory" => Self::MemoryOnly,
            _ => Self::All,
        }
    }
}

impl OnedrivePolicy {
    fn from_env_defaults(topology: &TopologyPolicy) -> Self {
        Self {
            replication_enabled: env_bool(
                "CCBG_ONEDRIVE_REPLICATION_ENABLED",
                topology.sync_targets.contains(&ProviderId::Onedrive),
            ),
            fallback_enabled: env_bool(
                "CCBG_ONEDRIVE_FALLBACK_ENABLED",
                topology.fallback_read_order.contains(&ProviderId::Onedrive),
            ),
            scope_mode: OnedriveScopeMode::parse(&env_or("CCBG_ONEDRIVE_POLICY_MODE", "all")),
            memory_buckets: normalize_bucket_list(&env_csv_list("CCBG_ONEDRIVE_MEMORY_BUCKETS")),
            memory_prefixes: normalize_prefix_list(&env_csv_list("CCBG_ONEDRIVE_MEMORY_PREFIXES")),
            updated_at_unix_ms: current_unix_ms(),
        }
    }

    fn from_input(input: OnedrivePolicyInput) -> Self {
        Self {
            replication_enabled: input.replication_enabled,
            fallback_enabled: input.fallback_enabled,
            scope_mode: input.scope_mode,
            memory_buckets: normalize_bucket_list(&input.memory_buckets),
            memory_prefixes: normalize_prefix_list(&input.memory_prefixes),
            updated_at_unix_ms: current_unix_ms(),
        }
    }

    fn matches_bucket(&self, bucket: &str) -> bool {
        match self.scope_mode {
            OnedriveScopeMode::All => true,
            OnedriveScopeMode::MemoryOnly => {
                if self.memory_buckets.iter().any(|value| value == bucket) {
                    true
                } else {
                    !self.memory_prefixes.is_empty()
                }
            }
        }
    }

    fn matches_object(&self, bucket: &str, key: &str) -> bool {
        match self.scope_mode {
            OnedriveScopeMode::All => true,
            OnedriveScopeMode::MemoryOnly => {
                self.memory_buckets.iter().any(|value| value == bucket)
                    || self
                        .memory_prefixes
                        .iter()
                        .any(|prefix| key.starts_with(prefix))
            }
        }
    }
}

impl AuthCapturePolicy {
    fn from_env_defaults() -> Self {
        Self {
            enabled: env_bool("CCBG_AUTH_CAPTURE_ENABLED", false),
            broker_url: env_opt("CCBG_AUTH_CAPTURE_BROKER_URL"),
            cdp_endpoint_url: env_opt("CCBG_AUTH_CAPTURE_CDP_ENDPOINT_URL"),
            cdp_target_selector: env_opt("CCBG_AUTH_CAPTURE_CDP_TARGET_SELECTOR"),
            cdp_target_timeout_ms: env_opt("CCBG_AUTH_CAPTURE_CDP_TARGET_TIMEOUT_MS")
                .and_then(|value| value.parse::<u64>().ok()),
            llm_analysis_enabled: env_bool("CCBG_AUTH_CAPTURE_LLM_ANALYSIS_ENABLED", false),
            llm_endpoint: env_opt("CCBG_AUTH_CAPTURE_LLM_ENDPOINT"),
            llm_model_id: env_opt("CCBG_AUTH_CAPTURE_LLM_MODEL_ID"),
            llm_api_key: env_opt_or_file(
                "CCBG_AUTH_CAPTURE_LLM_API_KEY",
                "CCBG_AUTH_CAPTURE_LLM_API_KEY_FILE",
            ),
            updated_at_unix_ms: current_unix_ms(),
        }
    }

    fn apply_input(&mut self, input: AuthCapturePolicyInput) {
        self.enabled = input.enabled;
        self.broker_url = normalize_secret_field(input.broker_url);
        self.cdp_endpoint_url = normalize_secret_field(input.cdp_endpoint_url);
        self.cdp_target_selector = normalize_secret_field(input.cdp_target_selector);
        self.cdp_target_timeout_ms = input.cdp_target_timeout_ms.filter(|value| *value > 0);
        self.llm_analysis_enabled = input.llm_analysis_enabled;
        self.llm_endpoint = normalize_secret_field(input.llm_endpoint);
        self.llm_model_id = normalize_secret_field(input.llm_model_id);
        if input.clear_llm_api_key {
            self.llm_api_key = None;
        } else if let Some(api_key) = normalize_secret_field(input.llm_api_key) {
            self.llm_api_key = Some(api_key);
        }
        self.updated_at_unix_ms = current_unix_ms();
    }

    fn payload(&self) -> AuthCapturePolicyPayload {
        AuthCapturePolicyPayload {
            enabled: self.enabled,
            broker_url: self.broker_url.clone(),
            cdp_endpoint_url: self.cdp_endpoint_url.clone(),
            cdp_target_selector: self.cdp_target_selector.clone(),
            cdp_target_timeout_ms: self.cdp_target_timeout_ms,
            llm_analysis_enabled: self.llm_analysis_enabled,
            llm_endpoint: self.llm_endpoint.clone(),
            llm_model_id: self.llm_model_id.clone(),
            llm_api_key_present: self
                .llm_api_key
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty()),
            updated_at_unix_ms: self.updated_at_unix_ms,
        }
    }
}

impl AuthCapturePrompt {
    fn from_input(input: AuthCapturePromptCreateInput) -> Self {
        Self {
            prompt_id: random_urlsafe_token(16),
            provider: input.provider.trim().to_string(),
            session_id: normalize_secret_field(input.session_id),
            flow_id: normalize_secret_field(input.flow_id),
            input_id: normalize_secret_field(input.input_id),
            title: input.title.trim().to_string(),
            message: input.message.trim().to_string(),
            field_label: input.field_label.trim().to_string(),
            field_kind: input.field_kind,
            placeholder: normalize_secret_field(input.placeholder),
            status: "pending".to_string(),
            created_at_unix_ms: current_unix_ms(),
            answered_at_unix_ms: None,
            answer_present: false,
            answer_value: None,
        }
    }

    fn answer(&mut self, value: String) {
        self.answer_value = Some(value);
        self.answer_present = true;
        self.status = "answered".to_string();
        self.answered_at_unix_ms = Some(current_unix_ms());
    }

    fn sanitized(&self) -> Self {
        let mut copy = self.clone();
        copy.answer_value = None;
        copy
    }
}

impl BrowserFlowAuthSession {
    fn new(
        session_id: String,
        provider: String,
        surface: String,
        flow_id: String,
        inputs: BTreeMap<String, serde_json::Value>,
        runtime: BTreeMap<String, serde_json::Value>,
    ) -> Self {
        let now = current_unix_ms();
        Self {
            session_id,
            provider,
            surface,
            flow_id,
            status: BROWSER_FLOW_AUTH_SESSION_STATUS_PENDING.to_string(),
            inputs,
            runtime,
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            report: None,
            last_error: None,
        }
    }

    fn apply_request(
        &mut self,
        provider: String,
        surface: String,
        flow_id: String,
        inputs: BTreeMap<String, serde_json::Value>,
        runtime: BTreeMap<String, serde_json::Value>,
    ) {
        self.provider = provider;
        self.surface = surface;
        self.flow_id = flow_id;
        self.inputs.extend(inputs);
        self.runtime.extend(runtime);
        self.updated_at_unix_ms = current_unix_ms();
        if matches!(
            self.status.as_str(),
            BROWSER_FLOW_AUTH_SESSION_STATUS_COMPLETED | BROWSER_FLOW_AUTH_SESSION_STATUS_FAILED
        ) {
            self.status = BROWSER_FLOW_AUTH_SESSION_STATUS_PENDING.to_string();
            self.report = None;
            self.last_error = None;
        }
    }

    fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
        self.updated_at_unix_ms = current_unix_ms();
    }

    fn set_completed(&mut self, report: BrowserFlowExecutionReport) {
        self.status = BROWSER_FLOW_AUTH_SESSION_STATUS_COMPLETED.to_string();
        self.report = Some(report);
        self.last_error = None;
        self.updated_at_unix_ms = current_unix_ms();
    }

    fn set_failed(&mut self, error: &BlobError) {
        self.status = BROWSER_FLOW_AUTH_SESSION_STATUS_FAILED.to_string();
        self.last_error = Some(error.to_string());
        self.updated_at_unix_ms = current_unix_ms();
    }
}

impl ProviderCredentialRecord {
    fn normalize(mut self) -> Self {
        self.token = normalize_secret_field(self.token);
        self.browser_id = normalize_secret_field(self.browser_id);
        self.cookie_header = normalize_secret_field(self.cookie_header);
        self.family_id = normalize_secret_field(self.family_id);
        self.root_folder_id = normalize_secret_field(self.root_folder_id);
        self.client_id = normalize_secret_field(self.client_id);
        self.tenant = normalize_secret_field(self.tenant);
        self.drive_id = normalize_secret_field(self.drive_id);
        self.redirect_url = normalize_secret_field(self.redirect_url);
        self
    }

    fn is_empty(&self) -> bool {
        self.token.is_none()
            && self.browser_id.is_none()
            && self.cookie_header.is_none()
            && self.family_id.is_none()
            && self.root_folder_id.is_none()
            && self.client_id.is_none()
            && self.tenant.is_none()
            && self.drive_id.is_none()
            && self.redirect_url.is_none()
    }
}

impl From<ProviderCredentialInput> for ProviderCredentialRecord {
    fn from(value: ProviderCredentialInput) -> Self {
        Self {
            token: value.token,
            browser_id: value.browser_id,
            cookie_header: value.cookie_header,
            family_id: value.family_id,
            root_folder_id: value.root_folder_id,
            client_id: value.client_id,
            tenant: value.tenant,
            drive_id: value.drive_id,
            redirect_url: value.redirect_url,
        }
        .normalize()
    }
}

impl AppConfig {
    fn from_env() -> Result<Self> {
        let bind_addr: SocketAddr = env::var("CCBG_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:61080".to_string())
            .parse()
            .context("invalid CCBG_BIND_ADDR")?;
        validate_port_range(bind_addr.port())?;
        let admin_bind_addr: SocketAddr = env::var("CCBG_ADMIN_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:61081".to_string())
            .parse()
            .context("invalid CCBG_ADMIN_BIND_ADDR")?;
        validate_port_range(admin_bind_addr.port())?;
        let auth_callback_bind_addr: SocketAddr = env::var("CCBG_AUTH_CALLBACK_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:61082".to_string())
            .parse()
            .context("invalid CCBG_AUTH_CALLBACK_BIND_ADDR")?;
        validate_port_range(auth_callback_bind_addr.port())?;
        let metrics_bind_addr: SocketAddr = env::var("CCBG_METRICS_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:61083".to_string())
            .parse()
            .context("invalid CCBG_METRICS_BIND_ADDR")?;
        validate_port_range(metrics_bind_addr.port())?;
        let admin_mode = AdminMode::parse(&env_or("CCBG_ADMIN_MODE", "web"))?;

        let onedrive_enabled = env_bool("CCBG_ONEDRIVE_ENABLED", true);
        let primary_provider = ProviderId::parse(
            &env::var("CCBG_PRIMARY_PROVIDER")
                .or_else(|_| env::var("CCBG_PROVIDER"))
                .unwrap_or_else(|_| "unicom".to_string()),
        )?;
        let sync_targets = parse_provider_list(&env::var("CCBG_SYNC_TARGETS").unwrap_or_default())?;
        let fallback_read_order =
            parse_provider_list(&env::var("CCBG_FALLBACK_READ_ORDER").unwrap_or_default())?;
        let replication_mode =
            ReplicationMode::parse(&env_or("CCBG_REPLICATION_MODE", "async_backup"))?;
        let topology = TopologyPolicy::from_input(TopologyInput {
            primary_provider,
            sync_targets,
            fallback_read_order,
            onedrive_enabled,
            replication_mode,
        })?;

        Ok(Self {
            bind_addr,
            admin_mode,
            admin_bind_addr,
            auth_callback_bind_addr,
            metrics_bind_addr,
            notify_webhook_url: env_opt("CCBG_NOTIFY_WEBHOOK_URL"),
            notify_webhook_signing_secret: env_opt_or_file(
                "CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET",
                "CCBG_NOTIFY_WEBHOOK_SIGNING_SECRET_FILE",
            ),
            notify_poll_interval_seconds: env_u64("CCBG_NOTIFY_POLL_INTERVAL_SECONDS", 15).max(5),
            replication_failed_alert_threshold: env_usize(
                "CCBG_REPLICATION_FAILED_ALERT_THRESHOLD",
                1,
            )
            .max(1),
            replication_failed_alert_min_age_ms: env_u64(
                "CCBG_REPLICATION_FAILED_ALERT_MIN_AGE_MS",
                0,
            ),
            control_plane_file: env_or("CCBG_CONTROL_PLANE_FILE", "./data/control-plane.json"),
            credentials_dir: env_or("CCBG_CREDENTIALS_DIR", "./data/provider-credentials"),
            browser_flow_catalog_dir: env_or(
                "CCBG_BROWSER_FLOW_CATALOG_DIR",
                "./config/browser-flows",
            ),
            provider_capability_catalog_dir: env_or(
                "CCBG_PROVIDER_CAPABILITY_CATALOG_DIR",
                "./config/provider-capabilities",
            ),
            topology,
            s3_access_key_id: env_or("CCBG_S3_ACCESS_KEY_ID", "ccbg"),
            s3_secret_access_key: env_or("CCBG_S3_SECRET_ACCESS_KEY", "change-me"),
            s3_region: env_or("CCBG_S3_REGION", "us-east-1"),
            metadata_db_path: env_or("CCBG_METADATA_DB_PATH", "./data/ccbg.db"),
            metadata_snapshot_recent_limit: env_usize("CCBG_METADATA_SNAPSHOT_RECENT_LIMIT", 32),
            metadata_retention: MetadataRetentionPolicy {
                completed_history_limit: env_usize("CCBG_METADATA_COMPLETED_HISTORY_LIMIT", 512),
                failed_history_limit: env_usize("CCBG_METADATA_FAILED_HISTORY_LIMIT", 256),
            },
            replication_workers: env_usize("CCBG_REPLICATION_WORKERS", 2),
            data_plane_max_in_flight: env_usize("CCBG_DATA_PLANE_MAX_IN_FLIGHT", 8).max(1),
            replication_recent_limit: env_usize("CCBG_REPLICATION_RECENT_LIMIT", 64),
            replication_max_attempts: env_u64("CCBG_REPLICATION_MAX_ATTEMPTS", 3) as u32,
            replication_base_retry_delay_ms: env_u64("CCBG_REPLICATION_BASE_RETRY_DELAY_MS", 1_000),
            replication_max_retry_delay_ms: env_u64("CCBG_REPLICATION_MAX_RETRY_DELAY_MS", 30_000),
            object_action_history_limit: env_usize(
                "CCBG_OBJECT_ACTION_HISTORY_LIMIT",
                DEFAULT_OBJECT_ACTION_HISTORY_LIMIT,
            ),
            max_in_memory_object_bytes: env_usize(
                "CCBG_MAX_IN_MEMORY_OBJECT_BYTES",
                8 * 1024 * 1024,
            ),
            onedrive: OneDriveConfig {
                enabled: onedrive_enabled,
                tenant: env_or("CCBG_ONEDRIVE_TENANT", "common"),
                client_id: env_opt("CCBG_ONEDRIVE_CLIENT_ID"),
                use_device_code: env_bool("CCBG_ONEDRIVE_USE_DEVICE_CODE", false),
                redirect_url: env_opt("CCBG_ONEDRIVE_REDIRECT_URL"),
                drive_id: env_opt("CCBG_ONEDRIVE_DRIVE_ID"),
                graph_base_url: env_or(
                    "CCBG_ONEDRIVE_GRAPH_BASE_URL",
                    "https://graph.microsoft.com/v1.0",
                ),
                auth_base_url: env_or(
                    "CCBG_ONEDRIVE_AUTH_BASE_URL",
                    DEFAULT_ONEDRIVE_AUTH_BASE_URL,
                ),
                scopes: env_or("CCBG_ONEDRIVE_SCOPES", DEFAULT_ONEDRIVE_SCOPES),
                session_file: Some(
                    env_opt("CCBG_ONEDRIVE_SESSION_FILE")
                        .unwrap_or_else(|| "./data/onedrive-session.json".to_string()),
                ),
                token_source: resolve_token_source("CCBG_ONEDRIVE"),
                root_prefix: env_opt("CCBG_ONEDRIVE_ROOT_PREFIX"),
                user_agent: env_or("CCBG_ONEDRIVE_USER_AGENT", "carrier-cloud-blob-gateway/0.1"),
                request_timeout_secs: env_u64("CCBG_ONEDRIVE_TIMEOUT_SECS", 30),
            },
        })
    }
}

fn validate_port_range(port: u16) -> Result<()> {
    if (60000..=65534).contains(&port) {
        Ok(())
    } else {
        anyhow::bail!("CCBG_BIND_ADDR port must be between 60000 and 65534, got {port}")
    }
}

#[derive(Debug, Serialize)]
struct IndexPayload {
    service: &'static str,
    backend: &'static str,
    primary_provider: &'static str,
    sync_targets: Vec<&'static str>,
    fallback_read_order: Vec<&'static str>,
    replication: ReplicationQueueSummary,
    endpoints: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct ReplicationQueueSummary {
    pending_jobs: usize,
    recent_jobs: usize,
}

#[derive(Debug, Serialize)]
struct ReplicationTargetStatusPayload {
    provider: String,
    label: String,
    queued_count: usize,
    pending_count: usize,
    retry_scheduled_count: usize,
    completed_count: usize,
    failed_count: usize,
    latest_job: Option<ReplicationJob>,
}

#[derive(Debug, Serialize)]
struct ReplicationStatePayload {
    in_memory: ReplicationSnapshot,
    persisted: MetadataSnapshot,
    target_statuses: Vec<ReplicationTargetStatusPayload>,
    latest_failed_jobs: Vec<ReplicationJob>,
}

#[derive(Debug, Serialize)]
struct BackendPayload {
    role: &'static str,
    provider: &'static str,
    health: blob_core::ServiceHealth,
}

#[derive(Debug, Serialize)]
struct ProviderTestPayload {
    provider: &'static str,
    label: &'static str,
    roles: Vec<&'static str>,
    checked_at_unix_ms: u64,
    health: blob_core::ServiceHealth,
}

#[derive(Debug, Serialize)]
struct ReplicationRetryPayload {
    job_id: u64,
    status: &'static str,
    target: String,
    bucket: String,
    key: String,
}

#[derive(Debug, Serialize)]
struct ReplicationTargetRetryJobPayload {
    job_id: u64,
    status: &'static str,
    bucket: String,
    key: String,
}

#[derive(Debug, Serialize)]
struct ReplicationTargetRetryPayload {
    target: String,
    retried_jobs: usize,
    jobs: Vec<ReplicationTargetRetryJobPayload>,
}

#[derive(Debug, Serialize)]
struct ObjectProviderStatusPayload {
    provider: &'static str,
    label: &'static str,
    roles: Vec<&'static str>,
    fallback_order_index: Option<usize>,
    exists: bool,
    readable_via_gateway: bool,
    accepts_replication_put: Option<bool>,
    object_info: Option<blob_core::ObjectInfo>,
    access_error: Option<String>,
    latest_replication_job: Option<ReplicationJob>,
    fallback_gate: Option<&'static str>,
    fallback_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectActionHistoryReferencePayload {
    label: String,
    bucket: String,
    key: String,
    changes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ObjectActionHistoryEntryPayload {
    executed_at_unix_ms: u64,
    primary_provider: String,
    action: String,
    description: String,
    outcome: String,
    message: String,
    operator: Option<String>,
    ticket: Option<String>,
    notes: Option<String>,
    warnings: Vec<String>,
    references: Vec<ObjectActionHistoryReferencePayload>,
}

#[derive(Debug, Serialize)]
struct ObjectStatusPayload {
    bucket: String,
    key: String,
    primary_provider: &'static str,
    gateway_read_source: Option<&'static str>,
    gateway_fallback_from: Option<&'static str>,
    gateway_error: Option<String>,
    provider_states: Vec<ObjectProviderStatusPayload>,
}

#[derive(Debug, Deserialize)]
struct ObjectsQuery {
    container: Option<String>,
    prefix: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ObjectStatusQuery {
    bucket: String,
    key: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[derive(Clone)]
enum ObjectActionInput {
    Rename {
        bucket: String,
        key: String,
        new_key: String,
        #[serde(default)]
        operator: Option<String>,
        #[serde(default)]
        ticket: Option<String>,
        #[serde(default)]
        notes: Option<String>,
    },
    Copy {
        source_bucket: String,
        source_key: String,
        destination_bucket: String,
        destination_key: String,
        #[serde(default)]
        operator: Option<String>,
        #[serde(default)]
        ticket: Option<String>,
        #[serde(default)]
        notes: Option<String>,
    },
    Move {
        source_bucket: String,
        source_key: String,
        destination_bucket: String,
        destination_key: String,
        #[serde(default)]
        operator: Option<String>,
        #[serde(default)]
        ticket: Option<String>,
        #[serde(default)]
        notes: Option<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct ListObjectsV2Query {
    #[serde(rename = "list-type")]
    list_type: Option<String>,
    prefix: Option<String>,
    #[serde(rename = "max-keys")]
    max_keys: Option<usize>,
    #[serde(rename = "continuation-token")]
    continuation_token: Option<String>,
    delimiter: Option<String>,
}

#[derive(Debug)]
struct ApiError(BlobError);

impl From<BlobError> for ApiError {
    fn from(value: BlobError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error = self.0;
        let status = match &error {
            BlobError::Configuration(_) => StatusCode::BAD_REQUEST,
            BlobError::Upstream(_) => StatusCode::BAD_GATEWAY,
            BlobError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            BlobError::NotFound(_) => StatusCode::NOT_FOUND,
            BlobError::BodyStream(_) => StatusCode::BAD_REQUEST,
        };

        (status, Json(json!({ "error": error.to_string() }))).into_response()
    }
}

#[derive(Debug)]
struct DataPlaneApiError {
    status: StatusCode,
    message: String,
}

impl DataPlaneApiError {
    fn from_s3_error(error: S3Error) -> Self {
        Self {
            status: error.status,
            message: error.message,
        }
    }
}

impl From<BlobError> for DataPlaneApiError {
    fn from(value: BlobError) -> Self {
        let status = match &value {
            BlobError::Configuration(_) => StatusCode::BAD_REQUEST,
            BlobError::Upstream(_) => StatusCode::BAD_GATEWAY,
            BlobError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            BlobError::NotFound(_) => StatusCode::NOT_FOUND,
            BlobError::BodyStream(_) => StatusCode::BAD_REQUEST,
        };

        Self {
            status,
            message: value.to_string(),
        }
    }
}

impl IntoResponse for DataPlaneApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[derive(Debug)]
struct S3Error {
    code: &'static str,
    message: String,
    status: StatusCode,
}

impl S3Error {
    fn new(code: &'static str, status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status,
        }
    }

    fn access_denied(message: impl Into<String>) -> Self {
        Self::new("AccessDenied", StatusCode::FORBIDDEN, message)
    }

    fn invalid_access_key() -> Self {
        Self::new(
            "InvalidAccessKeyId",
            StatusCode::FORBIDDEN,
            "The AWS Access Key Id you provided does not exist in our records.",
        )
    }

    fn signature_mismatch(message: impl Into<String>) -> Self {
        Self::new("SignatureDoesNotMatch", StatusCode::FORBIDDEN, message)
    }

    fn no_such_bucket(bucket: &str) -> Self {
        Self::new(
            "NoSuchBucket",
            StatusCode::NOT_FOUND,
            format!("The specified bucket does not exist: {bucket}"),
        )
    }

    fn no_such_key(bucket: &str, key: &str) -> Self {
        Self::new(
            "NoSuchKey",
            StatusCode::NOT_FOUND,
            format!("The specified key does not exist: {bucket}/{key}"),
        )
    }

    fn not_implemented(message: impl Into<String>) -> Self {
        Self::new("NotImplemented", StatusCode::NOT_IMPLEMENTED, message)
    }

    fn entity_too_large(message: impl Into<String>) -> Self {
        Self::new("EntityTooLarge", StatusCode::PAYLOAD_TOO_LARGE, message)
    }

    fn internal_error(message: impl Into<String>) -> Self {
        Self::new("InternalError", StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for S3Error {
    fn into_response(self) -> Response {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<Error>\
<Code>{}</Code>\
<Message>{}</Message>\
<RequestId>{}</RequestId>\
</Error>",
            xml_escape(self.code),
            xml_escape(&self.message),
            REQUEST_ID
        );

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/xml"));
        headers.insert("x-amz-request-id", HeaderValue::from_static(REQUEST_ID));

        (self.status, headers, body).into_response()
    }
}

struct ParsedAuthorization {
    access_key: String,
    date: String,
    region: String,
    service: String,
    signed_headers: Vec<String>,
    signature: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| {
            "info,gatewayd=debug,provider_unicom=debug,provider_telecom=debug,provider_mobile=debug,provider_onedrive=debug,replication_engine=debug,policy_engine=debug"
                .to_string()
        }))
        .init();

    let mut config = AppConfig::from_env()?;
    let control_plane = load_control_plane_state(
        &config.control_plane_file,
        ControlPlaneState {
            topology: config.topology.clone(),
            onedrive_policy: OnedrivePolicy::from_env_defaults(&config.topology),
            auth_capture_policy: AuthCapturePolicy::from_env_defaults(),
            object_action_history: Vec::new(),
        },
        config.onedrive.enabled,
    )?;
    config.topology = control_plane.topology.clone();
    let config = Arc::new(config);
    let browser_flow_catalogs = Arc::new(
        BrowserFlowCatalogCollection::from_json_dir(&config.browser_flow_catalog_dir)
            .context("failed to load browser flow catalogs")?,
    );
    let backends =
        build_all_backends(&config).context("failed to build provider backend registry")?;
    let primary_provider_name = control_plane.topology.primary_provider.as_str();
    let backend_name =
        backend_for_provider_from(&backends, control_plane.topology.primary_provider)
            .expect("primary backend should exist")
            .name();
    let replication = Arc::new(ReplicationEngine::with_recent_limit(
        config.replication_recent_limit,
    ));
    let metadata_store = Arc::new(
        MetadataStore::open_with_options(
            &config.metadata_db_path,
            MetadataStoreOptions {
                retention: config.metadata_retention,
            },
        )
        .context("failed to open metadata store")?,
    );
    let prune_result = metadata_store
        .apply_retention()
        .context("failed to apply metadata retention")?;
    if prune_result.total_deleted() > 0 {
        info!(
            deleted_completed_jobs = prune_result.deleted_completed_jobs,
            deleted_failed_jobs = prune_result.deleted_failed_jobs,
            "pruned retained metadata history on startup"
        );
    }
    let mut restored_jobs = metadata_store
        .load_pending_jobs(None)
        .context("failed to restore pending replication jobs")?;
    let legacy_source_provider = Some(control_plane.topology.primary_provider.as_str().to_string());
    let mut restored_jobs_backfilled = false;
    for job in &mut restored_jobs {
        if job.source_provider.is_none() {
            job.source_provider = legacy_source_provider.clone();
            restored_jobs_backfilled = true;
        }
    }
    if restored_jobs_backfilled {
        metadata_store
            .enqueue_jobs(&restored_jobs)
            .context("failed to backfill legacy source_provider on restored jobs")?;
    }
    if !restored_jobs.is_empty() {
        info!(
            restored_jobs = restored_jobs.len(),
            "restored replication jobs from sqlite"
        );
        replication.restore_pending(restored_jobs);
    }
    let next_job_id = metadata_store
        .max_job_id()
        .context("failed to read max replication job id")?
        .unwrap_or(0)
        .saturating_add(1);
    replication.ensure_next_job_id_at_least(next_job_id);

    let state = AppState {
        config: config.clone(),
        backends: Arc::new(Mutex::new(backends)),
        replication,
        metadata_store,
        auth: Arc::new(AuthBrokerState::new()),
        control_plane: Arc::new(Mutex::new(control_plane)),
        notify_state: Arc::new(Mutex::new(NotifyState {
            last_alert_hash: None,
            last_attempt_at_unix_ms: None,
            last_success_at_unix_ms: None,
            last_error: None,
        })),
        browser_flow_catalogs,
        data_plane_concurrency: Arc::new(DataPlaneConcurrencyState {
            semaphore: Arc::new(Semaphore::new(config.data_plane_max_in_flight)),
        }),
        started_at_unix_ms: current_unix_ms(),
    };
    spawn_replication_workers(state.clone(), config.replication_workers);
    tokio::spawn(notify_loop(state.clone()));
    spawn_admin_services(state.clone())
        .await
        .context("failed to start admin/auth services")?;

    let app = Router::new()
        .route("/__ccbg", get(index))
        .route("/__ccbg/providers", get(list_provider_health))
        .route("/__ccbg/replication", get(replication_snapshot))
        .route("/healthz", get(healthz))
        .route("/v1/containers", get(list_containers))
        .route("/v1/objects", get(list_objects))
        .route("/", get(list_buckets))
        .route("/{bucket}", get(list_objects_v2).head(head_bucket))
        .route(
            "/{bucket}/{*key}",
            get(get_object)
                .head(head_object)
                .put(put_object)
                .delete(delete_object),
        )
        .with_state(state)
        .layer(DefaultBodyLimit::max(config.max_in_memory_object_bytes))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .context("failed to bind listener")?;

    info!(
        bind_addr = %config.bind_addr,
        admin_mode = ?config.admin_mode,
        admin_bind_addr = %config.admin_bind_addr,
        auth_callback_bind_addr = %config.auth_callback_bind_addr,
        metrics_bind_addr = %config.metrics_bind_addr,
        data_plane_max_in_flight = config.data_plane_max_in_flight,
        notify_webhook_enabled = config.notify_webhook_url.is_some(),
        notify_webhook_signature_enabled = config.notify_webhook_signing_secret.is_some(),
        notify_poll_interval_seconds = config.notify_poll_interval_seconds,
        control_plane_file = %config.control_plane_file,
        browser_flow_catalog_dir = %config.browser_flow_catalog_dir,
        provider_capability_catalog_dir = %config.provider_capability_catalog_dir,
        primary_provider = primary_provider_name,
        backend = backend_name,
        metadata_db_path = %config.metadata_db_path,
        metadata_snapshot_recent_limit = config.metadata_snapshot_recent_limit,
        metadata_completed_history_limit = config.metadata_retention.completed_history_limit,
        metadata_failed_history_limit = config.metadata_retention.failed_history_limit,
        replication_workers = config.replication_workers,
        replication_recent_limit = config.replication_recent_limit,
        replication_max_attempts = config.replication_max_attempts,
        replication_base_retry_delay_ms = config.replication_base_retry_delay_ms,
        replication_max_retry_delay_ms = config.replication_max_retry_delay_ms,
        object_action_history_limit = config.object_action_history_limit,
        max_in_memory_object_bytes = config.max_in_memory_object_bytes,
        "gateway ready"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server exited with error")
}

async fn spawn_admin_services(state: AppState) -> Result<()> {
    if matches!(state.config.admin_mode, AdminMode::Off) {
        return Ok(());
    }

    let admin_listener = tokio::net::TcpListener::bind(state.config.admin_bind_addr)
        .await
        .context("failed to bind admin listener")?;
    let admin_app = Router::new()
        .route("/", get(admin_index))
        .route("/api/status", get(admin_status))
        .route(
            "/api/replication/jobs/{job_id}/retry",
            post(retry_replication_job_api),
        )
        .route(
            "/api/replication/targets/{target}/retry-failed",
            post(retry_replication_target_api),
        )
        .route("/api/control-plane/topology", post(update_topology))
        .route(
            "/api/providers/{provider}/credentials",
            get(get_provider_credentials).post(update_provider_credentials),
        )
        .route("/api/providers/{provider}/test", post(test_provider))
        .route("/api/object-status", get(inspect_object_status))
        .route("/api/object-actions", post(run_object_action))
        .route(
            "/api/object-actions/history/clear",
            post(clear_object_action_history_api),
        )
        .route(
            "/api/browser-flows/catalogs",
            get(list_browser_flow_catalogs),
        )
        .route("/api/browser-flows/catalog", get(get_browser_flow_catalog))
        .route(
            "/api/browser-flows/flow/{flow_id}",
            get(get_browser_flow_by_id),
        )
        .route("/api/browser-flows/dry-run", post(run_browser_flow_dry_run))
        .route(
            "/api/browser-flows/session/{session_id}",
            get(get_browser_flow_session),
        )
        .route(
            "/api/browser-flows/session-run",
            post(run_browser_flow_session),
        )
        .route(
            "/api/policy/onedrive",
            get(get_onedrive_policy).post(update_onedrive_policy),
        )
        .route(
            "/api/policy/auth-capture",
            get(get_auth_capture_policy).post(update_auth_capture_policy),
        )
        .route(
            "/api/auth-capture/prompts",
            get(list_auth_capture_prompts).post(create_auth_capture_prompt),
        )
        .route(
            "/api/auth-capture/prompts/{prompt_id}",
            get(get_auth_capture_prompt),
        )
        .route(
            "/api/auth-capture/prompts/{prompt_id}/reply",
            post(reply_auth_capture_prompt),
        )
        .route("/api/auth/onedrive/status", get(onedrive_auth_status))
        .route(
            "/api/auth/onedrive/web/start",
            get(start_onedrive_web_login),
        )
        .route(
            "/api/auth/onedrive/device/start",
            post(start_onedrive_device_flow),
        )
        .route(
            "/api/auth/onedrive/device/{flow_id}",
            get(get_onedrive_device_flow),
        )
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(admin_listener, admin_app).await {
            warn!(error = %error, "admin service exited");
        }
    });

    info!(
        bind_addr = %state.config.admin_bind_addr,
        mode = ?state.config.admin_mode,
        "admin service ready"
    );

    let metrics_listener = tokio::net::TcpListener::bind(state.config.metrics_bind_addr)
        .await
        .context("failed to bind metrics listener")?;
    let metrics_app = Router::new()
        .route("/healthz", get(metrics_healthz))
        .route("/readyz", get(metrics_readyz))
        .route("/metrics", get(metrics_prometheus))
        .with_state(state.clone())
        .layer(TraceLayer::new_for_http());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(metrics_listener, metrics_app).await {
            warn!(error = %error, "metrics service exited");
        }
    });

    info!(
        bind_addr = %state.config.metrics_bind_addr,
        "metrics service ready"
    );

    if matches!(state.config.admin_mode, AdminMode::Web) {
        let callback_listener = tokio::net::TcpListener::bind(state.config.auth_callback_bind_addr)
            .await
            .context("failed to bind OneDrive callback listener")?;
        let callback_app = Router::new()
            .route("/auth/onedrive/callback", get(handle_onedrive_callback))
            .with_state(state.clone())
            .layer(TraceLayer::new_for_http());
        tokio::spawn(async move {
            if let Err(error) = axum::serve(callback_listener, callback_app).await {
                warn!(error = %error, "onedrive callback service exited");
            }
        });

        info!(
            bind_addr = %state.config.auth_callback_bind_addr,
            "onedrive callback service ready"
        );
    }

    Ok(())
}

fn normalized_onedrive_scopes(config: &OneDriveConfig) -> String {
    let normalized = config
        .scopes
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

fn onedrive_auth_base_url(config: &OneDriveConfig) -> &str {
    config.auth_base_url.trim_end_matches('/')
}

fn onedrive_auth_tenant(config: &OneDriveConfig) -> &str {
    let trimmed = config.tenant.trim();
    if trimmed.is_empty() {
        "common"
    } else {
        trimmed
    }
}

fn onedrive_authorize_endpoint(config: &OneDriveConfig) -> String {
    format!(
        "{}/{}/oauth2/v2.0/authorize",
        onedrive_auth_base_url(config),
        encode_query_component(onedrive_auth_tenant(config))
    )
}

fn onedrive_token_endpoint(config: &OneDriveConfig) -> String {
    format!(
        "{}/{}/oauth2/v2.0/token",
        onedrive_auth_base_url(config),
        encode_query_component(onedrive_auth_tenant(config))
    )
}

fn onedrive_device_code_endpoint(config: &OneDriveConfig) -> String {
    format!(
        "{}/{}/oauth2/v2.0/devicecode",
        onedrive_auth_base_url(config),
        encode_query_component(onedrive_auth_tenant(config))
    )
}

fn onedrive_session_file(config: &OneDriveConfig) -> Result<&str, ApiError> {
    config
        .session_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BlobError::Configuration(
                "CCBG_ONEDRIVE_SESSION_FILE is required for built-in OneDrive OAuth".to_string(),
            )
            .into()
        })
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn sanitize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn ensure_onedrive_client_id(config: &OneDriveConfig) -> Result<&str, ApiError> {
    config
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BlobError::Configuration(
                "CCBG_ONEDRIVE_CLIENT_ID is required for built-in OneDrive OAuth".to_string(),
            )
            .into()
        })
}

fn ensure_onedrive_redirect_url(config: &OneDriveConfig) -> Result<&str, ApiError> {
    config
        .redirect_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BlobError::Configuration(
                "CCBG_ONEDRIVE_REDIRECT_URL is required for web-based OneDrive OAuth".to_string(),
            )
            .into()
        })
}

fn random_urlsafe_token(bytes_len: usize) -> String {
    let mut bytes = vec![0_u8; bytes_len];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn encode_query_component(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn build_onedrive_authorize_url(
    config: &OneDriveConfig,
    state: &str,
    code_challenge: &str,
) -> Result<String, ApiError> {
    let client_id = ensure_onedrive_client_id(config)?;
    let redirect_url = ensure_onedrive_redirect_url(config)?;
    let scope = normalized_onedrive_scopes(config);
    Ok(format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        onedrive_authorize_endpoint(config),
        encode_query_component(client_id),
        encode_query_component(redirect_url),
        encode_query_component(&scope),
        encode_query_component(state),
        encode_query_component(code_challenge),
    ))
}

fn oauth_session_from_token_response(
    response: OAuthTokenResponse,
    previous_refresh_token: Option<&str>,
) -> Result<OneDriveOAuthSession, BlobError> {
    let access_token = response.access_token.trim();
    if access_token.is_empty() {
        return Err(BlobError::Upstream(
            "OneDrive token endpoint returned an empty access_token".to_string(),
        ));
    }

    Ok(OneDriveOAuthSession {
        access_token: access_token.to_string(),
        refresh_token: response
            .refresh_token
            .or_else(|| previous_refresh_token.map(ToString::to_string)),
        token_type: response
            .token_type
            .unwrap_or_else(|| "Bearer".to_string())
            .trim()
            .to_string(),
        scope: response.scope.map(|value| value.trim().to_string()),
        expires_at_unix: response
            .expires_in
            .map(|expires_in| current_unix_ms() / 1000 + expires_in),
    })
}

async fn exchange_authorization_code(
    client: &HttpClient,
    config: &OneDriveConfig,
    code: &str,
    code_verifier: &str,
) -> Result<OneDriveOAuthSession, BlobError> {
    let client_id = ensure_onedrive_client_id(config).map_err(|error| error.0)?;
    let redirect_url = ensure_onedrive_redirect_url(config).map_err(|error| error.0)?;
    let scope = normalized_onedrive_scopes(config);
    let action = "exchange OneDrive authorization code";
    let response = client
        .post(onedrive_token_endpoint(config))
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_url),
            ("code_verifier", code_verifier),
            ("scope", scope.as_str()),
        ])
        .send()
        .await
        .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BlobError::Upstream(format!(
            "{action} failed with {status}: {}",
            body.trim()
        )));
    }

    let payload = response
        .json::<OAuthTokenResponse>()
        .await
        .map_err(|error| BlobError::Upstream(format!("{action} returned invalid JSON: {error}")))?;
    oauth_session_from_token_response(payload, None)
}

async fn request_device_code(
    client: &HttpClient,
    config: &OneDriveConfig,
) -> Result<DeviceCodeStartResponse, BlobError> {
    let client_id = ensure_onedrive_client_id(config).map_err(|error| error.0)?;
    let scope = normalized_onedrive_scopes(config);
    let action = "start OneDrive device code flow";
    let response = client
        .post(onedrive_device_code_endpoint(config))
        .form(&[("client_id", client_id), ("scope", scope.as_str())])
        .send()
        .await
        .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(BlobError::Upstream(format!(
            "{action} failed with {status}: {}",
            body.trim()
        )));
    }

    response
        .json::<DeviceCodeStartResponse>()
        .await
        .map_err(|error| BlobError::Upstream(format!("{action} returned invalid JSON: {error}")))
}

async fn poll_device_code_once(
    client: &HttpClient,
    config: &OneDriveConfig,
    device_code: &str,
) -> Result<Result<OneDriveOAuthSession, &'static str>, BlobError> {
    let client_id = ensure_onedrive_client_id(config).map_err(|error| error.0)?;
    let action = "poll OneDrive device code flow";
    let response = client
        .post(onedrive_token_endpoint(config))
        .form(&[
            ("client_id", client_id),
            ("grant_type", DEVICE_CODE_GRANT_TYPE),
            ("device_code", device_code),
        ])
        .send()
        .await
        .map_err(|error| BlobError::Upstream(format!("{action} request failed: {error}")))?;

    if response.status().is_success() {
        let payload = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(|error| {
                BlobError::Upstream(format!("{action} returned invalid JSON: {error}"))
            })?;
        return oauth_session_from_token_response(payload, None).map(Ok);
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if let Ok(payload) = serde_json::from_str::<DeviceCodePollErrorResponse>(&body) {
        return match payload.error.as_str() {
            "authorization_pending" => Ok(Err("authorization_pending")),
            "slow_down" => Ok(Err("slow_down")),
            "authorization_declined" => Err(BlobError::Upstream(
                payload
                    .error_description
                    .unwrap_or_else(|| "user declined the OneDrive device code flow".to_string()),
            )),
            "expired_token" | "bad_verification_code" => Err(BlobError::Upstream(
                payload
                    .error_description
                    .unwrap_or_else(|| "OneDrive device code flow expired".to_string()),
            )),
            _ => Err(BlobError::Upstream(format!(
                "{action} failed with {status}: {}",
                payload.error_description.unwrap_or(body)
            ))),
        };
    }

    Err(BlobError::Upstream(format!(
        "{action} failed with {status}: {}",
        body.trim()
    )))
}

fn read_onedrive_auth_status(state: &AppState) -> OneDriveAuthStatusPayload {
    let effective_config = effective_onedrive_config_from_app(&state.config);
    let config = &effective_config;
    let preferred_mode = if config.use_device_code {
        "device_code"
    } else {
        "web_callback"
    };
    let mut token_state = "missing";
    let mut expires_at_unix = None;
    let mut has_refresh_token = false;

    if let Some(session_file) = config
        .session_file
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Ok(raw) = fs::read_to_string(session_file) {
            let trimmed = raw.trim();
            if let Some(session) = decode_stored_oauth_session(trimmed) {
                expires_at_unix = session.expires_at_unix;
                has_refresh_token = session
                    .refresh_token
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty());
                token_state = if session
                    .expires_at_unix
                    .is_some_and(|expires_at| expires_at <= current_unix_ms() / 1000)
                {
                    "session_expired"
                } else {
                    "session_ready"
                };
            } else if !trimmed.is_empty() {
                token_state = "raw_token_file";
            }
        }
    }

    if token_state == "missing" {
        match &config.token_source {
            TokenSource::Static { bearer } if !bearer.trim().is_empty() => {
                token_state = "inline_token"
            }
            TokenSource::File { path } => {
                if let Ok(raw) = fs::read_to_string(path) {
                    if !raw.trim().is_empty() {
                        token_state = "raw_token_file";
                    }
                }
            }
            TokenSource::EnvVar { key } => {
                if env::var(key)
                    .ok()
                    .is_some_and(|value| !value.trim().is_empty())
                {
                    token_state = "env_token";
                }
            }
            _ => {}
        }
    }

    OneDriveAuthStatusPayload {
        enabled: config.enabled,
        preferred_mode,
        client_id_configured: config
            .client_id
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()),
        redirect_url: config.redirect_url.clone(),
        session_file: config.session_file.clone(),
        auth_base_url: config.auth_base_url.clone(),
        scopes: normalized_onedrive_scopes(config),
        token_state,
        expires_at_unix,
        has_refresh_token,
        pending_pkce_logins: state
            .auth
            .pending_pkce
            .lock()
            .expect("pkce store poisoned")
            .len(),
        pending_device_flows: state
            .auth
            .device_flows
            .lock()
            .expect("device flow store poisoned")
            .len(),
    }
}

fn control_plane_snapshot(state: &AppState) -> ControlPlaneState {
    state
        .control_plane
        .lock()
        .expect("control plane poisoned")
        .clone()
}

fn current_onedrive_policy(state: &AppState) -> OnedrivePolicy {
    control_plane_snapshot(state).onedrive_policy
}

fn current_auth_capture_policy(state: &AppState) -> AuthCapturePolicy {
    control_plane_snapshot(state).auth_capture_policy
}

fn current_auth_capture_policy_payload(state: &AppState) -> AuthCapturePolicyPayload {
    current_auth_capture_policy(state).payload()
}

fn browser_flow_catalog_and_flow<'a>(
    state: &'a AppState,
    provider: &str,
    surface: &str,
    flow_id: &str,
) -> Result<(&'a BrowserFlowCatalog, &'a BrowserFlow), BlobError> {
    let catalog = state
        .browser_flow_catalogs
        .get(provider, surface)
        .ok_or_else(|| {
            BlobError::NotFound(format!(
                "browser flow catalog not found for {provider}/{surface}"
            ))
        })?;
    let flow = catalog
        .find_flow(flow_id)
        .ok_or_else(|| BlobError::NotFound(format!("browser flow not found: {flow_id}")))?;
    Ok((catalog, flow))
}

fn require_browser_flow_coordinates(
    provider: &str,
    surface: &str,
    flow_id: &str,
) -> Result<(String, String, String), BlobError> {
    let provider = provider.trim();
    let surface = surface.trim();
    let flow_id = flow_id.trim();
    if provider.is_empty() || surface.is_empty() || flow_id.is_empty() {
        return Err(BlobError::Configuration(
            "provider, surface, and flow_id are all required".to_string(),
        ));
    }
    Ok((
        provider.to_string(),
        surface.to_string(),
        flow_id.to_string(),
    ))
}

fn resolve_browser_flow_cdp_config(
    state: &AppState,
    input: &BrowserFlowSessionRunInput,
) -> Result<CdpConnectionConfig, BlobError> {
    let policy = current_auth_capture_policy(state);
    let endpoint_url = normalize_secret_field(input.cdp_endpoint_url.clone())
        .or(policy.cdp_endpoint_url)
        .ok_or_else(|| {
            BlobError::Configuration(
                "cdp endpoint_url is required; set it in the request or auth-capture policy"
                    .to_string(),
            )
        })?;
    let target_selector =
        normalize_secret_field(input.cdp_target_selector.clone()).or(policy.cdp_target_selector);
    let target_timeout_ms = input
        .cdp_target_timeout_ms
        .filter(|value| *value > 0)
        .or(policy.cdp_target_timeout_ms.filter(|value| *value > 0));

    Ok(CdpConnectionConfig {
        endpoint_url,
        target_selector,
        target_timeout_ms,
    })
}

fn auth_prompt_field_kind_for_input(input: &BrowserFlowInput) -> AuthPromptFieldKind {
    let haystack = format!(
        "{} {} {}",
        input.id,
        input.label,
        input.description.as_deref().unwrap_or("")
    )
    .to_ascii_lowercase();

    if haystack.contains("sms") || haystack.contains("验证码") {
        return AuthPromptFieldKind::SmsCode;
    }
    if haystack.contains("phone") || haystack.contains("mobile") || haystack.contains("手机号") {
        return AuthPromptFieldKind::PhoneNumber;
    }
    if haystack.contains("captcha") || haystack.contains("图形码") {
        return AuthPromptFieldKind::Captcha;
    }

    match input.kind {
        BrowserFlowInputKind::Secret => AuthPromptFieldKind::Password,
        BrowserFlowInputKind::Text
        | BrowserFlowInputKind::File
        | BrowserFlowInputKind::RuntimeValue => AuthPromptFieldKind::Text,
    }
}

fn value_is_present(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.trim().is_empty(),
        serde_json::Value::Array(values) => !values.is_empty(),
        serde_json::Value::Object(values) => !values.is_empty(),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
    }
}

fn merge_answered_prompts_into_inputs(
    state: &AppState,
    session_id: &str,
    inputs: &mut BTreeMap<String, serde_json::Value>,
) {
    let prompts = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned");
    for prompt in prompts.values() {
        if prompt.session_id.as_deref() != Some(session_id) || !prompt.answer_present {
            continue;
        }
        let Some(input_id) = prompt.input_id.as_deref() else {
            continue;
        };
        if inputs.contains_key(input_id) {
            continue;
        }
        if let Some(value) = prompt.answer_value.as_deref() {
            inputs.insert(
                input_id.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
    }
}

fn auth_session_prompts(state: &AppState, session_id: &str) -> Vec<AuthCapturePrompt> {
    let mut prompts = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned")
        .values()
        .filter(|prompt| prompt.session_id.as_deref() == Some(session_id))
        .cloned()
        .map(|prompt| prompt.sanitized())
        .collect::<Vec<_>>();
    prompts.sort_by_key(|prompt| prompt.created_at_unix_ms);
    prompts.reverse();
    prompts
}

fn browser_flow_auth_session_payload(
    state: &AppState,
    session: &BrowserFlowAuthSession,
) -> BrowserFlowAuthSessionPayload {
    BrowserFlowAuthSessionPayload {
        session_id: session.session_id.clone(),
        provider: session.provider.clone(),
        surface: session.surface.clone(),
        flow_id: session.flow_id.clone(),
        status: session.status.clone(),
        created_at_unix_ms: session.created_at_unix_ms,
        updated_at_unix_ms: session.updated_at_unix_ms,
        prompts: auth_session_prompts(state, &session.session_id),
        report: session.report.clone(),
        last_error: session.last_error.clone(),
    }
}

fn update_browser_flow_auth_session(
    state: &AppState,
    session_id: &str,
    update: impl FnOnce(&mut BrowserFlowAuthSession),
) -> Result<BrowserFlowAuthSession, BlobError> {
    let mut sessions = state
        .auth
        .browser_flow_sessions
        .lock()
        .expect("browser flow auth session store poisoned");
    let session = sessions.get_mut(session_id).ok_or_else(|| {
        BlobError::NotFound(format!("browser flow auth session not found: {session_id}"))
    })?;
    update(session);
    Ok(session.clone())
}

fn merge_browser_flow_auth_session_runtime(
    state: &AppState,
    session_id: &str,
    runtime: BTreeMap<String, serde_json::Value>,
) -> Result<BrowserFlowAuthSession, BlobError> {
    update_browser_flow_auth_session(state, session_id, |session| {
        session.runtime.extend(runtime);
    })
}

fn upsert_browser_flow_auth_session(
    state: &AppState,
    session_id: Option<String>,
    provider: String,
    surface: String,
    flow_id: String,
    inputs: BTreeMap<String, serde_json::Value>,
    runtime: BTreeMap<String, serde_json::Value>,
) -> BrowserFlowAuthSession {
    let session_id = normalize_secret_field(session_id).unwrap_or_else(|| random_urlsafe_token(16));
    let mut sessions = state
        .auth
        .browser_flow_sessions
        .lock()
        .expect("browser flow auth session store poisoned");
    let session = sessions.entry(session_id.clone()).or_insert_with(|| {
        BrowserFlowAuthSession::new(
            session_id.clone(),
            provider.clone(),
            surface.clone(),
            flow_id.clone(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
    });
    session.apply_request(provider, surface, flow_id, inputs, runtime);
    session.clone()
}

fn missing_required_browser_flow_inputs<'a>(
    flow: &'a BrowserFlow,
    inputs: &BTreeMap<String, serde_json::Value>,
) -> Vec<&'a BrowserFlowInput> {
    flow.inputs
        .iter()
        .filter(|input| input.required)
        .filter(|input| {
            inputs
                .get(&input.id)
                .is_none_or(|value| !value_is_present(value))
        })
        .collect()
}

fn ensure_auth_capture_prompts_for_inputs(
    state: &AppState,
    session: &BrowserFlowAuthSession,
    missing_inputs: &[&BrowserFlowInput],
) -> Vec<AuthCapturePrompt> {
    let mut prompts = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned");
    let mut created_or_existing = Vec::with_capacity(missing_inputs.len());

    for input in missing_inputs {
        if let Some(existing) = prompts
            .values()
            .find(|prompt| {
                prompt.session_id.as_deref() == Some(session.session_id.as_str())
                    && prompt.input_id.as_deref() == Some(input.id.as_str())
                    && prompt.status == "pending"
            })
            .cloned()
        {
            created_or_existing.push(existing.sanitized());
            continue;
        }

        let prompt = AuthCapturePrompt::from_input(AuthCapturePromptCreateInput {
            provider: session.provider.clone(),
            session_id: Some(session.session_id.clone()),
            flow_id: Some(session.flow_id.clone()),
            input_id: Some(input.id.clone()),
            title: format!("{} Input Required", session.flow_id),
            message: input.description.clone().unwrap_or_else(|| {
                format!("Provide {} to continue this browser flow.", input.label)
            }),
            field_label: input.label.clone(),
            field_kind: auth_prompt_field_kind_for_input(input),
            placeholder: None,
        });
        let sanitized = prompt.sanitized();
        prompts.insert(prompt.prompt_id.clone(), prompt);
        created_or_existing.push(sanitized);
    }

    created_or_existing
}

#[cfg(test)]
async fn execute_browser_flow_session<S>(
    catalogs: &BrowserFlowCatalogCollection,
    provider: &str,
    surface: &str,
    flow_id: &str,
    inputs: BTreeMap<String, serde_json::Value>,
    runtime: BTreeMap<String, serde_json::Value>,
    session: S,
) -> Result<BrowserFlowExecutionReport, BlobError>
where
    S: BrowserFlowSession,
{
    let (provider, surface, flow_id) =
        require_browser_flow_coordinates(provider, surface, flow_id)?;
    let plan = catalogs.bind_flow(
        &provider,
        &surface,
        &flow_id,
        &BrowserFlowBindingContext { inputs, runtime },
    )?;
    BrowserFlowSessionExecutor::new(session)
        .execute(&plan)
        .await
}

fn browser_flow_plan(
    catalogs: &BrowserFlowCatalogCollection,
    provider: &str,
    surface: &str,
    flow_id: &str,
    inputs: BTreeMap<String, serde_json::Value>,
    runtime: BTreeMap<String, serde_json::Value>,
) -> Result<blob_core::BoundBrowserFlowPlan, BlobError> {
    let (provider, surface, flow_id) =
        require_browser_flow_coordinates(provider, surface, flow_id)?;
    catalogs.bind_flow(
        &provider,
        &surface,
        &flow_id,
        &BrowserFlowBindingContext { inputs, runtime },
    )
}

fn browser_flow_prerequisite_is_satisfied(
    flow: &BrowserFlow,
    prerequisite_flow: &BrowserFlow,
    runtime: &BTreeMap<String, serde_json::Value>,
) -> bool {
    let Some(prerequisite_flow_id) = flow.prerequisite_flow_id.as_deref() else {
        return true;
    };
    if prerequisite_flow.id != prerequisite_flow_id {
        return false;
    }
    browser_flow_outputs_are_present(prerequisite_flow, runtime)
}

fn browser_flow_outputs_are_present(
    flow: &BrowserFlow,
    runtime: &BTreeMap<String, serde_json::Value>,
) -> bool {
    flow.outputs.iter().all(|output| {
        runtime
            .get(&output.id)
            .is_some_and(|value| value_is_present(value))
    })
}

fn browser_flow_auth_session_snapshot(
    state: &AppState,
    session_id: &str,
) -> Result<BrowserFlowAuthSession, ApiError> {
    state
        .auth
        .browser_flow_sessions
        .lock()
        .expect("browser flow auth session store poisoned")
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            ApiError::from(BlobError::NotFound(format!(
                "browser flow auth session not found: {session_id}"
            )))
        })
}

async fn execute_browser_flow_plan_with_output_capture(
    state: &AppState,
    session_id: &str,
    plan: &blob_core::BoundBrowserFlowPlan,
    session: &CdpBrowserFlowSession,
) -> Result<BrowserFlowExecutionReport, ApiError> {
    let report = BrowserFlowSessionExecutor::new(session.clone())
        .execute(plan)
        .await
        .map_err(|error| {
            let _ = update_browser_flow_auth_session(state, session_id, |auth_session| {
                auth_session.set_failed(&error);
            });
            ApiError::from(error)
        })?;

    let captured_runtime = capture_browser_flow_outputs(plan, session)
        .await
        .map_err(|error| {
            let _ = update_browser_flow_auth_session(state, session_id, |auth_session| {
                auth_session.set_failed(&error);
            });
            ApiError::from(error)
        })?;
    let _ = merge_browser_flow_auth_session_runtime(state, session_id, captured_runtime)
        .map_err(ApiError::from)?;

    Ok(report)
}

async fn run_browser_flow_prerequisite_if_needed(
    state: &AppState,
    provider: &str,
    surface: &str,
    flow: &BrowserFlow,
    session_id: &str,
    merged_inputs: &BTreeMap<String, serde_json::Value>,
    session_runtime: &BTreeMap<String, serde_json::Value>,
    session: &CdpBrowserFlowSession,
) -> Result<(), ApiError> {
    let Some(prerequisite_flow_id) = flow.prerequisite_flow_id.as_deref() else {
        return Ok(());
    };

    let (_, direct_prerequisite_flow) =
        browser_flow_catalog_and_flow(state, provider, surface, prerequisite_flow_id)?;
    if browser_flow_prerequisite_is_satisfied(flow, direct_prerequisite_flow, session_runtime) {
        return Ok(());
    }

    let mut prerequisite_chain = Vec::new();
    let mut cursor_flow_id = Some(prerequisite_flow_id.to_string());
    let mut seen_prerequisites = HashSet::new();
    while let Some(current_flow_id) = cursor_flow_id.take() {
        if !seen_prerequisites.insert(current_flow_id.clone()) {
            return Err(ApiError::from(BlobError::Configuration(format!(
                "browser flow prerequisite cycle detected while executing {current_flow_id}"
            ))));
        }
        let (_, current_flow) =
            browser_flow_catalog_and_flow(state, provider, surface, &current_flow_id)?;
        prerequisite_chain.push(current_flow_id);
        cursor_flow_id = current_flow.prerequisite_flow_id.clone();
    }

    prerequisite_chain.reverse();
    let mut current_runtime = session_runtime.clone();
    for prerequisite_flow_id in prerequisite_chain {
        let (_, prerequisite_flow) =
            browser_flow_catalog_and_flow(state, provider, surface, &prerequisite_flow_id)?;
        if browser_flow_outputs_are_present(prerequisite_flow, &current_runtime) {
            continue;
        }

        let plan = browser_flow_plan(
            state.browser_flow_catalogs.as_ref(),
            provider,
            surface,
            &prerequisite_flow_id,
            merged_inputs.clone(),
            current_runtime.clone(),
        )?;
        let _ = execute_browser_flow_plan_with_output_capture(state, session_id, &plan, session)
            .await?;
        current_runtime = browser_flow_auth_session_snapshot(state, session_id)?.runtime;
    }

    Ok(())
}

async fn capture_browser_flow_outputs(
    plan: &blob_core::BoundBrowserFlowPlan,
    session: &impl BrowserFlowOutputReader,
) -> Result<BTreeMap<String, serde_json::Value>, BlobError> {
    let mut captured = BTreeMap::new();
    for output in &plan.flow.outputs {
        let value = match output.kind {
            BrowserFlowOutputKind::ScriptValue => {
                session.evaluate_output_script(&output.source).await?
            }
            BrowserFlowOutputKind::Url => match session.read_current_url().await? {
                Some(url) => serde_json::Value::String(url),
                None => serde_json::Value::Null,
            },
            BrowserFlowOutputKind::RequestHeader
            | BrowserFlowOutputKind::RequestField
            | BrowserFlowOutputKind::ResponseField
            | BrowserFlowOutputKind::DomText => continue,
        };
        if !matches!(value, serde_json::Value::Null) {
            captured.insert(output.id.clone(), value);
        }
    }
    Ok(captured)
}

#[async_trait::async_trait]
impl BrowserFlowOutputReader for CdpBrowserFlowSession {
    async fn evaluate_output_script(
        &self,
        expression: &str,
    ) -> Result<serde_json::Value, BlobError> {
        self.evaluate_value(expression).await
    }

    async fn read_current_url(&self) -> Result<Option<String>, BlobError> {
        self.current_url().await
    }
}

fn browser_flow_catalog_summary_payloads(
    state: &AppState,
) -> Vec<BrowserFlowCatalogSummaryPayload> {
    state
        .browser_flow_catalogs
        .entries()
        .iter()
        .map(|entry| BrowserFlowCatalogSummaryPayload {
            provider: entry.catalog.provider.clone(),
            surface: entry.catalog.surface.clone(),
            flow_count: entry.catalog.flows.len(),
            source_path: entry.source_path.display().to_string(),
        })
        .collect()
}

fn runtime_topology_payload(topology: &TopologyPolicy) -> RuntimeTopologyPayload {
    RuntimeTopologyPayload {
        primary_provider: topology.primary_provider_name(),
        sync_targets: topology.sync_target_names(),
        fallback_read_order: topology.fallback_read_order_names(),
    }
}

fn desired_topology_payload(state: &AppState) -> DesiredTopologyPayload {
    let desired = control_plane_snapshot(state).topology;
    DesiredTopologyPayload {
        primary_provider: desired.primary_provider_name(),
        sync_targets: desired.sync_target_names(),
        fallback_read_order: desired.fallback_read_order_names(),
        restart_required: false,
    }
}

fn unavailable_health(backend: &DynBackend, error: BlobError) -> blob_core::ServiceHealth {
    blob_core::ServiceHealth {
        backend: backend.name().to_string(),
        status: blob_core::HealthStatus::Unavailable,
        capabilities: backend.capabilities(),
        scopes: Vec::new(),
        notes: vec![error.to_string()],
    }
}

async fn provider_health_payloads(state: &AppState) -> Result<Vec<BackendPayload>, BlobError> {
    let topology = runtime_topology(state);
    let primary_backend = backend_for_provider(state, topology.primary_provider)?;
    let sync_backends = sync_backends_for_topology(state, &topology);
    let mut providers = Vec::with_capacity(1 + sync_backends.len());

    let primary_health = match primary_backend.health().await {
        Ok(health) => health,
        Err(error) => unavailable_health(&primary_backend, error),
    };
    providers.push(BackendPayload {
        role: "primary",
        provider: topology.primary_provider_name(),
        health: primary_health,
    });

    for backend in &sync_backends {
        let health = match backend.backend.health().await {
            Ok(health) => health,
            Err(error) => unavailable_health(&backend.backend, error),
        };
        providers.push(BackendPayload {
            role: "sync_target",
            provider: backend.provider.as_str(),
            health,
        });
    }

    Ok(providers)
}

fn replication_state_payload(state: &AppState) -> Result<ReplicationStatePayload, BlobError> {
    let persisted = state
        .metadata_store
        .snapshot(state.config.metadata_snapshot_recent_limit)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;
    let target_statuses = build_replication_target_status_payloads(state, &persisted);
    let latest_failed_jobs = state
        .metadata_store
        .latest_failed_jobs(None)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;

    Ok(ReplicationStatePayload {
        in_memory: state.replication.snapshot(),
        persisted,
        target_statuses,
        latest_failed_jobs,
    })
}

fn build_replication_target_status_payloads(
    state: &AppState,
    persisted: &MetadataSnapshot,
) -> Vec<ReplicationTargetStatusPayload> {
    let topology = runtime_topology(state);
    let mut statuses = HashMap::<String, MetadataTargetStatus>::new();
    for status in &persisted.target_statuses {
        statuses.insert(status.target.clone(), status.clone());
    }

    let mut target_names = topology
        .sync_target_names()
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for target in statuses.keys() {
        if !target_names.iter().any(|name| name == target) {
            target_names.push(target.clone());
        }
    }
    target_names.sort();

    target_names
        .into_iter()
        .map(|target| {
            let status = statuses.remove(&target);
            ReplicationTargetStatusPayload {
                provider: target.clone(),
                label: provider_label_name(&target).to_string(),
                queued_count: status.as_ref().map(|item| item.queued_count).unwrap_or(0),
                pending_count: status.as_ref().map(|item| item.pending_count).unwrap_or(0),
                retry_scheduled_count: status
                    .as_ref()
                    .map(|item| item.retry_scheduled_count)
                    .unwrap_or(0),
                completed_count: status
                    .as_ref()
                    .map(|item| item.completed_count)
                    .unwrap_or(0),
                failed_count: status.as_ref().map(|item| item.failed_count).unwrap_or(0),
                latest_job: status.and_then(|item| item.latest_job),
            }
        })
        .collect()
}

fn build_admin_alerts(
    state: &AppState,
    provider_health: &[BackendPayload],
    replication_state: &ReplicationStatePayload,
    onedrive_auth: &OneDriveAuthStatusPayload,
) -> Vec<AdminAlertPayload> {
    let mut alerts = Vec::new();
    let topology = runtime_topology(state);
    let now_unix_ms = current_unix_ms();

    for provider in provider_health {
        let notes = if provider.health.notes.is_empty() {
            "No extra notes.".to_string()
        } else {
            provider.health.notes.join(" | ")
        };
        match provider.health.status {
            blob_core::HealthStatus::Healthy => {}
            blob_core::HealthStatus::Degraded => alerts.push(AdminAlertPayload {
                severity: "warn",
                title: format!("{} is degraded", provider_label_name(provider.provider)),
                detail: format!("role={} | {}", provider.role, notes),
            }),
            blob_core::HealthStatus::Unavailable => alerts.push(AdminAlertPayload {
                severity: if provider.role == "primary" {
                    "error"
                } else {
                    "warn"
                },
                title: format!("{} is unavailable", provider_label_name(provider.provider)),
                detail: format!("role={} | {}", provider.role, notes),
            }),
        }
    }

    let matured_failed_objects = replication_state
        .latest_failed_jobs
        .iter()
        .filter(|job| {
            now_unix_ms.saturating_sub(job.enqueued_at_unix_ms as u64)
                >= state.config.replication_failed_alert_min_age_ms
        })
        .count();
    if matured_failed_objects >= state.config.replication_failed_alert_threshold {
        alerts.push(AdminAlertPayload {
            severity: "error",
            title: format!(
                "{} latest failed replication object(s) exceeded alert threshold",
                matured_failed_objects
            ),
            detail: format!(
                "threshold={} min_age_ms={} | Check Latest Failed Objects or recent replication jobs for details.",
                state.config.replication_failed_alert_threshold,
                state.config.replication_failed_alert_min_age_ms
            ),
        });
    }

    if state.config.replication_workers == 0 && !topology.sync_targets.is_empty() {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Replication workers are disabled".to_string(),
            detail: format!(
                "{} sync target(s) are configured, but CCBG_REPLICATION_WORKERS=0 so pending jobs will not drain.",
                topology.sync_targets.len()
            ),
        });
    }

    if state.config.data_plane_max_in_flight <= 2 {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Data plane concurrency is set very low".to_string(),
            detail: format!(
                "CCBG_DATA_PLANE_MAX_IN_FLIGHT={} | This is safe for tiny routers, but concurrent clients may see 503 responses sooner.",
                state.config.data_plane_max_in_flight
            ),
        });
    }

    if !socket_addr_is_loopback(&state.config.admin_bind_addr) {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Admin Web is exposed beyond loopback".to_string(),
            detail: format!(
                "admin_bind_addr={} | Soft-router deployments should normally keep the admin UI bound to 127.0.0.1 and publish it only through an explicit trusted tunnel or reverse proxy.",
                state.config.admin_bind_addr
            ),
        });
    }

    if matches!(state.config.admin_mode, AdminMode::Web)
        && !socket_addr_is_loopback(&state.config.auth_callback_bind_addr)
    {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "OAuth callback listener is exposed beyond loopback".to_string(),
            detail: format!(
                "auth_callback_bind_addr={} | Keep the callback listener loopback-only unless you intentionally terminate and filter it elsewhere.",
                state.config.auth_callback_bind_addr
            ),
        });
    }

    if !socket_addr_is_loopback(&state.config.metrics_bind_addr) {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Metrics endpoint is exposed beyond loopback".to_string(),
            detail: format!(
                "metrics_bind_addr={} | On routers, prefer loopback-only health/metrics and let an upstream collector or tunnel fetch it if needed.",
                state.config.metrics_bind_addr
            ),
        });
    }

    if state.config.s3_secret_access_key == "change-me" {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "S3 secret is still using the example default".to_string(),
            detail: "CCBG_S3_SECRET_ACCESS_KEY still equals the example value `change-me`; rotate it before exposing the S3 endpoint to any non-local client.".to_string(),
        });
    }

    if replication_state.in_memory.pending_count != replication_state.persisted.pending_count {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Replication queue counts differ".to_string(),
            detail: format!(
                "in_memory_pending={} persisted_pending={}",
                replication_state.in_memory.pending_count,
                replication_state.persisted.pending_count
            ),
        });
    }

    if replication_state.persisted.pending_count > 32 {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "Replication backlog is growing".to_string(),
            detail: format!(
                "{} pending jobs are persisted locally.",
                replication_state.persisted.pending_count
            ),
        });
    }

    let onedrive_in_use = topology.sync_targets.contains(&ProviderId::Onedrive)
        || topology.fallback_read_order.contains(&ProviderId::Onedrive);
    if onedrive_in_use
        && matches!(
            onedrive_auth.token_state,
            "missing" | "raw_token_file" | "env_token" | "inline_token"
        )
    {
        alerts.push(AdminAlertPayload {
            severity: "warn",
            title: "OneDrive is configured but OAuth session is not fully established".to_string(),
            detail: format!(
                "token_state={} | built-in refresh support is strongest when session_file contains refresh_token.",
                onedrive_auth.token_state
            ),
        });
    }

    alerts
}

fn alerts_fingerprint(alerts: &[AdminAlertPayload]) -> String {
    let payload = serde_json::to_vec(alerts).unwrap_or_default();
    sha256_hex(&payload)
}

fn sign_notify_payload(secret: &str, timestamp: u64, body: &[u8]) -> String {
    let payload_hash = sha256_hex(body);
    let string_to_sign = format!("{timestamp}.{payload_hash}");
    hex::encode(hmac_sha256(secret.as_bytes(), string_to_sign.as_bytes()))
}

async fn notify_loop(state: AppState) {
    loop {
        if let Err(error) = process_notify_tick(&state).await {
            warn!(error = %error, "notify webhook tick failed");
            let mut notify_state = state.notify_state.lock().expect("notify state poisoned");
            notify_state.last_attempt_at_unix_ms = Some(current_unix_ms());
            notify_state.last_error = Some(error.to_string());
        }
        sleep(Duration::from_secs(
            state.config.notify_poll_interval_seconds,
        ))
        .await;
    }
}

async fn process_notify_tick(state: &AppState) -> Result<()> {
    let Some(webhook_url) = state.config.notify_webhook_url.as_deref() else {
        return Ok(());
    };

    let replication_state = replication_state_payload(state)?;
    let provider_health = provider_health_payloads(state).await?;
    let onedrive_auth = read_onedrive_auth_status(state);
    let alerts = build_admin_alerts(state, &provider_health, &replication_state, &onedrive_auth);
    let object_action_history = control_plane_snapshot(state).object_action_history;
    let monitoring = monitoring_summary_payload(
        &provider_health,
        &replication_state,
        &object_action_history,
        &alerts,
    );
    let alert_hash = alerts_fingerprint(&alerts);

    {
        let notify_state = state.notify_state.lock().expect("notify state poisoned");
        if notify_state.last_alert_hash.as_deref() == Some(alert_hash.as_str()) {
            return Ok(());
        }
    }

    let event_id = random_urlsafe_token(18);
    let payload = NotifyWebhookPayload {
        event_id: event_id.clone(),
        service: "carrier-cloud-blob-gateway",
        emitted_at_unix_ms: current_unix_ms(),
        runtime: runtime_status_payload(state),
        monitoring,
        alerts,
    };
    let payload_body =
        serde_json::to_vec(&payload).context("failed to serialize notify webhook payload")?;
    let timestamp = current_unix_ms();

    {
        let mut notify_state = state.notify_state.lock().expect("notify state poisoned");
        notify_state.last_attempt_at_unix_ms = Some(current_unix_ms());
        notify_state.last_error = None;
    }

    let mut request = state
        .auth
        .http_client
        .post(webhook_url)
        .header(CONTENT_TYPE, "application/json")
        .header(NOTIFY_EVENT_ID_HEADER, event_id)
        .header(NOTIFY_TIMESTAMP_HEADER, timestamp.to_string())
        .body(payload_body.clone());
    if let Some(secret) = state.config.notify_webhook_signing_secret.as_deref() {
        let signature = sign_notify_payload(secret, timestamp, &payload_body);
        request = request
            .header(NOTIFY_SIGNATURE_VERSION_HEADER, "v1")
            .header(NOTIFY_SIGNATURE_HEADER, signature);
    }
    request
        .send()
        .await
        .context("notify webhook request failed")?
        .error_for_status()
        .context("notify webhook returned error status")?;

    let mut notify_state = state.notify_state.lock().expect("notify state poisoned");
    notify_state.last_alert_hash = Some(alert_hash);
    notify_state.last_success_at_unix_ms = Some(current_unix_ms());
    notify_state.last_error = None;
    Ok(())
}

fn provider_label(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Stub => "Local Stub",
        ProviderId::Unicom => "China Unicom",
        ProviderId::Telecom => "China Telecom",
        ProviderId::Mobile => "China Mobile",
        ProviderId::Onedrive => "Microsoft OneDrive",
    }
}

fn provider_label_name(provider: &str) -> &'static str {
    ProviderId::parse(provider)
        .ok()
        .map(provider_label)
        .unwrap_or("Unknown Provider")
}

fn provider_roles(topology: &TopologyPolicy, provider: ProviderId) -> Vec<&'static str> {
    let mut roles = Vec::new();
    if topology.primary_provider == provider {
        roles.push("primary");
    }
    if topology.sync_targets.contains(&provider) {
        roles.push("sync_target");
    }
    if topology.fallback_read_order.contains(&provider) {
        roles.push("fallback");
    }
    roles
}

fn fallback_gate_name(gate: FallbackObjectGate) -> &'static str {
    match gate {
        FallbackObjectGate::Allowed => "allowed",
        FallbackObjectGate::MissingMetadata => "missing_metadata",
        FallbackObjectGate::PendingPut => "pending_put",
        FallbackObjectGate::Deleted => "deleted",
        FallbackObjectGate::PolicyBlocked => "policy_blocked",
    }
}

fn topology_provider_catalog(onedrive_enabled: bool) -> Vec<TopologyProviderOptionPayload> {
    [
        ProviderId::Unicom,
        ProviderId::Telecom,
        ProviderId::Mobile,
        ProviderId::Onedrive,
        ProviderId::Stub,
    ]
    .into_iter()
    .map(|provider| TopologyProviderOptionPayload {
        provider: provider.as_str(),
        label: provider_label(provider),
        can_be_primary: provider.can_be_primary(),
        can_be_sync_target: provider.can_be_sync_target(),
        enabled: provider != ProviderId::Onedrive || onedrive_enabled,
    })
    .collect()
}

async fn admin_index(State(state): State<AppState>) -> Html<String> {
    let auth = read_onedrive_auth_status(&state);
    let desired = desired_topology_payload(&state);
    let runtime = runtime_topology_payload(&runtime_topology(&state));
    let provider_catalog =
        serde_json::to_string(&topology_provider_catalog(state.config.onedrive.enabled))
            .unwrap_or_else(|_| "[]".to_string());
    let onedrive_policy = serde_json::to_string_pretty(&current_onedrive_policy(&state))
        .unwrap_or_else(|_| "{}".to_string());
    let auth_capture_policy =
        serde_json::to_string_pretty(&current_auth_capture_policy_payload(&state))
            .unwrap_or_else(|_| "{}".to_string());
    let body = format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>CCBG Admin</title>
  <style>
    :root {{
      color-scheme: light;
      --bg: #f3efe4;
      --panel: #fffaf0;
      --ink: #1c1b19;
      --accent: #0f766e;
      --muted: #6b665d;
      --border: #d8d0c2;
      --warn: #b45309;
      --ok: #166534;
    }}
    body {{ margin:0; font-family: ui-sans-serif, system-ui, sans-serif; background: linear-gradient(180deg, #f7f1e6 0%, #efe7d9 100%); color: var(--ink); }}
    main {{ max-width: 920px; margin: 0 auto; padding: 32px 20px 48px; }}
    h1 {{ margin: 0 0 8px; font-size: 34px; }}
    p {{ color: var(--muted); }}
    .grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 16px; margin-top: 24px; }}
    .card {{ background: var(--panel); border: 1px solid var(--border); border-radius: 18px; padding: 18px; box-shadow: 0 12px 32px rgba(37, 30, 18, 0.06); }}
    .actions {{ display:flex; gap: 12px; flex-wrap:wrap; margin: 18px 0; }}
    button, a.cta {{ appearance:none; border:0; border-radius: 999px; padding: 10px 16px; background: var(--accent); color:#fff; font-weight: 700; text-decoration:none; cursor:pointer; }}
    button.secondary {{ background:#dbe7e5; color:#12413d; }}
    label {{ display:block; font-weight:600; margin-top: 12px; }}
    input, select, textarea {{ width:100%; box-sizing:border-box; margin-top: 6px; padding: 10px 12px; border-radius: 12px; border:1px solid var(--border); background:#fffef9; }}
    textarea {{ min-height: 96px; resize: vertical; }}
    input[type="checkbox"] {{ width:auto; margin-right: 8px; }}
    code, pre {{ background:#f3eee3; border-radius: 12px; }}
    pre {{ padding: 14px; overflow:auto; white-space: pre-wrap; }}
    .status-ok {{ color: var(--ok); font-weight:700; }}
    .status-warn {{ color: var(--warn); font-weight:700; }}
    .flash {{ min-height: 20px; }}
    .hint {{ font-size: 14px; color: var(--muted); }}
    .provider-grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; margin-top: 14px; }}
    .provider-card {{ background:#fffdf7; border:1px solid var(--border); border-radius: 16px; padding: 14px; }}
    .provider-card.disabled {{ opacity: 0.58; }}
    .provider-card h3 {{ margin: 0; font-size: 18px; }}
    .provider-role {{ margin-top: 10px; display:flex; align-items:center; gap: 8px; font-weight:600; }}
    .provider-role input[type="checkbox"], .provider-role input[type="radio"] {{ margin: 0; }}
    .provider-note {{ margin-top: 10px; font-size: 13px; color: var(--muted); }}
    .pill {{ display:inline-block; padding: 4px 10px; border-radius: 999px; background:#e5f1ef; color:#14514b; font-size: 12px; font-weight:700; }}
    .fallback-list {{ display:flex; flex-direction:column; gap: 8px; margin-top: 12px; }}
    .fallback-row {{ display:flex; align-items:center; justify-content:space-between; gap: 10px; padding: 10px 12px; border-radius: 12px; border:1px solid var(--border); background:#fffef9; }}
    .fallback-actions {{ display:flex; gap: 8px; }}
    .status-bad {{ color: #991b1b; font-weight:700; }}
    .metric-grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(170px, 1fr)); gap: 12px; margin-top: 14px; }}
    .metric-card {{ border:1px solid var(--border); border-radius: 14px; padding: 14px; background:#fffef9; }}
    .metric-card strong {{ display:block; font-size: 26px; margin-top: 6px; }}
    .health-grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; margin-top: 14px; }}
    .health-card {{ border:1px solid var(--border); border-radius: 16px; padding: 14px; background:#fffef9; }}
    .health-card.healthy {{ border-color:#9dd3b9; }}
    .health-card.degraded {{ border-color:#f1c27d; }}
    .health-card.unavailable {{ border-color:#ef9a9a; }}
    .health-card h3 {{ margin:0; font-size:18px; }}
    .health-meta {{ margin-top: 8px; font-size: 13px; color: var(--muted); }}
    .scope-grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 10px; margin-top: 12px; }}
    .scope-card {{ border:1px solid #eadfce; border-radius: 14px; background:#fff9f2; padding: 12px; }}
    .scope-card h4 {{ margin:0; font-size:15px; }}
    .scope-meta {{ margin-top: 6px; font-size: 12px; color: var(--muted); }}
    .health-notes {{ margin-top: 10px; font-size: 13px; color: var(--muted); white-space: pre-wrap; }}
    .alert-list {{ display:flex; flex-direction:column; gap: 10px; margin-top: 14px; }}
    .alert-card {{ border-radius: 16px; padding: 14px; border:1px solid var(--border); background:#fffef9; }}
    .alert-card.warn {{ border-color:#f1c27d; background:#fff8eb; }}
    .alert-card.error {{ border-color:#ef9a9a; background:#fff1f1; }}
    .alert-card h3 {{ margin:0 0 6px; font-size: 16px; }}
    .queue-layout {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 16px; margin-top: 16px; }}
    .table-wrap {{ overflow:auto; border:1px solid var(--border); border-radius: 14px; background:#fffef9; }}
    table {{ width:100%; border-collapse: collapse; font-size: 13px; }}
    th, td {{ padding: 10px 12px; border-bottom:1px solid #ece4d6; text-align:left; vertical-align: top; }}
    th {{ background:#f7f1e6; position: sticky; top: 0; }}
    tr:last-child td {{ border-bottom: 0; }}
    .mono {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
    .object-results {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 12px; margin-top: 14px; }}
    .object-card {{ border:1px solid var(--border); border-radius: 16px; padding: 14px; background:#fffef9; }}
    .object-card h3 {{ margin:0 0 8px; font-size:18px; }}
    .object-meta {{ font-size:13px; color: var(--muted); margin-top: 6px; white-space: pre-wrap; }}
    .field-group.hidden {{ display:none; }}
    .preview-card {{ border:1px solid var(--border); border-radius: 14px; padding: 12px; background:#fffef9; margin-top: 12px; }}
    .preview-card strong {{ display:block; margin-bottom: 6px; }}
    .delta-list {{ margin: 8px 0 0; padding-left: 18px; }}
    .delta-list li {{ margin-top: 4px; }}
    .comparison-grid {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 10px; margin-top: 12px; }}
    .comparison-card {{ border:1px solid #eadfce; border-radius: 14px; background:#fff9f2; padding: 12px; }}
    .comparison-card h4 {{ margin:0; font-size:15px; }}
    .history-list {{ display:flex; flex-direction:column; gap: 10px; margin-top: 12px; }}
    .history-card {{ border:1px solid var(--border); border-radius: 14px; background:#fffef9; padding: 12px; }}
    .history-card h4 {{ margin:0; font-size:15px; }}
    .history-toolbar {{ display:grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 10px; margin-top: 12px; }}
    .inline-controls {{ display:flex; flex-wrap:wrap; gap: 12px; align-items:center; margin-top: 12px; }}
  </style>
</head>
<body>
  <main>
    <h1>Carrier Cloud Blob Gateway</h1>
    <p>OneDrive async backup control plane. Web and terminal entrypoints are both exposed here.</p>
    <div class="actions">
      <a class="cta" href="/api/auth/onedrive/web/start">Connect OneDrive In Browser</a>
      <button class="secondary" id="device-start">Start Device Code</button>
      <button class="secondary" id="reload-status">Refresh Status</button>
    </div>
    <div class="inline-controls">
      <label><input id="status-auto-refresh-enabled" type="checkbox" checked /> Auto-refresh dashboard</label>
      <label>Refresh Every (s)
        <input id="status-auto-refresh-interval-seconds" type="number" min="5" step="5" value="15" style="width:96px; margin-left:8px;" />
      </label>
      <div id="status-refresh-summary" class="hint">Auto-refreshing dashboard every 15s.</div>
    </div>
    <div class="grid">
      <section class="card">
        <h2>Runtime</h2>
        <div id="runtime-summary" class="metric-grid"></div>
        <details>
          <summary>Raw runtime payload</summary>
          <pre id="runtime-json">Loading…</pre>
        </details>
      </section>
      <section class="card">
        <h2>Monitoring Summary</h2>
        <div id="monitoring-summary" class="metric-grid"></div>
        <div id="monitoring-failures" class="health-notes">Loading monitoring summary…</div>
      </section>
      <section class="card">
        <h2>Operations Overview</h2>
        <div id="operations-overview" class="metric-grid"></div>
        <div id="operations-overview-notes" class="health-notes">Loading operations overview…</div>
      </section>
      <section class="card">
        <h2>Notify</h2>
        <div id="notify-summary" class="health-notes">Loading notify status…</div>
      </section>
      <section class="card">
        <h2>OneDrive Auth</h2>
        <div id="auth-summary" class="metric-grid"></div>
        <details>
          <summary>Raw auth payload</summary>
          <pre id="auth-status">{auth_json}</pre>
        </details>
      </section>
      <section class="card">
        <h2>Runtime Topology</h2>
        <div id="runtime-topology-summary" class="health-notes">Loading topology…</div>
        <details>
          <summary>Raw topology payload</summary>
          <pre id="runtime-topology">{runtime_topology}</pre>
        </details>
      </section>
      <section class="card">
        <h2>Terminal Flow</h2>
        <div id="device-flow-summary" class="hint">Start a device-code login only when browser PKCE is unavailable.</div>
        <details>
          <summary>Device flow log</summary>
          <pre id="device-output">POST /api/auth/onedrive/device/start to get a device code flow.</pre>
        </details>
      </section>
    </div>
    <div class="grid">
      <section class="card">
        <h2>Alerts</h2>
        <div id="admin-alerts" class="alert-list">
          <div class="alert-card">
            <h3>Loading alerts…</h3>
            <div>Waiting for the first status refresh.</div>
          </div>
        </div>
      </section>
      <section class="card">
        <h2>Provider Health</h2>
        <div id="provider-health-grid" class="health-grid"></div>
        <pre id="provider-test-output">Click "Test Now" on any provider card to run an on-demand check.</pre>
      </section>
    </div>
    <section class="card" style="margin-top: 16px;">
      <h2>Replication Queue</h2>
      <p>Watch pending copy/delete work here. Failed jobs stay visible in the recent history table with the last error message.</p>
      <div id="replication-feedback" class="flash"></div>
      <div id="replication-metrics" class="metric-grid"></div>
      <div style="margin-top: 16px;">
        <h3>Target Status</h3>
        <div id="replication-targets" class="table-wrap"></div>
      </div>
      <div style="margin-top: 16px;">
        <h3>Latest Failed Objects</h3>
        <div class="inline-controls">
          <label>Target
            <select id="replication-failed-target-filter">
              <option value="all">all</option>
            </select>
          </label>
          <label>Object
            <input id="replication-failed-object-filter" type="text" placeholder="bucket/key or error" />
          </label>
          <label>Start
            <input id="replication-failed-start-filter" type="datetime-local" />
          </label>
          <label>End
            <input id="replication-failed-end-filter" type="datetime-local" />
          </label>
          <button id="export-replication-failed-json" class="secondary" type="button">Export Failed JSON</button>
          <button id="export-replication-failed-csv" class="secondary" type="button">Export Failed CSV</button>
        </div>
        <div id="replication-failed-summary" class="hint">No latest failed objects.</div>
        <div id="replication-failed" class="table-wrap"></div>
      </div>
      <div class="queue-layout">
        <div>
          <h3>Pending Jobs</h3>
          <div id="replication-pending" class="table-wrap"></div>
        </div>
        <div>
          <h3>Recent Jobs</h3>
          <div id="replication-recent" class="table-wrap"></div>
        </div>
      </div>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Object Status</h2>
      <p>Inspect one bucket/key pair to see where the object exists, which providers can currently serve it, and what the latest replication metadata says.</p>
      <div class="grid" style="margin-top: 0;">
        <div>
          <label>Bucket</label>
          <input id="object-status-bucket" type="text" placeholder="agent-memory" />
        </div>
        <div>
          <label>Key</label>
          <input id="object-status-key" type="text" placeholder="sessions/thread-1.json" />
        </div>
      </div>
      <div class="actions">
        <button id="inspect-object-status">Inspect Object</button>
      </div>
      <div id="object-status-feedback" class="flash"></div>
      <div id="object-status-summary" class="hint">No object inspected yet.</div>
      <div id="object-status-results" class="object-results"></div>
      <details>
        <summary>Raw object status payload</summary>
        <pre id="object-status-json">Run an object inspection to see the raw JSON payload here.</pre>
      </details>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Object Actions</h2>
      <p>Run admin-level rename/copy/move actions against the current primary provider. Successful actions also update replication metadata so fallback state stays coherent.</p>
      <div class="grid" style="margin-top: 0;">
        <div>
          <label>Action</label>
          <select id="object-action-kind">
            <option value="rename">rename</option>
            <option value="copy">copy</option>
            <option value="move">move</option>
          </select>
        </div>
      </div>
      <div id="object-action-rename-fields">
        <div class="grid" style="margin-top: 0;">
          <div>
            <label>Bucket</label>
            <input id="object-action-rename-bucket" type="text" placeholder="family" />
          </div>
          <div>
            <label>Key</label>
            <input id="object-action-rename-key" type="text" placeholder="shared/note.txt" />
          </div>
          <div>
            <label>New Key</label>
            <input id="object-action-rename-new-key" type="text" placeholder="shared/renamed.txt" />
          </div>
        </div>
      </div>
      <div id="object-action-transfer-fields" class="field-group hidden">
        <div class="grid" style="margin-top: 0;">
          <div>
            <label>Source Bucket</label>
            <input id="object-action-source-bucket" type="text" placeholder="family" />
          </div>
          <div>
            <label>Source Key</label>
            <input id="object-action-source-key" type="text" placeholder="shared/renamed.txt" />
          </div>
          <div>
            <label>Destination Bucket</label>
            <input id="object-action-destination-bucket" type="text" placeholder="root" />
          </div>
          <div>
            <label>Destination Key</label>
            <input id="object-action-destination-key" type="text" placeholder="docs/copied.txt" />
          </div>
        </div>
      </div>
      <div class="actions">
        <button id="run-object-action">Run Object Action</button>
      </div>
      <div class="grid" style="margin-top: 0;">
        <div>
          <label>Operator</label>
          <input id="object-action-operator" type="text" placeholder="alice" />
        </div>
        <div>
          <label>Ticket / Change ID</label>
          <input id="object-action-ticket" type="text" placeholder="CHG-2026-0514" />
        </div>
      </div>
      <label>Notes</label>
      <textarea id="object-action-notes" placeholder="Reason for this action, risk notes, rollback context..."></textarea>
      <div id="object-action-feedback" class="flash"></div>
      <div id="object-action-preview" class="preview-card">
        <strong>Execution Preview</strong>
        <div id="object-action-preview-summary" class="hint">Select an action and fill the fields to see risks before execution.</div>
      </div>
      <div id="object-action-summary" class="hint">No object action has been run from the admin UI yet.</div>
      <div id="object-action-inspection-summary" class="hint">No before/after object inspection captured yet.</div>
      <div id="object-action-inspection-results" class="object-results"></div>
      <div class="history-toolbar">
        <div>
          <label>History Action Filter</label>
          <select id="object-action-history-action-filter">
            <option value="all">all</option>
            <option value="rename">rename</option>
            <option value="copy">copy</option>
            <option value="move">move</option>
          </select>
        </div>
        <div>
          <label>History Outcome Filter</label>
          <select id="object-action-history-outcome-filter">
            <option value="all">all</option>
            <option value="success">success</option>
            <option value="failed">failed</option>
          </select>
        </div>
        <div>
          <label>History Provider Filter</label>
          <select id="object-action-history-provider-filter">
            <option value="all">all</option>
          </select>
        </div>
        <div>
          <label>History Operator Filter</label>
          <input id="object-action-history-operator-filter" type="text" placeholder="alice" />
        </div>
        <div>
          <label>History Object Filter</label>
          <input id="object-action-history-object-filter" type="text" placeholder="family/shared/note.txt" />
        </div>
        <div>
          <label>History Start Time</label>
          <input id="object-action-history-start-filter" type="datetime-local" />
        </div>
        <div>
          <label>History End Time</label>
          <input id="object-action-history-end-filter" type="datetime-local" />
        </div>
      </div>
      <div class="actions">
        <button id="clear-object-action-history" class="secondary" type="button">Clear Shared History</button>
        <button id="export-object-action-history" class="secondary" type="button">Export Shared History</button>
        <button id="export-object-action-history-csv" class="secondary" type="button">Export Shared History CSV</button>
      </div>
      <div id="object-action-history-summary" class="hint">No shared object action history yet.</div>
      <div id="object-action-history" class="history-list"></div>
      <details>
        <summary>Raw object action payload</summary>
        <pre id="object-action-json">Submit an object action to see the request payload and result here.</pre>
      </details>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Live Primary / Sync Topology</h2>
      <p>Changes here apply immediately. New requests use the new primary provider, while already queued replication jobs keep their recorded source provider.</p>
      <div id="topology-feedback" class="flash"></div>
      <div id="topology-provider-grid" class="provider-grid"></div>
      <label>Fallback Read Order</label>
      <p class="hint">Fallback is optional. Only providers enabled below will be used for read fallback when the primary provider cannot serve data.</p>
      <div id="topology-fallback-order" class="fallback-list"></div>
      <div class="actions">
        <button id="save-topology">Apply Topology Live</button>
      </div>
      <div id="desired-topology-summary" class="hint">Loading desired topology…</div>
      <details>
        <summary>Raw desired topology payload</summary>
        <pre id="desired-topology">{desired_topology}</pre>
      </details>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>OneDrive Backup Scope</h2>
      <p>Use this to disable OneDrive completely, back up everything, or only back up Hermes / OpenClaw memory buckets or prefixes.</p>
      <label><input id="onedrive-replication-enabled" type="checkbox" /> Enable async copy to OneDrive</label>
      <label><input id="onedrive-fallback-enabled" type="checkbox" /> Allow fallback reads from OneDrive</label>
      <label>Scope</label>
      <select id="onedrive-scope-mode">
        <option value="all">all</option>
        <option value="memory_only">memory_only</option>
      </select>
      <label>Memory Buckets (comma separated)</label>
      <input id="onedrive-memory-buckets" type="text" />
      <label>Memory Prefixes (comma separated)</label>
      <input id="onedrive-memory-prefixes" type="text" />
      <div class="actions">
        <button id="save-onedrive-policy">Save OneDrive Policy</button>
      </div>
      <div id="onedrive-policy-summary" class="hint">Loading OneDrive backup policy…</div>
      <details>
        <summary>Raw OneDrive policy payload</summary>
        <pre id="onedrive-policy">{onedrive_policy}</pre>
      </details>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Auth Capture / LLM</h2>
      <p>Use this only on larger hosts or point the lite gateway to a remote auth-broker. Small-memory routers should keep capture disabled locally and delegate it to another device.</p>
      <label><input id="auth-capture-enabled" type="checkbox" /> Enable auth capture sidecar integration</label>
      <label>Auth Broker URL</label>
      <input id="auth-capture-broker-url" type="text" placeholder="http://auth-broker-host:port" />
      <label>CDP Endpoint URL</label>
      <input id="auth-capture-cdp-endpoint-url" type="text" placeholder="http://127.0.0.1:9222" />
      <label>CDP Target Selector</label>
      <input id="auth-capture-cdp-target-selector" type="text" placeholder="title:pan.wo.cn | url:https://pan.wo.cn/* | ws://..." />
      <label>CDP Target Timeout (ms)</label>
      <input id="auth-capture-cdp-target-timeout-ms" type="number" min="1" step="1" placeholder="15000" />
      <label><input id="auth-capture-llm-enabled" type="checkbox" /> Allow LLM-assisted auth analysis</label>
      <label>LLM Endpoint</label>
      <input id="auth-capture-llm-endpoint" type="text" placeholder="http://192.168.1.36:1234/v1" />
      <label>LLM Model ID</label>
      <input id="auth-capture-llm-model-id" type="text" placeholder="supergemma4-26b-uncensored-v2" />
      <label>LLM API Key</label>
      <input id="auth-capture-llm-api-key" type="password" placeholder="Paste provider API key only when you want to set or rotate it" />
      <label><input id="auth-capture-clear-llm-api-key" type="checkbox" /> Clear stored LLM API key</label>
      <div class="actions">
        <button id="save-auth-capture-policy">Save Auth Capture Settings</button>
      </div>
      <div id="auth-capture-summary" class="hint">Loading auth capture policy…</div>
      <details>
        <summary>Raw auth capture policy payload</summary>
        <pre id="auth-capture-policy-json">{auth_capture_policy}</pre>
      </details>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Pending Verification Inputs</h2>
      <p>If carrier login needs a phone number, SMS code, password, or captcha, the auth-broker should push a prompt here instead of hanging in the background.</p>
      <div id="auth-capture-prompts-feedback" class="flash"></div>
      <div id="auth-capture-prompts" class="provider-grid"></div>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Provider Credentials</h2>
      <p>Each provider stores its auth overrides in a dedicated JSON file. Leave a field blank to clear the stored override and fall back to env/default values.</p>
      <div id="provider-credentials-feedback" class="flash"></div>
      <div id="provider-credentials-grid" class="provider-grid"></div>
    </section>
    <section class="card" style="margin-top: 16px;">
      <h2>Diagnostics</h2>
      <p>Raw status payloads stay here for debugging, but the main dashboard above should remain readable without JSON.</p>
      <details>
        <summary>Gateway status JSON</summary>
        <pre id="gateway-status">Loading…</pre>
      </details>
    </section>
  </main>
  <script>
    const providerCredentialsCatalog = [
      {{
        provider: 'unicom',
        label: 'China Unicom',
        fields: [
          {{ key: 'token', label: 'Access Token', multiline: true, placeholder: 'Paste browser token / accessToken here' }},
          {{ key: 'cookie_header', label: 'Cookie Header', multiline: true, placeholder: 'cookie1=value; cookie2=value' }},
          {{ key: 'family_id', label: 'Family ID (Optional)', placeholder: 'Used to discover and expose the family bucket when available' }},
        ],
      }},
      {{
        provider: 'telecom',
        label: 'China Telecom',
        fields: [
          {{ key: 'browser_id', label: 'Browser ID', placeholder: 'Paste the Browser-Id header value' }},
          {{ key: 'cookie_header', label: 'Cookie Header', multiline: true, placeholder: 'cookie1=value; cookie2=value' }},
          {{ key: 'token', label: 'Access Token (Optional)', multiline: true, placeholder: 'Only needed if the upstream later requires signed requests' }},
          {{ key: 'root_folder_id', label: 'Root Folder ID', placeholder: 'Default -11' }},
        ],
      }},
      {{
        provider: 'mobile',
        label: 'China Mobile',
        fields: [
          {{ key: 'token', label: 'Access Token', multiline: true, placeholder: 'Paste browser token here' }},
          {{ key: 'cookie_header', label: 'Cookie Header', multiline: true, placeholder: 'cookie1=value; cookie2=value' }},
        ],
      }},
      {{
        provider: 'onedrive',
        label: 'Microsoft OneDrive',
        fields: [
          {{ key: 'client_id', label: 'Client ID', placeholder: 'Application (client) ID' }},
          {{ key: 'tenant', label: 'Tenant', placeholder: 'common / organizations / tenant-id' }},
          {{ key: 'drive_id', label: 'Drive ID', placeholder: 'Optional drive id' }},
          {{ key: 'redirect_url', label: 'Redirect URL', placeholder: 'http://host:port/auth/onedrive/callback' }},
          {{ key: 'token', label: 'Manual Access Token', multiline: true, placeholder: 'Optional manual override token' }},
        ],
      }},
    ];
    let providerCredentialState = {{}};
    let topologyProviderCatalog = {provider_catalog};
    let topologyDraft = {{
      primary_provider: 'unicom',
      sync_targets: [],
      fallback_read_order: [],
    }};
    let runtimeTopologyState = {{
      primary_provider: 'unicom',
      sync_targets: [],
      fallback_read_order: [],
    }};
    let authPromptState = new Map();
    let objectActionHistory = [];
    let objectActionHistoryLimit = {DEFAULT_OBJECT_ACTION_HISTORY_LIMIT};
    let latestFailedJobs = [];
    let replicationStateSnapshot = {{
      in_memory: {{ pending_jobs: [] }},
      persisted: {{ recent_jobs: [] }},
      target_statuses: [],
      latest_failed_jobs: [],
    }};
    let statusAutoRefreshTimer = null;
    let statusRefreshInFlight = false;
    let lastStatusRefreshUnixMs = null;

    function csvToArray(value) {{
      return value.split(',').map(part => part.trim()).filter(Boolean);
    }}
    function dedupe(items) {{
      return [...new Set((items || []).filter(Boolean))];
    }}
    function providerLabel(provider) {{
      const entry = topologyProviderCatalog.find(item => item.provider === provider);
      return entry ? entry.label : provider;
    }}
    function formatBytes(value) {{
      if (value === null || value === undefined || Number.isNaN(Number(value))) {{
        return 'unknown';
      }}
      const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
      let size = Number(value);
      let unit = 0;
      while (size >= 1024 && unit < units.length - 1) {{
        size /= 1024;
        unit += 1;
      }}
      return `${{size >= 100 || unit === 0 ? size.toFixed(0) : size.toFixed(1)}} ${{units[unit]}}`;
    }}
    function fetchJson(url, options) {{
      return fetch(url, options).then(async response => {{
        const payload = await response.json().catch(() => ({{}}));
        if (!response.ok) {{
          throw new Error(payload.error || `request failed: ${{response.status}}`);
        }}
        return payload;
      }});
    }}
    function setTopologyFeedback(message, tone) {{
      const node = document.getElementById('topology-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function setObjectStatusFeedback(message, tone) {{
      const node = document.getElementById('object-status-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function setObjectActionFeedback(message, tone) {{
      const node = document.getElementById('object-action-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function setProviderCredentialsFeedback(message, tone) {{
      const node = document.getElementById('provider-credentials-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function setReplicationFeedback(message, tone) {{
      const node = document.getElementById('replication-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function setAuthPromptFeedback(message, tone) {{
      const node = document.getElementById('auth-capture-prompts-feedback');
      node.textContent = message || '';
      node.className = tone === 'ok' ? 'flash status-ok' : 'flash status-warn';
    }}
    function escapeHtml(value) {{
      return String(value ?? '')
        .replaceAll('&', '&amp;')
        .replaceAll('<', '&lt;')
        .replaceAll('>', '&gt;');
    }}
    function statusClass(status) {{
      if (status === 'healthy' || status === 'completed') {{
        return 'status-ok';
      }}
      if (status === 'degraded' || status === 'pending' || status === 'retry_scheduled') {{
        return 'status-warn';
      }}
      return 'status-bad';
    }}
    function formatTimestamp(unixMs) {{
      if (unixMs === null || unixMs === undefined) {{
        return 'n/a';
      }}
      const date = new Date(Number(unixMs));
      if (Number.isNaN(date.getTime())) {{
        return String(unixMs);
      }}
      return date.toLocaleString();
    }}
    function formatDurationMs(ms) {{
      const totalSeconds = Math.max(0, Math.floor(Number(ms || 0) / 1000));
      const hours = Math.floor(totalSeconds / 3600);
      const minutes = Math.floor((totalSeconds % 3600) / 60);
      const seconds = totalSeconds % 60;
      if (hours > 0) {{
        return `${{hours}}h ${{minutes}}m ${{seconds}}s`;
      }}
      if (minutes > 0) {{
        return `${{minutes}}m ${{seconds}}s`;
      }}
      return `${{seconds}}s`;
    }}
    function parseDateTimeLocalToUnixMs(value) {{
      if (!value) {{
        return null;
      }}
      const parsed = new Date(value).getTime();
      return Number.isNaN(parsed) ? null : parsed;
    }}
    function renderStatusRefreshSummary(errorMessage) {{
      const enabled = !!document.getElementById('status-auto-refresh-enabled')?.checked;
      const rawInterval = Number(document.getElementById('status-auto-refresh-interval-seconds')?.value || 15);
      const intervalSeconds = Number.isFinite(rawInterval) && rawInterval >= 5 ? rawInterval : 15;
      const parts = [enabled ? `Auto-refreshing dashboard every ${{intervalSeconds}}s.` : 'Auto-refresh paused.'];
      if (lastStatusRefreshUnixMs) {{
        parts.push(`Last refresh: ${{formatTimestamp(lastStatusRefreshUnixMs)}}.`);
      }}
      if (errorMessage) {{
        parts.push(`Last error: ${{errorMessage}}.`);
      }}
      document.getElementById('status-refresh-summary').textContent = parts.join(' ');
    }}
    function stopStatusAutoRefresh() {{
      if (statusAutoRefreshTimer) {{
        clearInterval(statusAutoRefreshTimer);
        statusAutoRefreshTimer = null;
      }}
      renderStatusRefreshSummary();
    }}
    function startStatusAutoRefresh() {{
      stopStatusAutoRefresh();
      const enabled = !!document.getElementById('status-auto-refresh-enabled')?.checked;
      if (!enabled) {{
        return;
      }}
      const rawInterval = Number(document.getElementById('status-auto-refresh-interval-seconds')?.value || 15);
      const intervalSeconds = Number.isFinite(rawInterval) && rawInterval >= 5 ? rawInterval : 15;
      statusAutoRefreshTimer = setInterval(() => {{
        refreshStatus({{ lightweight: true }});
      }}, intervalSeconds * 1000);
      renderStatusRefreshSummary();
    }}
    function renderRuntimeSummary(runtime) {{
      const container = document.getElementById('runtime-summary');
      document.getElementById('runtime-json').textContent = JSON.stringify(runtime || {{}}, null, 2);
      container.innerHTML = `
        <div class="metric-card">
          <div>Uptime</div>
          <strong>${{escapeHtml(formatDurationMs(runtime?.uptime_ms))}}</strong>
        </div>
        <div class="metric-card">
          <div>Admin Mode</div>
          <strong>${{escapeHtml(runtime?.admin_mode || 'n/a')}}</strong>
        </div>
        <div class="metric-card">
          <div>Data Plane</div>
          <strong>${{escapeHtml(runtime?.bind_addr || 'n/a')}}</strong>
        </div>
        <div class="metric-card">
          <div>Replication Workers</div>
          <strong>${{escapeHtml(String(runtime?.replication_workers ?? 'n/a'))}}</strong>
        </div>
        <div class="metric-card">
          <div>History Limit</div>
          <strong>${{escapeHtml(String(runtime?.object_action_history_limit ?? 'n/a'))}}</strong>
        </div>
        <div class="metric-card">
          <div>Started</div>
          <strong>${{escapeHtml(formatTimestamp(runtime?.started_at_unix_ms))}}</strong>
        </div>
      `;
    }}
    function renderMonitoringSummary(monitoring) {{
      const summaryNode = document.getElementById('monitoring-summary');
      const failuresNode = document.getElementById('monitoring-failures');
      const providerSummary = monitoring?.provider_summary || {{}};
      const replication = monitoring?.replication || {{}};
      const objectActions = monitoring?.object_actions || {{}};
      const latestFailedObjects = monitoring?.latest_failed_objects || [];
      summaryNode.innerHTML = `
        <div class="metric-card">
          <div>Open Alerts</div>
          <strong>${{escapeHtml(String(monitoring?.open_alert_count ?? 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Healthy Providers</div>
          <strong>${{escapeHtml(`${{providerSummary.healthy ?? 0}} / ${{providerSummary.total ?? 0}}`)}}</strong>
        </div>
        <div class="metric-card">
          <div>Replication Pending</div>
          <strong>${{escapeHtml(String(replication.pending_jobs ?? 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Replication Failed</div>
          <strong>${{escapeHtml(String(replication.failed_jobs ?? 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Latest Failed Objects</div>
          <strong>${{escapeHtml(String(latestFailedObjects.length || 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Action Failures</div>
          <strong>${{escapeHtml(String(objectActions.failed_entries ?? 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Last Object Action</div>
          <strong>${{escapeHtml(formatTimestamp(objectActions.last_action_at_unix_ms))}}</strong>
        </div>
      `;
      const recentFailures = monitoring?.recent_failures || [];
      if (!recentFailures.length) {{
        failuresNode.textContent = 'No recent failed replication jobs or object actions.';
        return;
      }}
      failuresNode.textContent = recentFailures.map(item => {{
        const parts = [
          `kind=${{item.kind || 'unknown'}}`,
          item.provider ? `provider=${{providerLabel(item.provider) || item.provider}}` : null,
          item.target ? `target=${{item.target}}` : null,
          item.action ? `action=${{item.action}}` : null,
          item.object ? `object=${{item.object}}` : null,
          item.occurred_at_unix_ms ? `at=${{formatTimestamp(item.occurred_at_unix_ms)}}` : null,
          `message=${{item.message || 'none'}}`,
        ].filter(Boolean);
        return parts.join(' | ');
      }}).join('\n');
    }}
    function renderNotifySummary(notify) {{
      const container = document.getElementById('notify-summary');
      const summary = [
        `Webhook enabled: ${{notify?.webhook_enabled ? 'yes' : 'no'}}`,
        `Poll interval: ${{notify?.poll_interval_seconds ?? 'n/a'}}s`,
        `Last attempt: ${{formatTimestamp(notify?.last_attempt_at_unix_ms)}}`,
        `Last success: ${{formatTimestamp(notify?.last_success_at_unix_ms)}}`,
        `Last error: ${{notify?.last_error || 'none'}}`,
      ].join('\n');
      container.textContent = summary;
    }}
    function renderOperationsOverview(overview) {{
      const summaryNode = document.getElementById('operations-overview');
      const notesNode = document.getElementById('operations-overview-notes');
      const syncTargets = overview?.sync_targets || [];
      const fallbackReadOrder = overview?.fallback_read_order || [];
      const oldestPending = overview?.oldest_pending_job_age_ms;
      const oldestFailed = overview?.oldest_latest_failed_object_age_ms;
      const notifyFreshness = overview?.notify_last_success_age_ms;
      summaryNode.innerHTML = `
        <div class="metric-card">
          <div>Primary Write</div>
          <strong>${{escapeHtml(providerLabel(overview?.primary_provider || 'n/a'))}}</strong>
        </div>
        <div class="metric-card">
          <div>Replication Mode</div>
          <strong>${{escapeHtml(overview?.replication_mode || 'n/a')}}</strong>
        </div>
        <div class="metric-card">
          <div>Sync Targets</div>
          <strong>${{escapeHtml(String(syncTargets.length))}}</strong>
        </div>
        <div class="metric-card">
          <div>Latest Failed Objects</div>
          <strong>${{escapeHtml(String(overview?.latest_failed_objects ?? 0))}}</strong>
        </div>
        <div class="metric-card">
          <div>Oldest Pending</div>
          <strong>${{escapeHtml(oldestPending === null || oldestPending === undefined ? 'n/a' : formatDurationMs(oldestPending))}}</strong>
        </div>
        <div class="metric-card">
          <div>Oldest Failed Object</div>
          <strong>${{escapeHtml(oldestFailed === null || oldestFailed === undefined ? 'n/a' : formatDurationMs(oldestFailed))}}</strong>
        </div>
        <div class="metric-card">
          <div>Notify Freshness</div>
          <strong>${{escapeHtml(notifyFreshness === null || notifyFreshness === undefined ? 'n/a' : formatDurationMs(notifyFreshness))}}</strong>
        </div>
      `;
      const notes = [
        `Primary write: ${{providerLabel(overview?.primary_provider || 'unknown')}}`,
        `Async backup targets: ${{syncTargets.length ? syncTargets.map(providerLabel).join(', ') : 'none'}}`,
        `Fallback read order: ${{fallbackReadOrder.length ? fallbackReadOrder.map(providerLabel).join(' -> ') : 'disabled'}}`,
        `OneDrive async backup: ${{overview?.onedrive_async_backup_enabled ? 'enabled' : 'disabled'}}`,
        `OneDrive fallback: ${{overview?.onedrive_fallback_enabled ? 'enabled' : 'disabled'}}`,
        `Data plane concurrency: max=${{overview?.data_plane_max_in_flight ?? 'n/a'}} | available=${{overview?.data_plane_permits_available ?? 'n/a'}}`,
        `Loopback-only listeners: data=${{overview?.data_plane_loopback_only ? 'yes' : 'no'}} | admin=${{overview?.admin_loopback_only ? 'yes' : 'no'}} | callback=${{overview?.auth_callback_loopback_only ? 'yes' : 'no'}} | metrics=${{overview?.metrics_loopback_only ? 'yes' : 'no'}}`,
        `S3 secret rotated: ${{overview?.s3_secret_uses_default ? 'no' : 'yes'}}`,
        `Replication workers: ${{overview?.replication_workers ?? 'n/a'}}`,
        `Pending jobs: ${{overview?.pending_jobs ?? 0}} | retry scheduled: ${{overview?.retry_scheduled_jobs ?? 0}} | latest failed objects: ${{overview?.latest_failed_objects ?? 0}}`,
        `Failed alert gate: threshold=${{overview?.replication_failed_alert_threshold ?? 'n/a'}} | min_age_ms=${{overview?.replication_failed_alert_min_age_ms ?? 'n/a'}}`,
        `Last object action age: ${{overview?.latest_object_action_age_ms === null || overview?.latest_object_action_age_ms === undefined ? 'n/a' : formatDurationMs(overview.latest_object_action_age_ms)}}`,
        `Notify webhook: ${{overview?.notify_webhook_enabled ? 'enabled' : 'disabled'}} | last success age: ${{notifyFreshness === null || notifyFreshness === undefined ? 'n/a' : formatDurationMs(notifyFreshness)}}`,
        `Notify last error: ${{overview?.notify_last_error || 'none'}}`,
      ];
      notesNode.textContent = notes.join('\n');
    }}
    function renderAuthSummary(auth) {{
      const container = document.getElementById('auth-summary');
      const tokenState = auth?.token_state || 'unknown';
      container.innerHTML = `
        <div class="metric-card">
          <div>Mode</div>
          <strong>${{escapeHtml(auth?.preferred_mode || 'n/a')}}</strong>
        </div>
        <div class="metric-card">
          <div>Token</div>
          <strong class="${{statusClass(tokenState === 'ready' ? 'healthy' : (tokenState === 'missing' ? 'unavailable' : 'degraded'))}}">${{escapeHtml(tokenState)}}</strong>
        </div>
        <div class="metric-card">
          <div>Refresh Token</div>
          <strong>${{auth?.has_refresh_token ? 'yes' : 'no'}}</strong>
        </div>
        <div class="metric-card">
          <div>Redirect / Session</div>
          <strong>${{auth?.redirect_url ? 'configured' : 'manual'}}</strong>
        </div>
      `;
    }}
    function renderRuntimeTopologySummary(payload) {{
      const summary = [
        `Primary write: ${{providerLabel(payload.primary_provider)}}`,
        `Async sync: ${{(payload.sync_targets || []).length ? payload.sync_targets.map(providerLabel).join(', ') : 'none'}}`,
        `Fallback read order: ${{(payload.fallback_read_order || []).length ? payload.fallback_read_order.map(providerLabel).join(' -> ') : 'disabled'}}`,
      ].join('\\n');
      document.getElementById('runtime-topology-summary').textContent = summary;
    }}
    function renderDesiredTopologySummary(payload) {{
      const summary = [
        `Planned primary: ${{providerLabel(payload.primary_provider)}}`,
        `Planned sync targets: ${{(payload.sync_targets || []).length ? payload.sync_targets.map(providerLabel).join(', ') : 'none'}}`,
        `Planned fallback order: ${{(payload.fallback_read_order || []).length ? payload.fallback_read_order.map(providerLabel).join(' -> ') : 'disabled'}}`,
        `Restart required: ${{payload.restart_required ? 'yes' : 'no'}}`,
      ].join('\\n');
      document.getElementById('desired-topology-summary').textContent = summary;
    }}
    function renderOnedrivePolicySummary(payload) {{
      const scope = payload.scope_mode || 'all';
      const summary = [
        `Async backup: ${{payload.replication_enabled ? 'enabled' : 'disabled'}}`,
        `Fallback read: ${{payload.fallback_enabled ? 'enabled' : 'disabled'}}`,
        `Scope: ${{scope}}`,
        `Memory buckets: ${{(payload.memory_buckets || []).length ? payload.memory_buckets.join(', ') : 'none'}}`,
        `Memory prefixes: ${{(payload.memory_prefixes || []).length ? payload.memory_prefixes.join(', ') : 'none'}}`,
      ].join('\\n');
      document.getElementById('onedrive-policy-summary').textContent = summary;
    }}
    function renderAuthCapturePolicy(payload) {{
      document.getElementById('auth-capture-enabled').checked = !!payload.enabled;
      document.getElementById('auth-capture-broker-url').value = payload.broker_url || '';
      document.getElementById('auth-capture-cdp-endpoint-url').value = payload.cdp_endpoint_url || '';
      document.getElementById('auth-capture-cdp-target-selector').value = payload.cdp_target_selector || '';
      document.getElementById('auth-capture-cdp-target-timeout-ms').value = payload.cdp_target_timeout_ms || '';
      document.getElementById('auth-capture-llm-enabled').checked = !!payload.llm_analysis_enabled;
      document.getElementById('auth-capture-llm-endpoint').value = payload.llm_endpoint || '';
      document.getElementById('auth-capture-llm-model-id').value = payload.llm_model_id || '';
      document.getElementById('auth-capture-llm-api-key').value = '';
      document.getElementById('auth-capture-clear-llm-api-key').checked = false;
      document.getElementById('auth-capture-policy-json').textContent = JSON.stringify(payload, null, 2);
      const summary = [
        `Capture sidecar: ${{payload.enabled ? 'enabled' : 'disabled'}}`,
        `Broker URL: ${{payload.broker_url || 'not set'}}`,
        `CDP endpoint: ${{payload.cdp_endpoint_url || 'not set'}}`,
        `CDP target: ${{payload.cdp_target_selector || 'not set'}}`,
        `CDP timeout: ${{payload.cdp_target_timeout_ms || 'default'}} ms`,
        `LLM analysis: ${{payload.llm_analysis_enabled ? 'enabled' : 'disabled'}}`,
        `LLM endpoint: ${{payload.llm_endpoint || 'not set'}}`,
        `LLM model: ${{payload.llm_model_id || 'not set'}}`,
        `LLM API key: ${{payload.llm_api_key_present ? 'configured' : 'not set'}}`,
      ].join('\\n');
      document.getElementById('auth-capture-summary').textContent = summary;
    }}
    function authPromptInputType(kind) {{
      return kind === 'password' ? 'password' : 'text';
    }}
    function renderAuthCapturePrompts(prompts) {{
      const container = document.getElementById('auth-capture-prompts');
      authPromptState.clear();
      (prompts || []).forEach(prompt => authPromptState.set(prompt.prompt_id, prompt));
      if (!prompts || !prompts.length) {{
        container.innerHTML = `
          <div class="provider-card">
            <h3>No pending verification input</h3>
            <div class="provider-note">The auth-broker has not asked for phone numbers, SMS codes, or captcha values.</div>
          </div>
        `;
        return;
      }}
      container.innerHTML = prompts.map(prompt => `
        <div class="provider-card">
          <h3>${{escapeHtml(prompt.title || providerLabel(prompt.provider))}}</h3>
          <div class="provider-note">Provider: <span class="mono">${{escapeHtml(prompt.provider || 'unknown')}}</span></div>
          <div class="provider-note">Created: ${{escapeHtml(formatTimestamp(prompt.created_at_unix_ms))}}</div>
          <div class="provider-note">${{escapeHtml(prompt.message || '')}}</div>
          <label>${{escapeHtml(prompt.field_label || 'Input')}}
            <input id="auth-prompt-${{prompt.prompt_id}}" type="${{authPromptInputType(prompt.field_kind)}}" placeholder="${{escapeHtml(prompt.placeholder || '')}}" />
          </label>
          <div class="actions">
            <button class="secondary" type="button" data-auth-prompt-reply="${{prompt.prompt_id}}">Submit Input</button>
          </div>
          <div class="provider-note">Status: <span class="${{statusClass(prompt.status === 'answered' ? 'healthy' : 'pending')}}">${{escapeHtml(prompt.status || 'pending')}}</span></div>
        </div>
      `).join('');
    }}
    function renderAlerts(alerts) {{
      const container = document.getElementById('admin-alerts');
      if (!alerts || !alerts.length) {{
        container.innerHTML = `
          <div class="alert-card">
            <h3>No active alerts</h3>
            <div>Provider health, replication queue, and OneDrive session state all look acceptable right now.</div>
          </div>
        `;
        return;
      }}
      container.innerHTML = alerts.map(alert => `
        <div class="alert-card ${{alert.severity === 'error' ? 'error' : 'warn'}}">
          <h3>${{escapeHtml(alert.title)}}</h3>
          <div>${{escapeHtml(alert.detail)}}</div>
        </div>
      `).join('');
    }}
    function renderProviderHealth(providers) {{
      const container = document.getElementById('provider-health-grid');
      if (!providers || !providers.length) {{
        container.innerHTML = `
          <div class="health-card unavailable">
            <h3>No providers</h3>
            <div class="health-meta">No provider health data is available.</div>
          </div>
        `;
        return;
      }}
      container.innerHTML = providers.map(provider => {{
        const notes = (provider.health.notes || []).length
          ? provider.health.notes.map(note => escapeHtml(note)).join('<br>')
          : 'No extra notes.';
        const status = provider.health.status || 'unavailable';
        const scopes = (provider.health.scopes || []).map(scope => {{
          const meta = [];
          meta.push(`kind=${{scope.kind || 'unknown'}}`);
          meta.push(`writable=${{scope.writable ? 'yes' : 'no'}}`);
          if (scope.root) {{
            meta.push(`root=${{scope.root}}`);
          }}
          if (scope.container) {{
            meta.push(`container=${{scope.container}}`);
          }}
          if (scope.object_count !== null && scope.object_count !== undefined) {{
            meta.push(`root_entries=${{scope.object_count}}`);
          }}
          const capacity = scope.capacity
            ? `total=${{formatBytes(scope.capacity.total_bytes)}} | used=${{formatBytes(scope.capacity.used_bytes)}} | free=${{formatBytes(scope.capacity.free_bytes)}}`
            : 'capacity=unknown';
          const scopeNotes = (scope.notes || []).length
            ? escapeHtml(scope.notes.join(' | '))
            : 'No extra scope notes.';
          return `
            <div class="scope-card">
              <h4>${{escapeHtml(scope.label || scope.id || 'scope')}}</h4>
              <div class="scope-meta">${{escapeHtml(meta.join(' | '))}}</div>
              <div class="scope-meta">${{escapeHtml(capacity)}}</div>
              <div class="scope-meta">${{scopeNotes}}</div>
            </div>
          `;
        }}).join('');
        return `
          <div class="health-card ${{status}}">
            <h3>${{escapeHtml(providerLabel(provider.provider))}}</h3>
            <div class="health-meta">
              <span class="${{statusClass(status)}}">${{escapeHtml(status)}}</span>
              · role=${{escapeHtml(provider.role)}}
              · backend=${{escapeHtml(provider.health.backend || provider.provider)}}
            </div>
            <div class="health-meta">
              read=${{provider.health.capabilities?.read ? 'yes' : 'no'}}
              · write=${{provider.health.capabilities?.write ? 'yes' : 'no'}}
              · delete=${{provider.health.capabilities?.delete ? 'yes' : 'no'}}
            </div>
            <div class="actions">
              <button class="secondary" type="button" data-provider-test="${{provider.provider}}">Test Now</button>
            </div>
            <div class="scope-grid">${{scopes || '<div class="scope-card"><h4>No storage scopes</h4><div class="scope-meta">This provider did not report any storage partitions.</div></div>'}}</div>
            <div class="health-notes">${{notes}}</div>
          </div>
        `;
      }}).join('');
    }}
    function renderJobsTable(elementId, jobs) {{
      const container = document.getElementById(elementId);
      if (!jobs || !jobs.length) {{
        container.innerHTML = `
          <table>
            <tbody>
              <tr><td>No jobs</td></tr>
            </tbody>
          </table>
        `;
        return;
      }}
      container.innerHTML = `
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Status</th>
              <th>Route</th>
              <th>Object</th>
              <th>Attempts</th>
              <th>Next Retry</th>
              <th>Error</th>
              <th>Queued</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            ${{
              jobs.map(job => `
                <tr>
                  <td class="mono">${{job.job_id}}</td>
                  <td><span class="${{statusClass(job.status)}}">${{escapeHtml(job.status)}}</span><br><span class="mono">${{escapeHtml(job.operation)}}</span></td>
                  <td class="mono">${{escapeHtml(job.source_provider || 'n/a')}} → ${{escapeHtml(job.target)}}</td>
                  <td class="mono">${{escapeHtml(job.object.bucket)}}/${{escapeHtml(job.object.key)}}</td>
                  <td>${{job.attempts || 0}}</td>
                  <td>${{escapeHtml(formatTimestamp(job.next_attempt_at_unix_ms))}}</td>
                  <td>${{escapeHtml(job.last_error || '') || 'None'}}</td>
                  <td>${{escapeHtml(formatTimestamp(job.enqueued_at_unix_ms))}}</td>
                  <td>${{job.status === 'failed' ? `<button class="secondary" type="button" data-retry-replication-job="${{job.job_id}}">Retry</button>` : ''}}</td>
                </tr>
              `).join('')
            }}
          </tbody>
        </table>
      `;
    }}
    async function retryReplicationJob(jobId) {{
      setReplicationFeedback(`Retrying replication job ${{jobId}}…`, 'warn');
      try {{
        const result = await fetchJson(`/api/replication/jobs/${{encodeURIComponent(jobId)}}/retry`, {{
          method: 'POST',
        }});
        await refreshStatus();
        setReplicationFeedback(
          `Replication job ${{result.job_id}} requeued for ${{providerLabel(result.target)}} -> ${{result.bucket}}/${{result.key}}.`,
          'ok'
        );
      }} catch (error) {{
        setReplicationFeedback(error.message, 'warn');
      }}
    }}
    async function retryFailedReplicationTarget(target) {{
      setReplicationFeedback(`Retrying latest failed replication jobs for ${{providerLabel(target)}}…`, 'warn');
      try {{
        const result = await fetchJson(`/api/replication/targets/${{encodeURIComponent(target)}}/retry-failed`, {{
          method: 'POST',
        }});
        await refreshStatus();
        if (!result.retried_jobs) {{
          setReplicationFeedback(`No latest failed jobs to retry for ${{providerLabel(result.target)}}.`, 'ok');
          return;
        }}
        const retriedSummary = (result.jobs || [])
          .map(job => `${{job.bucket}}/${{job.key}} (#${{job.job_id}})`)
          .join(', ');
        setReplicationFeedback(
          `Requeued ${{result.retried_jobs}} latest failed job(s) for ${{providerLabel(result.target)}}: ${{retriedSummary}}.`,
          'ok'
        );
      }} catch (error) {{
        setReplicationFeedback(error.message, 'warn');
      }}
    }}
    function renderReplication(replicationState) {{
      if (!replicationState) {{
        return;
      }}
      replicationStateSnapshot.in_memory = replicationState.in_memory || {{ pending_jobs: [] }};
      replicationStateSnapshot.persisted = replicationState.persisted || {{ recent_jobs: [] }};
      replicationStateSnapshot.target_statuses = replicationState.target_statuses || [];
      replicationStateSnapshot.latest_failed_jobs = replicationState.latest_failed_jobs || [];
      latestFailedJobs.splice(0, latestFailedJobs.length, ...((replicationState.latest_failed_jobs || [])));
      updateReplicationFailedTargetFilter(latestFailedJobs);
      document.getElementById('replication-metrics').innerHTML = `
        <div class="metric-card">
          <div>In-memory Pending</div>
          <strong>${{replicationState.in_memory.pending_count || 0}}</strong>
        </div>
        <div class="metric-card">
          <div>Persisted Queue</div>
          <strong>${{replicationState.persisted.pending_count || 0}}</strong>
        </div>
        <div class="metric-card">
          <div>Failed Jobs</div>
          <strong>${{replicationState.persisted.failed_count || 0}}</strong>
        </div>
        <div class="metric-card">
          <div>Retry Scheduled</div>
          <strong>${{replicationState.persisted.retry_scheduled_count || 0}}</strong>
        </div>
        <div class="metric-card">
          <div>Completed History</div>
          <strong>${{replicationState.persisted.completed_count || 0}}</strong>
        </div>
      `;
      const targets = replicationState.target_statuses || [];
      const targetNode = document.getElementById('replication-targets');
      if (!targets.length) {{
        targetNode.innerHTML = `
          <table>
            <tbody>
              <tr><td>No target-level replication state yet</td></tr>
            </tbody>
          </table>
        `;
      }} else {{
        targetNode.innerHTML = `
          <table>
            <thead>
              <tr>
                <th>Target</th>
                <th>Queued</th>
                <th>Retry</th>
                <th>Completed</th>
                <th>Failed</th>
                <th>Latest</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              ${{
                targets.map(target => {{
                  const latest = target.latest_job
                    ? `job=${{target.latest_job.job_id}} | status=${{target.latest_job.status}} | attempts=${{target.latest_job.attempts}} | next=${{formatTimestamp(target.latest_job.next_attempt_at_unix_ms)}} | error=${{target.latest_job.last_error || 'none'}}`
                    : 'No jobs yet';
                  return `
                    <tr>
                      <td><strong>${{escapeHtml(target.label || providerLabel(target.provider))}}</strong><br><span class="mono">${{escapeHtml(target.provider)}}</span></td>
                      <td>${{target.queued_count || 0}}<br><span class="mono">pending=${{target.pending_count || 0}}</span></td>
                      <td>${{target.retry_scheduled_count || 0}}</td>
                      <td>${{target.completed_count || 0}}</td>
                      <td>${{target.failed_count || 0}}</td>
                      <td class="mono">${{escapeHtml(latest)}}</td>
                      <td>${{(target.failed_count || 0) > 0 ? `<button class="secondary" type="button" data-retry-replication-target="${{target.provider}}">Retry Failed</button>` : ''}}</td>
                    </tr>
                  `;
                }}).join('')
              }}
            </tbody>
          </table>
        `;
      }}
      renderJobsTable('replication-pending', (replicationState.in_memory.pending_jobs || []).slice(0, 12));
      renderJobsTable('replication-recent', (replicationState.persisted.recent_jobs || []).slice(0, 12));

      const failedJobs = filteredReplicationFailedJobs(latestFailedJobs);
      const failedSummary = document.getElementById('replication-failed-summary');
      const failedNode = document.getElementById('replication-failed');
      const filters = replicationFailedJobFilters();
      const summaryParts = [`${{failedJobs.length}} latest failed object entr${{failedJobs.length === 1 ? 'y' : 'ies'}}`];
      if (filters.target !== 'all') {{
        summaryParts.push(`for ${{providerLabel(filters.target)}}`);
      }}
      if (filters.object) {{
        summaryParts.push(`matching "${{filters.object}}"`);
      }}
      if (filters.start_unix_ms !== null) {{
        summaryParts.push(`since ${{formatTimestamp(filters.start_unix_ms)}}`);
      }}
      if (filters.end_unix_ms !== null) {{
        summaryParts.push(`until ${{formatTimestamp(filters.end_unix_ms)}}`);
      }}
      failedSummary.textContent = `${{summaryParts.join(' ')}}.`;
      if (!failedJobs.length) {{
        failedNode.innerHTML = `
          <table>
            <tbody>
              <tr><td>No latest failed objects for the current filter.</td></tr>
            </tbody>
          </table>
        `;
        return;
      }}
      failedNode.innerHTML = `
        <table>
          <thead>
            <tr>
              <th>ID</th>
              <th>Route</th>
              <th>Object</th>
              <th>Attempts</th>
              <th>Queued</th>
              <th>Next Retry</th>
              <th>Error</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            ${{failedJobs.map(job => `
              <tr>
                <td class="mono">${{job.job_id}}</td>
                <td class="mono">${{escapeHtml(job.source_provider || 'n/a')}} → ${{escapeHtml(job.target)}}</td>
                <td class="mono">${{escapeHtml(job.object.bucket)}}/${{escapeHtml(job.object.key)}}</td>
                <td>${{job.attempts || 0}}</td>
                <td>${{escapeHtml(formatTimestamp(job.enqueued_at_unix_ms))}}</td>
                <td>${{escapeHtml(formatTimestamp(job.next_attempt_at_unix_ms))}}</td>
                <td>${{escapeHtml(job.last_error || '') || 'None'}}</td>
                <td><button class="secondary" type="button" data-retry-replication-job="${{job.job_id}}">Retry</button></td>
              </tr>
            `).join('')}}
          </tbody>
        </table>
      `;
    }}
    function renderObjectStatus(payload) {{
      document.getElementById('object-status-json').textContent = JSON.stringify(payload, null, 2);
      const summary = payload.gateway_read_source
        ? `Gateway would currently read from ${{providerLabel(payload.gateway_read_source)}}${{payload.gateway_fallback_from ? ` (fallback from ${{providerLabel(payload.gateway_fallback_from)}})` : ''}}.`
        : `Gateway read resolution failed: ${{payload.gateway_error || 'unknown error'}}`;
      document.getElementById('object-status-summary').textContent = summary;
      const container = document.getElementById('object-status-results');
      container.innerHTML = (payload.provider_states || []).map(state => {{
        const meta = [];
        meta.push(`roles=${{(state.roles || []).join(', ') || 'none'}}`);
        meta.push(`exists=${{state.exists ? 'yes' : 'no'}}`);
        meta.push(`readable_via_gateway=${{state.readable_via_gateway ? 'yes' : 'no'}}`);
        if (state.accepts_replication_put !== null && state.accepts_replication_put !== undefined) {{
          meta.push(`accepts_replication_put=${{state.accepts_replication_put ? 'yes' : 'no'}}`);
        }}
        if (state.fallback_order_index) {{
          meta.push(`fallback_order=#${{state.fallback_order_index}}`);
        }}
        if (state.object_info?.size !== undefined) {{
          meta.push(`size=${{state.object_info.size}}`);
        }}
        if (state.object_info?.content_type) {{
          meta.push(`content_type=${{state.object_info.content_type}}`);
        }}
        if (state.object_info?.etag) {{
          meta.push(`etag=${{state.object_info.etag}}`);
        }}
        const latestJob = state.latest_replication_job
          ? `job_id=${{state.latest_replication_job.job_id}} | status=${{state.latest_replication_job.status}} | operation=${{state.latest_replication_job.operation}} | source=${{state.latest_replication_job.source_provider || 'n/a'}} | attempts=${{state.latest_replication_job.attempts}} | next_retry=${{formatTimestamp(state.latest_replication_job.next_attempt_at_unix_ms)}} | last_error=${{state.latest_replication_job.last_error || 'none'}}`
          : 'No replication job metadata.';
        const fallbackState = state.fallback_gate
          ? `fallback_gate=${{state.fallback_gate}} | reason=${{state.fallback_reason || 'n/a'}}`
          : 'fallback_gate=n/a';
        const access = state.access_error ? `access_error=${{state.access_error}}` : 'access_error=none';
        return `
          <div class="object-card">
            <h3>${{escapeHtml(providerLabel(state.provider))}}</h3>
            <div class="${{state.readable_via_gateway ? 'status-ok' : (state.exists ? 'status-warn' : 'status-bad')}}">${{state.readable_via_gateway ? 'Gateway-readable' : (state.exists ? 'Exists but blocked' : 'Not present')}}</div>
            <div class="object-meta">${{escapeHtml(meta.join(' | '))}}</div>
            <div class="object-meta">${{escapeHtml(fallbackState)}}</div>
            <div class="object-meta">${{escapeHtml(latestJob)}}</div>
            <div class="object-meta">${{escapeHtml(access)}}</div>
          </div>
        `;
      }}).join('');
    }}
    function renderObjectActionEditor() {{
      const action = document.getElementById('object-action-kind').value;
      document.getElementById('object-action-rename-fields').className =
        action === 'rename' ? '' : 'field-group hidden';
      document.getElementById('object-action-transfer-fields').className =
        action === 'rename' ? 'field-group hidden' : '';
      renderObjectActionPreview();
    }}
    function objectParentPath(key) {{
      const trimmed = String(key || '').trim().replace(/^\/+|\/+$/g, '');
      if (!trimmed.includes('/')) {{
        return '';
      }}
      return trimmed.slice(0, trimmed.lastIndexOf('/'));
    }}
    function objectRefId(bucket, key) {{
      return `${{bucket}}/${{key}}`;
    }}
    function actionDescription(payload) {{
      return payload.action === 'rename'
        ? `${{payload.bucket}}/${{payload.key}} -> ${{payload.new_key}}`
        : `${{payload.source_bucket}}/${{payload.source_key}} -> ${{payload.destination_bucket}}/${{payload.destination_key}}`;
    }}
    function objectActionTouchedObjects(payload) {{
      const refs = payload.action === 'rename'
        ? [
            {{ bucket: payload.bucket, key: payload.key, label: 'source before / old key after' }},
            {{ bucket: payload.bucket, key: payload.new_key, label: 'renamed target' }},
          ]
        : [
            {{ bucket: payload.source_bucket, key: payload.source_key, label: 'source' }},
            {{ bucket: payload.destination_bucket, key: payload.destination_key, label: 'destination' }},
          ];
      const seen = new Set();
      return refs.filter(ref => {{
        const id = objectRefId(ref.bucket, ref.key);
        if (seen.has(id)) {{
          return false;
        }}
        seen.add(id);
        return true;
      }});
    }}
    async function fetchObjectStatusSnapshot(bucket, key) {{
      const query = new URLSearchParams({{ bucket, key }});
      return fetchJson(`/api/object-status?${{query.toString()}}`);
    }}
    async function captureObjectActionSnapshots(refs) {{
      const entries = await Promise.all(refs.map(async ref => {{
        try {{
          const payload = await fetchObjectStatusSnapshot(ref.bucket, ref.key);
          return [objectRefId(ref.bucket, ref.key), payload];
        }} catch (error) {{
          return [objectRefId(ref.bucket, ref.key), {{ bucket: ref.bucket, key: ref.key, gateway_error: error.message, provider_states: [] }}];
        }}
      }}));
      return Object.fromEntries(entries);
    }}
    function summarizeObjectStatusPayload(payload) {{
      if (!payload) {{
        return 'No snapshot.';
      }}
      const states = payload.provider_states || [];
      const existing = states.filter(state => state.exists).length;
      const readable = states.filter(state => state.readable_via_gateway).length;
      if (payload.gateway_read_source) {{
        return `gateway=${{providerLabel(payload.gateway_read_source)}}${{payload.gateway_fallback_from ? ` fallback_from=${{providerLabel(payload.gateway_fallback_from)}}` : ''}} | exists_on=${{existing}} providers | gateway_readable=${{readable}} providers`;
      }}
      return `gateway_error=${{payload.gateway_error || 'unknown'}} | exists_on=${{existing}} providers | gateway_readable=${{readable}} providers`;
    }}
    function objectStateIndex(payload) {{
      const index = new Map();
      (payload?.provider_states || []).forEach(state => {{
        index.set(state.provider, state);
      }});
      return index;
    }}
    function objectStatusDelta(before, after) {{
      const changes = [];
      const beforeSource = before?.gateway_read_source || 'none';
      const afterSource = after?.gateway_read_source || 'none';
      if (beforeSource !== afterSource) {{
        changes.push(`gateway source: ${{providerLabel(beforeSource)}} -> ${{providerLabel(afterSource)}}`);
      }}
      const beforeError = before?.gateway_error || 'none';
      const afterError = after?.gateway_error || 'none';
      if (beforeError !== afterError) {{
        changes.push(`gateway error: ${{beforeError}} -> ${{afterError}}`);
      }}
      const beforeStates = before?.provider_states || [];
      const afterStates = after?.provider_states || [];
      const beforeExisting = beforeStates.filter(state => state.exists).length;
      const afterExisting = afterStates.filter(state => state.exists).length;
      if (beforeExisting !== afterExisting) {{
        changes.push(`exists on providers: ${{beforeExisting}} -> ${{afterExisting}}`);
      }}
      const beforeReadable = beforeStates.filter(state => state.readable_via_gateway).length;
      const afterReadable = afterStates.filter(state => state.readable_via_gateway).length;
      if (beforeReadable !== afterReadable) {{
        changes.push(`gateway-readable providers: ${{beforeReadable}} -> ${{afterReadable}}`);
      }}
      const beforeIndex = objectStateIndex(before);
      const afterIndex = objectStateIndex(after);
      const providers = dedupe([
        ...Array.from(beforeIndex.keys()),
        ...Array.from(afterIndex.keys()),
      ]);
      providers.forEach(provider => {{
        const prev = beforeIndex.get(provider);
        const next = afterIndex.get(provider);
        const prevExists = prev?.exists ? 'yes' : 'no';
        const nextExists = next?.exists ? 'yes' : 'no';
        if (prevExists !== nextExists) {{
          changes.push(`${{providerLabel(provider)}} exists: ${{prevExists}} -> ${{nextExists}}`);
        }}
        const prevReadable = prev?.readable_via_gateway ? 'yes' : 'no';
        const nextReadable = next?.readable_via_gateway ? 'yes' : 'no';
        if (prevReadable !== nextReadable) {{
          changes.push(`${{providerLabel(provider)}} gateway-readable: ${{prevReadable}} -> ${{nextReadable}}`);
        }}
      }});
      return changes;
    }}
    function latestReplicationJobSummary(job) {{
      if (!job) {{
        return 'none';
      }}
      return `status=${{job.status}} | op=${{job.operation}} | attempts=${{job.attempts}} | source=${{job.source_provider || 'n/a'}}`;
    }}
    function providerStateSnapshotSummary(state) {{
      if (!state) {{
        return 'missing';
      }}
      const parts = [
        `exists=${{state.exists ? 'yes' : 'no'}}`,
        `gateway_readable=${{state.readable_via_gateway ? 'yes' : 'no'}}`,
      ];
      if (state.fallback_gate) {{
        parts.push(`fallback=${{state.fallback_gate}}`);
      }}
      if (state.accepts_replication_put !== null && state.accepts_replication_put !== undefined) {{
        parts.push(`accepts_replication_put=${{state.accepts_replication_put ? 'yes' : 'no'}}`);
      }}
      if (state.access_error) {{
        parts.push(`access_error=${{state.access_error}}`);
      }}
      parts.push(`latest_job=${{latestReplicationJobSummary(state.latest_replication_job)}}`);
      return parts.join(' | ');
    }}
    function providerStateDelta(beforeState, afterState) {{
      const changes = [];
      const beforeExists = beforeState?.exists ? 'yes' : 'no';
      const afterExists = afterState?.exists ? 'yes' : 'no';
      if (beforeExists !== afterExists) {{
        changes.push(`exists: ${{beforeExists}} -> ${{afterExists}}`);
      }}
      const beforeReadable = beforeState?.readable_via_gateway ? 'yes' : 'no';
      const afterReadable = afterState?.readable_via_gateway ? 'yes' : 'no';
      if (beforeReadable !== afterReadable) {{
        changes.push(`gateway_readable: ${{beforeReadable}} -> ${{afterReadable}}`);
      }}
      const beforeFallback = beforeState?.fallback_gate || 'none';
      const afterFallback = afterState?.fallback_gate || 'none';
      if (beforeFallback !== afterFallback) {{
        changes.push(`fallback_gate: ${{beforeFallback}} -> ${{afterFallback}}`);
      }}
      const beforeAccess = beforeState?.access_error || 'none';
      const afterAccess = afterState?.access_error || 'none';
      if (beforeAccess !== afterAccess) {{
        changes.push(`access_error: ${{beforeAccess}} -> ${{afterAccess}}`);
      }}
      const beforeJob = latestReplicationJobSummary(beforeState?.latest_replication_job);
      const afterJob = latestReplicationJobSummary(afterState?.latest_replication_job);
      if (beforeJob !== afterJob) {{
        changes.push(`latest_job: ${{beforeJob}} -> ${{afterJob}}`);
      }}
      return changes;
    }}
    function renderProviderStateComparisons(before, after) {{
      const beforeIndex = objectStateIndex(before);
      const afterIndex = objectStateIndex(after);
      const providers = dedupe([
        ...Array.from(beforeIndex.keys()),
        ...Array.from(afterIndex.keys()),
      ]);
      if (!providers.length) {{
        return '<div class="object-meta">No provider-level state available.</div>';
      }}
      return `
        <div class="comparison-grid">
          ${{
            providers.map(provider => {{
              const beforeState = beforeIndex.get(provider);
              const afterState = afterIndex.get(provider);
              const deltas = providerStateDelta(beforeState, afterState);
              const deltaHtml = deltas.length
                ? `<ul class="delta-list">${{deltas.map(item => `<li>${{escapeHtml(item)}}</li>`).join('')}}</ul>`
                : '<div class="object-meta">No provider-level change detected.</div>';
              return `
                <div class="comparison-card">
                  <h4>${{escapeHtml(providerLabel(provider))}}</h4>
                  <div class="object-meta"><strong>Delta</strong>${{deltaHtml}}</div>
                  <div class="object-meta"><strong>Before</strong><br>${{escapeHtml(providerStateSnapshotSummary(beforeState))}}</div>
                  <div class="object-meta"><strong>After</strong><br>${{escapeHtml(providerStateSnapshotSummary(afterState))}}</div>
                </div>
              `;
            }}).join('')
          }}
        </div>
      `;
    }}
    function renderObjectActionInspection(details) {{
      const summaryNode = document.getElementById('object-action-inspection-summary');
      const resultsNode = document.getElementById('object-action-inspection-results');
      if (!details || !details.references || !details.references.length) {{
        summaryNode.textContent = 'No before/after object inspection captured yet.';
        resultsNode.innerHTML = '';
        return;
      }}
      summaryNode.textContent = `Captured before/after state for ${{details.references.length}} object reference(s).`;
      resultsNode.innerHTML = details.references.map(ref => {{
        const before = summarizeObjectStatusPayload(ref.before);
        const after = summarizeObjectStatusPayload(ref.after);
        const changes = objectStatusDelta(ref.before, ref.after);
        const providerComparisons = renderProviderStateComparisons(ref.before, ref.after);
        const deltaHtml = changes.length
          ? `<ul class="delta-list">${{changes.map(item => `<li>${{escapeHtml(item)}}</li>`).join('')}}</ul>`
          : '<div class="object-meta">No material state change detected yet.</div>';
        return `
          <div class="object-card">
            <h3>${{escapeHtml(ref.label)}}</h3>
            <div class="object-meta mono">${{escapeHtml(ref.bucket)}}/${{escapeHtml(ref.key)}}</div>
            <div class="object-meta"><strong>Changes</strong>${{deltaHtml}}</div>
            <div class="object-meta"><strong>Before</strong><br>${{escapeHtml(before)}}</div>
            <div class="object-meta"><strong>After</strong><br>${{escapeHtml(after)}}</div>
            <div class="object-meta"><strong>Provider Changes</strong></div>
            ${{providerComparisons}}
          </div>
        `;
      }}).join('');
    }}
    function clearObjectActionHistory() {{
      setObjectActionFeedback('Clearing shared object action history…', 'warn');
      fetchJson('/api/object-actions/history/clear', {{
        method: 'POST',
      }})
        .then(async () => {{
          await refreshStatus();
          setObjectActionFeedback('Shared object action history cleared.', 'ok');
        }})
        .catch(error => {{
          setObjectActionFeedback(error.message, 'warn');
        }});
    }}
    function objectActionHistoryFilters() {{
      return {{
        action: document.getElementById('object-action-history-action-filter').value,
        outcome: document.getElementById('object-action-history-outcome-filter').value,
        provider: document.getElementById('object-action-history-provider-filter').value,
        operator: document.getElementById('object-action-history-operator-filter').value.trim().toLowerCase(),
        object: document.getElementById('object-action-history-object-filter').value.trim().toLowerCase(),
        start_unix_ms: parseDateTimeLocalToUnixMs(document.getElementById('object-action-history-start-filter').value),
        end_unix_ms: parseDateTimeLocalToUnixMs(document.getElementById('object-action-history-end-filter').value),
      }};
    }}
    function updateObjectActionHistoryProviderFilter(history) {{
      const node = document.getElementById('object-action-history-provider-filter');
      const previous = node.value || 'all';
      const providers = dedupe((history || []).map(entry => entry.primary_provider).filter(Boolean));
      node.innerHTML = ['all', ...providers]
        .map(provider => `<option value="${{escapeHtml(provider)}}">${{escapeHtml(provider === 'all' ? 'all' : providerLabel(provider))}}</option>`)
        .join('');
      node.value = providers.includes(previous) || previous === 'all' ? previous : 'all';
    }}
    function filteredObjectActionHistory(history) {{
      const filters = objectActionHistoryFilters();
      return (history || []).filter(entry => {{
        if (filters.action !== 'all' && entry.action !== filters.action) {{
          return false;
        }}
        if (filters.outcome !== 'all' && entry.outcome !== filters.outcome) {{
          return false;
        }}
        if (filters.provider !== 'all' && entry.primary_provider !== filters.provider) {{
          return false;
        }}
        if (filters.operator) {{
          const operator = String(entry.operator || '').toLowerCase();
          if (!operator.includes(filters.operator)) {{
            return false;
          }}
        }}
        if (filters.object) {{
          const description = String(entry.description || '').toLowerCase();
          const matchesReference = (entry.references || []).some(ref => `${{ref.bucket || ''}}/${{ref.key || ''}}`.toLowerCase().includes(filters.object));
          if (!description.includes(filters.object) && !matchesReference) {{
            return false;
          }}
        }}
        if (filters.start_unix_ms !== null && Number(entry.executed_at_unix_ms || 0) < filters.start_unix_ms) {{
          return false;
        }}
        if (filters.end_unix_ms !== null && Number(entry.executed_at_unix_ms || 0) > filters.end_unix_ms) {{
          return false;
        }}
        return true;
      }});
    }}
    function csvEscape(value) {{
      const raw = value === null || value === undefined ? '' : String(value);
      return `"${{raw.replaceAll('"', '""')}}"`;
    }}
    function updateReplicationFailedTargetFilter(jobs) {{
      const node = document.getElementById('replication-failed-target-filter');
      const previous = node.value || 'all';
      const targets = dedupe((jobs || []).map(job => job.target).filter(Boolean));
      node.innerHTML = ['all', ...targets]
        .map(target => `<option value="${{escapeHtml(target)}}">${{escapeHtml(target === 'all' ? 'all' : providerLabel(target))}}</option>`)
        .join('');
      node.value = targets.includes(previous) || previous === 'all' ? previous : 'all';
    }}
    function replicationFailedJobFilters() {{
      return {{
        target: document.getElementById('replication-failed-target-filter').value || 'all',
        object: document.getElementById('replication-failed-object-filter').value.trim().toLowerCase(),
        start_unix_ms: parseDateTimeLocalToUnixMs(document.getElementById('replication-failed-start-filter').value),
        end_unix_ms: parseDateTimeLocalToUnixMs(document.getElementById('replication-failed-end-filter').value),
      }};
    }}
    function filteredReplicationFailedJobs(jobs) {{
      const filters = replicationFailedJobFilters();
      return (jobs || []).filter(job => {{
        if (filters.target !== 'all' && job.target !== filters.target) {{
          return false;
        }}
        if (filters.object) {{
          const objectRef = `${{job.object?.bucket || ''}}/${{job.object?.key || ''}}`.toLowerCase();
          const error = String(job.last_error || '').toLowerCase();
          if (!objectRef.includes(filters.object) && !error.includes(filters.object)) {{
            return false;
          }}
        }}
        const occurredAt = Number(job.enqueued_at_unix_ms || 0);
        if (filters.start_unix_ms !== null && occurredAt < filters.start_unix_ms) {{
          return false;
        }}
        if (filters.end_unix_ms !== null && occurredAt > filters.end_unix_ms) {{
          return false;
        }}
        return true;
      }});
    }}
    function downloadReplicationFailedJobs() {{
      const filtered = filteredReplicationFailedJobs(latestFailedJobs);
      const payload = {{
        exported_at: new Date().toISOString(),
        filters: replicationFailedJobFilters(),
        jobs: filtered,
      }};
      const blob = new Blob([JSON.stringify(payload, null, 2)], {{ type: 'application/json' }});
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = `replication-latest-failed-${{Date.now()}}.json`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
      setReplicationFeedback(`Exported ${{filtered.length}} latest failed object entr${{filtered.length === 1 ? 'y' : 'ies'}}.`, 'ok');
    }}
    function downloadReplicationFailedJobsCsv() {{
      const filtered = filteredReplicationFailedJobs(latestFailedJobs);
      const rows = [
        ['job_id', 'target', 'source_provider', 'operation', 'bucket', 'key', 'attempts', 'queued_at', 'next_retry_at', 'last_error'],
        ...filtered.map(job => [
          job.job_id,
          job.target || '',
          job.source_provider || '',
          job.operation || '',
          job.object?.bucket || '',
          job.object?.key || '',
          job.attempts || 0,
          formatTimestamp(job.enqueued_at_unix_ms),
          formatTimestamp(job.next_attempt_at_unix_ms),
          job.last_error || '',
        ]),
      ];
      const csv = rows.map(row => row.map(csvEscape).join(',')).join('\n');
      const blob = new Blob([csv], {{ type: 'text/csv;charset=utf-8' }});
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = `replication-latest-failed-${{Date.now()}}.csv`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
      setReplicationFeedback(`Exported ${{filtered.length}} latest failed object entr${{filtered.length === 1 ? 'y' : 'ies'}} as CSV.`, 'ok');
    }}
    function downloadObjectActionHistory() {{
      const filtered = filteredObjectActionHistory(objectActionHistory);
      const payload = {{
        exported_at: new Date().toISOString(),
        history_limit: objectActionHistoryLimit,
        filters: objectActionHistoryFilters(),
        entries: filtered,
      }};
      const blob = new Blob([JSON.stringify(payload, null, 2)], {{ type: 'application/json' }});
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = `object-action-history-${{Date.now()}}.json`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
      setObjectActionFeedback(`Exported ${{filtered.length}} shared history entr${{filtered.length === 1 ? 'y' : 'ies'}}.`, 'ok');
    }}
    function downloadObjectActionHistoryCsv() {{
      const filtered = filteredObjectActionHistory(objectActionHistory);
      const rows = [
        ['executed_at', 'primary_provider', 'action', 'outcome', 'operator', 'ticket', 'description', 'message', 'notes', 'warnings', 'references'],
        ...filtered.map(entry => [
          formatTimestamp(entry.executed_at_unix_ms),
          entry.primary_provider || '',
          entry.action || '',
          entry.outcome || '',
          entry.operator || '',
          entry.ticket || '',
          entry.description || '',
          entry.message || '',
          entry.notes || '',
          (entry.warnings || []).join(' | '),
          (entry.references || []).map(ref => `${{ref.label}}:${{ref.bucket}}/${{ref.key}}`).join(' | '),
        ]),
      ];
      const csv = rows.map(row => row.map(csvEscape).join(',')).join('\\n');
      const blob = new Blob([csv], {{ type: 'text/csv;charset=utf-8' }});
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = `object-action-history-${{Date.now()}}.csv`;
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
      setObjectActionFeedback(`Exported ${{filtered.length}} shared history entr${{filtered.length === 1 ? 'y' : 'ies'}} as CSV.`, 'ok');
    }}
    function renderObjectActionHistory(history) {{
      objectActionHistory.splice(0, objectActionHistory.length, ...(history || []));
      updateObjectActionHistoryProviderFilter(objectActionHistory);
      const summaryNode = document.getElementById('object-action-history-summary');
      const container = document.getElementById('object-action-history');
      if (!objectActionHistory.length) {{
        summaryNode.textContent = 'No shared object action history yet.';
        container.innerHTML = '';
        return;
      }}
      const filteredHistory = filteredObjectActionHistory(objectActionHistory);
      const filters = objectActionHistoryFilters();
      const timeWindowSummary = [
        filters.start_unix_ms !== null ? `start=${{formatTimestamp(filters.start_unix_ms)}}` : null,
        filters.end_unix_ms !== null ? `end=${{formatTimestamp(filters.end_unix_ms)}}` : null,
      ].filter(Boolean).join(' | ');
      let summaryText = `Showing ${{filteredHistory.length}} of ${{objectActionHistory.length}} recent object action(s) stored on this gateway. Limit=${{objectActionHistoryLimit}}.`;
      if (timeWindowSummary) {{
        summaryText += ` Time window: ${{timeWindowSummary}}.`;
      }}
      summaryNode.textContent = summaryText;
      if (!filteredHistory.length) {{
        container.innerHTML = '<div class="object-meta">No shared history entries match the current filters.</div>';
        return;
      }}
      container.innerHTML = filteredHistory.map(entry => {{
        const warningHtml = entry.warnings && entry.warnings.length
          ? `<ul class="delta-list">${{entry.warnings.map(item => `<li>${{escapeHtml(item)}}</li>`).join('')}}</ul>`
          : '<div class="object-meta">No warnings were recorded for this action.</div>';
        const refs = (entry.references || []).map(ref => {{
          const changeHtml = ref.changes && ref.changes.length
            ? `<ul class="delta-list">${{ref.changes.map(item => `<li>${{escapeHtml(item)}}</li>`).join('')}}</ul>`
            : '<div class="object-meta">No material change recorded.</div>';
          return `
            <div class="comparison-card">
              <h4>${{escapeHtml(ref.label)}}</h4>
              <div class="object-meta mono">${{escapeHtml(ref.bucket)}}/${{escapeHtml(ref.key)}}</div>
              <div class="object-meta">${{changeHtml}}</div>
            </div>
          `;
        }}).join('');
        return `
          <div class="history-card">
            <h4>${{escapeHtml(entry.action)}} · <span class="${{entry.outcome === 'success' ? 'status-ok' : 'status-warn'}}">${{escapeHtml(entry.outcome)}}</span></h4>
            <div class="object-meta">${{escapeHtml(formatTimestamp(entry.executed_at_unix_ms))}} | primary=${{escapeHtml(providerLabel(entry.primary_provider))}}</div>
            <div class="object-meta">${{escapeHtml(`operator=${{entry.operator || 'n/a'}} | ticket=${{entry.ticket || 'n/a'}}`)}}</div>
            <div class="object-meta mono">${{escapeHtml(entry.description)}}</div>
            <div class="object-meta">${{escapeHtml(entry.message || '')}}</div>
            <div class="object-meta"><strong>Notes</strong><br>${{escapeHtml(entry.notes || 'No notes recorded.')}}</div>
            <div class="object-meta"><strong>Warnings</strong>${{warningHtml}}</div>
            <div class="comparison-grid">${{refs || '<div class="object-meta">No captured object references.</div>'}}</div>
          </div>
        `;
      }}).join('');
    }}
    function buildObjectActionPreview(payload) {{
      const primary = runtimeTopologyState.primary_provider || 'unknown';
      const lines = [`Primary provider: ${{providerLabel(primary)}}`];
      const warnings = [];
      if (payload.action === 'rename') {{
        lines.push(`Planned rename: ${{payload.bucket}}/${{payload.key}} -> ${{payload.bucket}}/${{payload.new_key}}`);
        lines.push('Replication plan: put(new) + delete(old)');
        if (payload.key === payload.new_key) {{
          warnings.push('This is a no-op rename. The gateway will return success without changing the object.');
        }}
        if (primary === 'unicom' && objectParentPath(payload.key) !== objectParentPath(payload.new_key)) {{
          warnings.push('Current Unicom rename only supports staying in the same parent directory. Use move for cross-directory changes.');
        }}
      }} else if (payload.action === 'copy') {{
        lines.push(`Planned copy: ${{payload.source_bucket}}/${{payload.source_key}} -> ${{payload.destination_bucket}}/${{payload.destination_key}}`);
        lines.push('Replication plan: put(destination)');
        if (payload.source_bucket === payload.destination_bucket && payload.source_key === payload.destination_key) {{
          warnings.push('This copies an object onto the same bucket/key. The destination may be overwritten or treated as a provider-specific no-op.');
        }}
      }} else if (payload.action === 'move') {{
        lines.push(`Planned move: ${{payload.source_bucket}}/${{payload.source_key}} -> ${{payload.destination_bucket}}/${{payload.destination_key}}`);
        lines.push('Replication plan: put(destination) + delete(source)');
        if (payload.source_bucket === payload.destination_bucket && payload.source_key === payload.destination_key) {{
          warnings.push('This is a no-op move. The gateway will return success without changing the object.');
        }}
      }}
      if (payload.action === 'copy' || payload.action === 'move') {{
        if (payload.source_bucket !== payload.destination_bucket) {{
          warnings.push('This action crosses buckets/containers. Confirm the destination scope is intentional.');
        }}
        warnings.push('Destination writes may overwrite an existing object at the destination key.');
      }}
      return {{
        summary: lines.join('\\n'),
        warnings,
      }};
    }}
    function renderObjectActionPreview() {{
      const summaryNode = document.getElementById('object-action-preview-summary');
      try {{
        const payload = collectObjectActionInput();
        const preview = buildObjectActionPreview(payload);
        const warningLines = preview.warnings.length
          ? `\\nWarnings:\\n- ${{preview.warnings.join('\\n- ')}}`
          : '\\nWarnings:\\n- none';
        summaryNode.textContent = `${{preview.summary}}${{warningLines}}`;
      }} catch (error) {{
        summaryNode.textContent = error.message || 'Select an action and fill the fields to see risks before execution.';
      }}
    }}
    function collectObjectActionInput() {{
      const operator = document.getElementById('object-action-operator').value.trim();
      const ticket = document.getElementById('object-action-ticket').value.trim();
      const notes = document.getElementById('object-action-notes').value.trim();
      const action = document.getElementById('object-action-kind').value;
      if (action === 'rename') {{
        const bucket = document.getElementById('object-action-rename-bucket').value.trim();
        const key = document.getElementById('object-action-rename-key').value.trim();
        const new_key = document.getElementById('object-action-rename-new-key').value.trim();
        if (!bucket || !key || !new_key) {{
          throw new Error('Rename requires bucket, key, and new key.');
        }}
        return {{ action, bucket, key, new_key, operator, ticket, notes }};
      }}
      const source_bucket = document.getElementById('object-action-source-bucket').value.trim();
      const source_key = document.getElementById('object-action-source-key').value.trim();
      const destination_bucket = document.getElementById('object-action-destination-bucket').value.trim();
      const destination_key = document.getElementById('object-action-destination-key').value.trim();
      if (!source_bucket || !source_key || !destination_bucket || !destination_key) {{
        throw new Error(`${{action}} requires source and destination bucket/key values.`);
      }}
      return {{ action, source_bucket, source_key, destination_bucket, destination_key, operator, ticket, notes }};
    }}
    async function runObjectAction() {{
      let payload;
      try {{
        payload = collectObjectActionInput();
      }} catch (error) {{
        setObjectActionFeedback(error.message, 'warn');
        return;
      }}
      const description = actionDescription(payload);
      const refs = objectActionTouchedObjects(payload);
      setObjectActionFeedback(`Running ${{payload.action}} for ${{description}}…`, 'warn');
      document.getElementById('object-action-json').textContent = JSON.stringify(payload, null, 2);
      const beforeSnapshots = await captureObjectActionSnapshots(refs);
      try {{
        await fetchJson('/api/object-actions', {{
          method: 'POST',
          headers: {{ 'content-type': 'application/json' }},
          body: JSON.stringify(payload),
        }});
        const afterSnapshots = await captureObjectActionSnapshots(refs);
        const inspection = {{
          references: refs.map(ref => {{
            const id = objectRefId(ref.bucket, ref.key);
            return {{
              ...ref,
              before: beforeSnapshots[id] || null,
              after: afterSnapshots[id] || null,
            }};
          }}),
        }};
        renderObjectActionInspection(inspection);
        document.getElementById('object-action-summary').textContent =
          `Completed ${{payload.action}}: ${{description}}`;
        document.getElementById('object-action-json').textContent = JSON.stringify({{
          request: payload,
          result: 'no_content',
          inspection,
        }}, null, 2);
        setObjectActionFeedback(`Object action completed: ${{description}}`, 'ok');
        await refreshStatus();
      }} catch (error) {{
        const afterSnapshots = await captureObjectActionSnapshots(refs);
        const inspection = {{
          references: refs.map(ref => {{
            const id = objectRefId(ref.bucket, ref.key);
            return {{
              ...ref,
              before: beforeSnapshots[id] || null,
              after: afterSnapshots[id] || null,
            }};
          }}),
        }};
        renderObjectActionInspection(inspection);
        document.getElementById('object-action-summary').textContent = error.message;
        document.getElementById('object-action-json').textContent = JSON.stringify({{
          request: payload,
          error: error.message,
          inspection,
        }}, null, 2);
        setObjectActionFeedback(error.message, 'warn');
        await refreshStatus();
      }}
    }}
    function credentialFieldId(provider, key) {{
      return `provider-credential-${{provider}}-${{key}}`;
    }}
    function renderProviderCredentials() {{
      const container = document.getElementById('provider-credentials-grid');
      container.innerHTML = providerCredentialsCatalog.map(entry => {{
        const payload = providerCredentialState[entry.provider] || {{ provider: entry.provider, label: entry.label }};
        const output = payload.error
          ? payload.error
          : JSON.stringify(payload, null, 2);
        const fields = entry.fields.map(field => {{
          const value = payload[field.key] || '';
          const control = field.multiline
            ? `<textarea id="${{credentialFieldId(entry.provider, field.key)}}" placeholder="${{escapeHtml(field.placeholder || '')}}">${{escapeHtml(value)}}</textarea>`
            : `<input id="${{credentialFieldId(entry.provider, field.key)}}" type="text" value="${{escapeHtml(value)}}" placeholder="${{escapeHtml(field.placeholder || '')}}" />`;
          return `
            <label>${{escapeHtml(field.label)}}
              ${{control}}
            </label>
          `;
        }}).join('');
        const storagePath = payload.storage_path || 'Loading…';
        const extraNotes = entry.provider === 'onedrive'
          ? `OAuth session file: <span class="mono">${{escapeHtml(payload.session_file || 'n/a')}}</span>`
          : 'Carrier auth fields are isolated from the other providers.';
        return `
          <div class="provider-card">
            <h3>${{escapeHtml(entry.label)}}</h3>
            <div class="provider-note">Storage file: <span class="mono">${{escapeHtml(storagePath)}}</span></div>
            <div class="provider-note">${{extraNotes}}</div>
            ${{fields}}
            <div class="actions">
              <button class="secondary" type="button" data-provider-credential-save="${{entry.provider}}">Save Credentials</button>
              <button class="secondary" type="button" data-provider-credential-reload="${{entry.provider}}">Reload</button>
            </div>
            <pre id="provider-credential-output-${{entry.provider}}">${{escapeHtml(output)}}</pre>
          </div>
        `;
      }}).join('');
    }}
    async function refreshProviderCredentials() {{
      const entries = await Promise.all(providerCredentialsCatalog.map(async entry => {{
        try {{
          const payload = await fetchJson(`/api/providers/${{encodeURIComponent(entry.provider)}}/credentials`);
          return [entry.provider, payload];
        }} catch (error) {{
          return [entry.provider, {{
            provider: entry.provider,
            label: entry.label,
            storage_path: '',
            error: error.message,
          }}];
        }}
      }}));
      providerCredentialState = Object.fromEntries(entries);
      renderProviderCredentials();
    }}
    async function refreshAuthCapturePrompts() {{
      const prompts = await fetchJson('/api/auth-capture/prompts');
      renderAuthCapturePrompts(prompts || []);
    }}
    function collectProviderCredentialInput(provider) {{
      const entry = providerCredentialsCatalog.find(item => item.provider === provider);
      if (!entry) {{
        throw new Error(`unknown provider: ${{provider}}`);
      }}
      const payload = {{}};
      entry.fields.forEach(field => {{
        const node = document.getElementById(credentialFieldId(provider, field.key));
        payload[field.key] = node ? node.value : '';
      }});
      return payload;
    }}
    async function saveProviderCredential(provider) {{
      try {{
        setProviderCredentialsFeedback(`Saving ${{providerLabel(provider)}} credentials…`, 'warn');
        const result = await fetchJson(`/api/providers/${{encodeURIComponent(provider)}}/credentials`, {{
          method: 'POST',
          headers: {{ 'content-type': 'application/json' }},
          body: JSON.stringify(collectProviderCredentialInput(provider)),
        }});
        providerCredentialState[provider] = result;
        renderProviderCredentials();
        await refreshStatus();
        setProviderCredentialsFeedback(`${{providerLabel(provider)}} credentials saved and injected live.`, 'ok');
      }} catch (error) {{
        setProviderCredentialsFeedback(error.message, 'warn');
        const output = document.getElementById(`provider-credential-output-${{provider}}`);
        if (output) {{
          output.textContent = error.message;
        }}
      }}
    }}
    async function submitAuthCapturePrompt(promptId) {{
      const prompt = authPromptState.get(promptId);
      const input = document.getElementById(`auth-prompt-${{promptId}}`);
      if (!prompt || !input) {{
        setAuthPromptFeedback('Prompt is no longer available.', 'warn');
        return;
      }}
      const value = input.value.trim();
      if (!value) {{
        setAuthPromptFeedback(`Input required for ${{prompt.title || prompt.provider}}.`, 'warn');
        return;
      }}
      try {{
        setAuthPromptFeedback(`Submitting input for ${{prompt.title || prompt.provider}}…`, 'warn');
        await fetchJson(`/api/auth-capture/prompts/${{encodeURIComponent(promptId)}}/reply`, {{
          method: 'POST',
          headers: {{ 'content-type': 'application/json' }},
          body: JSON.stringify({{ value }}),
        }});
        await refreshAuthCapturePrompts();
        setAuthPromptFeedback(`Input submitted for ${{prompt.title || prompt.provider}}.`, 'ok');
      }} catch (error) {{
        setAuthPromptFeedback(error.message, 'warn');
      }}
    }}
    function normalizeTopologyDraft() {{
      const catalog = new Map(topologyProviderCatalog.map(item => [item.provider, item]));
      const firstPrimary = topologyProviderCatalog.find(item => item.enabled && item.can_be_primary);
      const currentPrimary = catalog.get(topologyDraft.primary_provider);
      if (!currentPrimary || !currentPrimary.enabled || !currentPrimary.can_be_primary) {{
        topologyDraft.primary_provider = firstPrimary ? firstPrimary.provider : 'unicom';
      }}
      topologyDraft.sync_targets = dedupe(topologyDraft.sync_targets).filter(provider => {{
        const entry = catalog.get(provider);
        return entry && entry.enabled && entry.can_be_sync_target && provider !== topologyDraft.primary_provider;
      }});
      topologyDraft.fallback_read_order = dedupe(topologyDraft.fallback_read_order).filter(provider =>
        topologyDraft.sync_targets.includes(provider)
      );
    }}
    function renderFallbackOrder() {{
      const container = document.getElementById('topology-fallback-order');
      if (!topologyDraft.fallback_read_order.length) {{
        container.innerHTML = `
          <div class="fallback-row">
            <div>No fallback providers enabled.</div>
            <div class="status-warn">Primary-only reads</div>
          </div>
        `;
        return;
      }}
      container.innerHTML = topologyDraft.fallback_read_order.map((provider, index) => `
        <div class="fallback-row">
          <div>
            <span class="pill">#${{index + 1}}</span>
            <strong>${{providerLabel(provider)}}</strong>
          </div>
          <div class="fallback-actions">
            <button class="secondary" type="button" data-move="up" data-provider="${{provider}}" ${{index === 0 ? 'disabled' : ''}}>Up</button>
            <button class="secondary" type="button" data-move="down" data-provider="${{provider}}" ${{index === topologyDraft.fallback_read_order.length - 1 ? 'disabled' : ''}}>Down</button>
          </div>
        </div>
      `).join('');
    }}
    function renderTopologyEditor() {{
      normalizeTopologyDraft();
      const container = document.getElementById('topology-provider-grid');
      container.innerHTML = topologyProviderCatalog.map(entry => {{
        const isPrimary = topologyDraft.primary_provider === entry.provider;
        const isSyncTarget = topologyDraft.sync_targets.includes(entry.provider);
        const isFallback = topologyDraft.fallback_read_order.includes(entry.provider);
        const syncDisabled = !entry.enabled || !entry.can_be_sync_target || isPrimary;
        const primaryDisabled = !entry.enabled || !entry.can_be_primary;
        const fallbackDisabled = syncDisabled || !isSyncTarget;
        const notes = [];
        if (!entry.enabled) {{
          notes.push('Disabled by current gateway configuration.');
        }}
        if (entry.provider === 'onedrive') {{
          notes.push('OneDrive fallback also depends on the OneDrive policy section below.');
        }}
        if (entry.provider === 'stub') {{
          notes.push('Stub is intended for local development and hot-switch verification.');
        }}
        return `
          <div class="provider-card ${{entry.enabled ? '' : 'disabled'}}">
            <h3>${{entry.label}}</h3>
            <label class="provider-role">
              <input type="radio" name="topology-primary" data-provider="${{entry.provider}}" ${{isPrimary ? 'checked' : ''}} ${{primaryDisabled ? 'disabled' : ''}} />
              Primary write
            </label>
            <label class="provider-role">
              <input type="checkbox" data-role="sync" data-provider="${{entry.provider}}" ${{isSyncTarget ? 'checked' : ''}} ${{syncDisabled ? 'disabled' : ''}} />
              Async sync target
            </label>
            <label class="provider-role">
              <input type="checkbox" data-role="fallback" data-provider="${{entry.provider}}" ${{isFallback ? 'checked' : ''}} ${{fallbackDisabled ? 'disabled' : ''}} />
              Allow fallback reads
            </label>
            <div class="provider-note">${{notes.join(' ') || 'No extra notes.'}}</div>
          </div>
        `;
      }}).join('');
      renderFallbackOrder();
    }}
    function loadDesiredTopology(payload) {{
      topologyDraft = {{
        primary_provider: payload.primary_provider,
        sync_targets: dedupe(payload.sync_targets || []),
        fallback_read_order: dedupe(payload.fallback_read_order || []),
      }};
      renderTopologyEditor();
      document.getElementById('desired-topology').textContent = JSON.stringify(payload, null, 2);
      renderDesiredTopologySummary(payload);
    }}
    function loadOnedrivePolicy(payload) {{
      document.getElementById('onedrive-replication-enabled').checked = !!payload.replication_enabled;
      document.getElementById('onedrive-fallback-enabled').checked = !!payload.fallback_enabled;
      document.getElementById('onedrive-scope-mode').value = payload.scope_mode || 'all';
      document.getElementById('onedrive-memory-buckets').value = (payload.memory_buckets || []).join(', ');
      document.getElementById('onedrive-memory-prefixes').value = (payload.memory_prefixes || []).join(', ');
      document.getElementById('onedrive-policy').textContent = JSON.stringify(payload, null, 2);
      renderOnedrivePolicySummary(payload);
    }}
    async function refreshStatus(options = {{}}) {{
      if (statusRefreshInFlight) {{
        return;
      }}
      statusRefreshInFlight = true;
      try {{
        const status = await fetchJson('/api/status');
        runtimeTopologyState = status.runtime_topology || runtimeTopologyState;
        objectActionHistoryLimit = Number(status.object_action_history_limit || objectActionHistoryLimit || {DEFAULT_OBJECT_ACTION_HISTORY_LIMIT});
        document.getElementById('gateway-status').textContent = JSON.stringify(status, null, 2);
        renderRuntimeSummary(status.runtime || {{}});
        renderMonitoringSummary(status.monitoring || {{}});
        renderOperationsOverview(status.operations_overview || {{}});
        renderNotifySummary(status.notify || {{}});
        document.getElementById('auth-status').textContent = JSON.stringify(status.onedrive_auth, null, 2);
        document.getElementById('runtime-topology').textContent = JSON.stringify(status.runtime_topology, null, 2);
        renderAuthSummary(status.onedrive_auth || {{}});
        renderRuntimeTopologySummary(status.runtime_topology || {{}});
        renderAlerts(status.alerts || []);
        renderProviderHealth(status.provider_health || []);
        renderReplication(status.replication_state);
        renderObjectActionHistory(status.object_action_history || []);
        if (!options.lightweight) {{
          loadDesiredTopology(status.desired_topology);
          loadOnedrivePolicy(status.onedrive_policy);
          renderAuthCapturePolicy(status.auth_capture_policy || {{}});
          await refreshProviderCredentials();
          await refreshAuthCapturePrompts();
        }}
        renderObjectActionPreview();
        lastStatusRefreshUnixMs = Date.now();
        renderStatusRefreshSummary();
      }} catch (error) {{
        document.getElementById('gateway-status').textContent = error.message;
        renderStatusRefreshSummary(error.message);
      }} finally {{
        statusRefreshInFlight = false;
      }}
    }}
    async function startDeviceFlow() {{
      try {{
        const payload = await fetchJson('/api/auth/onedrive/device/start', {{ method: 'POST' }});
        document.getElementById('device-flow-summary').textContent =
          `Code: ${{payload.user_code || 'n/a'}} | Verify: ${{payload.verification_uri_complete || payload.verification_uri || 'n/a'}} | Status: ${{payload.status || 'pending'}}`;
        document.getElementById('device-output').textContent = JSON.stringify(payload, null, 2);
        if (!payload.flow_id) {{
          return;
        }}
        const interval = Math.max(payload.interval || 5, 2) * 1000;
        const timer = setInterval(async () => {{
          const latest = await fetchJson(`/api/auth/onedrive/device/${{payload.flow_id}}`);
          document.getElementById('device-flow-summary').textContent =
            `Code: ${{latest.user_code || payload.user_code || 'n/a'}} | Verify: ${{latest.verification_uri_complete || latest.verification_uri || payload.verification_uri_complete || payload.verification_uri || 'n/a'}} | Status: ${{latest.status || 'pending'}}`;
          document.getElementById('device-output').textContent = JSON.stringify(latest, null, 2);
          if (latest.status === 'completed' || latest.status === 'failed') {{
            clearInterval(timer);
            refreshStatus();
          }}
        }}, interval);
      }} catch (error) {{
        document.getElementById('device-output').textContent = error.message;
      }}
    }}
    async function saveTopology() {{
      normalizeTopologyDraft();
      const payload = {{
        primary_provider: topologyDraft.primary_provider,
        sync_targets: topologyDraft.sync_targets,
        fallback_read_order: topologyDraft.fallback_read_order,
      }};
      try {{
        setTopologyFeedback('Applying topology live…', 'warn');
        const result = await fetchJson('/api/control-plane/topology', {{
          method: 'POST',
          headers: {{ 'content-type': 'application/json' }},
          body: JSON.stringify(payload),
        }});
        loadDesiredTopology(result);
        await refreshStatus();
        setTopologyFeedback('Topology applied live. New requests now use the selected primary provider.', 'ok');
      }} catch (error) {{
        setTopologyFeedback(error.message, 'warn');
      }}
    }}
    async function saveOnedrivePolicy() {{
      const payload = {{
        replication_enabled: document.getElementById('onedrive-replication-enabled').checked,
        fallback_enabled: document.getElementById('onedrive-fallback-enabled').checked,
        scope_mode: document.getElementById('onedrive-scope-mode').value,
        memory_buckets: csvToArray(document.getElementById('onedrive-memory-buckets').value),
        memory_prefixes: csvToArray(document.getElementById('onedrive-memory-prefixes').value),
      }};
      const result = await fetchJson('/api/policy/onedrive', {{
        method: 'POST',
        headers: {{ 'content-type': 'application/json' }},
        body: JSON.stringify(payload),
      }});
      loadOnedrivePolicy(result);
      refreshStatus();
    }}
    async function saveAuthCapturePolicy() {{
      const rawCdpTimeout = document.getElementById('auth-capture-cdp-target-timeout-ms').value.trim();
      const payload = {{
        enabled: document.getElementById('auth-capture-enabled').checked,
        broker_url: document.getElementById('auth-capture-broker-url').value.trim(),
        cdp_endpoint_url: document.getElementById('auth-capture-cdp-endpoint-url').value.trim(),
        cdp_target_selector: document.getElementById('auth-capture-cdp-target-selector').value.trim(),
        cdp_target_timeout_ms: rawCdpTimeout ? Number(rawCdpTimeout) : null,
        llm_analysis_enabled: document.getElementById('auth-capture-llm-enabled').checked,
        llm_endpoint: document.getElementById('auth-capture-llm-endpoint').value.trim(),
        llm_model_id: document.getElementById('auth-capture-llm-model-id').value.trim(),
        llm_api_key: document.getElementById('auth-capture-llm-api-key').value.trim(),
        clear_llm_api_key: document.getElementById('auth-capture-clear-llm-api-key').checked,
      }};
      const result = await fetchJson('/api/policy/auth-capture', {{
        method: 'POST',
        headers: {{ 'content-type': 'application/json' }},
        body: JSON.stringify(payload),
      }});
      renderAuthCapturePolicy(result);
      refreshStatus();
    }}
    async function runProviderTest(provider) {{
      const output = document.getElementById('provider-test-output');
      output.textContent = `Testing ${{providerLabel(provider)}}…`;
      try {{
        const result = await fetchJson(`/api/providers/${{encodeURIComponent(provider)}}/test`, {{
          method: 'POST',
        }});
        output.textContent = JSON.stringify(result, null, 2);
        await refreshStatus();
      }} catch (error) {{
        output.textContent = error.message;
      }}
    }}
    async function inspectObjectStatus() {{
      const bucket = document.getElementById('object-status-bucket').value.trim();
      const key = document.getElementById('object-status-key').value.trim();
      if (!bucket || !key) {{
        setObjectStatusFeedback('Bucket and key are both required.', 'warn');
        return;
      }}
      setObjectStatusFeedback('Inspecting object state…', 'warn');
      try {{
        const query = new URLSearchParams({{ bucket, key }});
        const result = await fetchJson(`/api/object-status?${{query.toString()}}`);
        renderObjectStatus(result);
        setObjectStatusFeedback('Object status loaded.', 'ok');
      }} catch (error) {{
        document.getElementById('object-status-json').textContent = error.message;
        document.getElementById('object-status-results').innerHTML = '';
        document.getElementById('object-status-summary').textContent = error.message;
        setObjectStatusFeedback(error.message, 'warn');
      }}
    }}
    document.getElementById('topology-provider-grid').addEventListener('change', event => {{
      const target = event.target;
      const provider = target.dataset.provider;
      if (!provider) {{
        return;
      }}
      if (target.name === 'topology-primary') {{
        topologyDraft.primary_provider = provider;
        topologyDraft.sync_targets = topologyDraft.sync_targets.filter(item => item !== provider);
        topologyDraft.fallback_read_order = topologyDraft.fallback_read_order.filter(item => item !== provider);
        renderTopologyEditor();
        setTopologyFeedback(`Primary provider changed to ${{providerLabel(provider)}}. Save to apply live.`, 'warn');
        return;
      }}
      if (target.dataset.role === 'sync') {{
        if (target.checked) {{
          topologyDraft.sync_targets = dedupe([...topologyDraft.sync_targets, provider]);
          if (!topologyDraft.fallback_read_order.includes(provider)) {{
            topologyDraft.fallback_read_order = [...topologyDraft.fallback_read_order, provider];
          }}
        }} else {{
          topologyDraft.sync_targets = topologyDraft.sync_targets.filter(item => item !== provider);
          topologyDraft.fallback_read_order = topologyDraft.fallback_read_order.filter(item => item !== provider);
        }}
        renderTopologyEditor();
        setTopologyFeedback(`Sync targets updated for ${{providerLabel(provider)}}. Save to apply live.`, 'warn');
        return;
      }}
      if (target.dataset.role === 'fallback') {{
        if (target.checked) {{
          if (topologyDraft.sync_targets.includes(provider) && !topologyDraft.fallback_read_order.includes(provider)) {{
            topologyDraft.fallback_read_order = [...topologyDraft.fallback_read_order, provider];
          }}
        }} else {{
          topologyDraft.fallback_read_order = topologyDraft.fallback_read_order.filter(item => item !== provider);
        }}
        renderTopologyEditor();
        setTopologyFeedback(`Fallback policy updated for ${{providerLabel(provider)}}. Save to apply live.`, 'warn');
      }}
    }});
    document.getElementById('topology-fallback-order').addEventListener('click', event => {{
      const button = event.target.closest('button[data-move]');
      if (!button) {{
        return;
      }}
      const provider = button.dataset.provider;
      const index = topologyDraft.fallback_read_order.indexOf(provider);
      if (index === -1) {{
        return;
      }}
      if (button.dataset.move === 'up' && index > 0) {{
        const next = [...topologyDraft.fallback_read_order];
        [next[index - 1], next[index]] = [next[index], next[index - 1]];
        topologyDraft.fallback_read_order = next;
      }}
      if (button.dataset.move === 'down' && index < topologyDraft.fallback_read_order.length - 1) {{
        const next = [...topologyDraft.fallback_read_order];
        [next[index], next[index + 1]] = [next[index + 1], next[index]];
        topologyDraft.fallback_read_order = next;
      }}
      renderTopologyEditor();
      setTopologyFeedback('Fallback order updated. Save to apply live.', 'warn');
    }});
    document.getElementById('provider-health-grid').addEventListener('click', event => {{
      const button = event.target.closest('button[data-provider-test]');
      if (!button) {{
        return;
      }}
      runProviderTest(button.dataset.providerTest);
    }});
    document.getElementById('replication-recent').addEventListener('click', event => {{
      const button = event.target.closest('button[data-retry-replication-job]');
      if (!button) {{
        return;
      }}
      retryReplicationJob(button.dataset.retryReplicationJob);
    }});
    document.getElementById('replication-failed').addEventListener('click', event => {{
      const button = event.target.closest('button[data-retry-replication-job]');
      if (!button) {{
        return;
      }}
      retryReplicationJob(button.dataset.retryReplicationJob);
    }});
    document.getElementById('replication-targets').addEventListener('click', event => {{
      const button = event.target.closest('button[data-retry-replication-target]');
      if (!button) {{
        return;
      }}
      retryFailedReplicationTarget(button.dataset.retryReplicationTarget);
    }});
    document.getElementById('provider-credentials-grid').addEventListener('click', event => {{
      const saveButton = event.target.closest('button[data-provider-credential-save]');
      if (saveButton) {{
        saveProviderCredential(saveButton.dataset.providerCredentialSave);
        return;
      }}
      const reloadButton = event.target.closest('button[data-provider-credential-reload]');
      if (reloadButton) {{
        refreshProviderCredentials();
      }}
    }});
    document.getElementById('auth-capture-prompts').addEventListener('click', event => {{
      const replyButton = event.target.closest('button[data-auth-prompt-reply]');
      if (!replyButton) {{
        return;
      }}
      submitAuthCapturePrompt(replyButton.dataset.authPromptReply);
    }});
    document.getElementById('reload-status').addEventListener('click', () => refreshStatus());
    document.getElementById('status-auto-refresh-enabled').addEventListener('change', startStatusAutoRefresh);
    document.getElementById('status-auto-refresh-interval-seconds').addEventListener('change', startStatusAutoRefresh);
    document.getElementById('device-start').addEventListener('click', startDeviceFlow);
    document.getElementById('save-topology').addEventListener('click', saveTopology);
    document.getElementById('save-onedrive-policy').addEventListener('click', saveOnedrivePolicy);
    document.getElementById('save-auth-capture-policy').addEventListener('click', saveAuthCapturePolicy);
    document.getElementById('inspect-object-status').addEventListener('click', inspectObjectStatus);
    document.getElementById('object-action-kind').addEventListener('change', renderObjectActionEditor);
    document.getElementById('run-object-action').addEventListener('click', runObjectAction);
    document.getElementById('clear-object-action-history').addEventListener('click', clearObjectActionHistory);
    document.getElementById('export-object-action-history').addEventListener('click', downloadObjectActionHistory);
    document.getElementById('export-object-action-history-csv').addEventListener('click', downloadObjectActionHistoryCsv);
    document.getElementById('export-replication-failed-json').addEventListener('click', downloadReplicationFailedJobs);
    document.getElementById('export-replication-failed-csv').addEventListener('click', downloadReplicationFailedJobsCsv);
    document.getElementById('replication-failed-target-filter').addEventListener('change', () => renderReplication(replicationStateSnapshot));
    [
      'replication-failed-object-filter',
      'replication-failed-start-filter',
      'replication-failed-end-filter',
    ].forEach(id => {{
      const eventName = id === 'replication-failed-object-filter' ? 'input' : 'change';
      document.getElementById(id).addEventListener(eventName, () => renderReplication(replicationStateSnapshot));
    }});
    [
      'object-action-history-action-filter',
      'object-action-history-outcome-filter',
      'object-action-history-provider-filter',
      'object-action-history-operator-filter',
      'object-action-history-object-filter',
      'object-action-history-start-filter',
      'object-action-history-end-filter',
    ].forEach(id => {{
      const eventName = id.endsWith('-filter') && !id.includes('action-filter') && !id.includes('outcome-filter') && !id.includes('provider-filter')
        ? 'input'
        : 'change';
      document.getElementById(id).addEventListener(eventName, () => renderObjectActionHistory(objectActionHistory));
    }});
    [
      'object-action-rename-bucket',
      'object-action-rename-key',
      'object-action-rename-new-key',
      'object-action-source-bucket',
      'object-action-source-key',
      'object-action-destination-bucket',
      'object-action-destination-key',
      'object-action-operator',
      'object-action-ticket',
      'object-action-notes',
    ].forEach(id => {{
      document.getElementById(id).addEventListener('input', renderObjectActionPreview);
    }});
    renderObjectActionEditor();
    renderObjectActionHistory();
    renderTopologyEditor();
    renderStatusRefreshSummary();
    startStatusAutoRefresh();
    refreshStatus();
  </script>
</body>
</html>"#,
        auth_json = serde_json::to_string_pretty(&auth).unwrap_or_else(|_| "{}".to_string()),
        runtime_topology =
            serde_json::to_string_pretty(&runtime).unwrap_or_else(|_| "{}".to_string()),
        desired_topology =
            serde_json::to_string_pretty(&desired).unwrap_or_else(|_| "{}".to_string()),
        provider_catalog = provider_catalog,
        onedrive_policy = onedrive_policy,
        auth_capture_policy = auth_capture_policy,
    );

    Html(body)
}

async fn admin_status(State(state): State<AppState>) -> Result<Json<AdminStatusPayload>, ApiError> {
    let replication_state = replication_state_payload(&state)?;
    let provider_health = provider_health_payloads(&state).await?;
    let onedrive_auth = read_onedrive_auth_status(&state);
    let alerts = build_admin_alerts(&state, &provider_health, &replication_state, &onedrive_auth);
    let object_action_history = control_plane_snapshot(&state).object_action_history;
    let monitoring = monitoring_summary_payload(
        &provider_health,
        &replication_state,
        &object_action_history,
        &alerts,
    );
    let notify = current_notify_status_payload(&state);
    let operations_overview =
        operations_overview_payload(&state, &replication_state, &monitoring, &notify);
    Ok(Json(AdminStatusPayload {
        runtime: runtime_status_payload(&state),
        monitoring,
        operations_overview,
        notify,
        runtime_topology: runtime_topology_payload(&runtime_topology(&state)),
        desired_topology: desired_topology_payload(&state),
        replication: ReplicationQueueSummary {
            pending_jobs: replication_state.in_memory.pending_count,
            recent_jobs: replication_state.in_memory.recent_count,
        },
        replication_state,
        object_action_history,
        object_action_history_limit: state.config.object_action_history_limit,
        provider_health,
        alerts,
        onedrive_auth,
        onedrive_policy: current_onedrive_policy(&state),
        auth_capture_policy: current_auth_capture_policy_payload(&state),
        browser_flow_catalogs: browser_flow_catalog_summary_payloads(&state),
    }))
}

async fn test_provider(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<ProviderTestPayload>, ApiError> {
    let provider = ProviderId::parse(&provider)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    let backend = backend_for_provider(&state, provider)?;
    let topology = runtime_topology(&state);
    let health = match backend.health().await {
        Ok(health) => health,
        Err(error) => unavailable_health(&backend, error),
    };

    Ok(Json(ProviderTestPayload {
        provider: provider.as_str(),
        label: provider_label(provider),
        roles: provider_roles(&topology, provider),
        checked_at_unix_ms: current_unix_ms(),
        health,
    }))
}

async fn retry_replication_job_api(
    State(state): State<AppState>,
    Path(job_id): Path<u64>,
) -> Result<Json<ReplicationRetryPayload>, ApiError> {
    let retried_job = state
        .metadata_store
        .retry_failed_job(job_id, u128::from(current_unix_ms()))
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    state.replication.enqueue_existing_job(retried_job.clone());

    Ok(Json(ReplicationRetryPayload {
        job_id: retried_job.job_id,
        status: retried_job.status.as_str(),
        target: retried_job.target,
        bucket: retried_job.object.bucket,
        key: retried_job.object.key,
    }))
}

async fn retry_replication_target_api(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> Result<Json<ReplicationTargetRetryPayload>, ApiError> {
    let target = ProviderId::parse(&target)
        .map_err(|error| BlobError::Configuration(error.to_string()))?
        .as_str()
        .to_string();
    let retried_jobs = state
        .metadata_store
        .retry_failed_jobs_for_target(&target, u128::from(current_unix_ms()))
        .map_err(|error| BlobError::Configuration(error.to_string()))?;

    for job in &retried_jobs {
        state.replication.enqueue_existing_job(job.clone());
    }

    Ok(Json(ReplicationTargetRetryPayload {
        target,
        retried_jobs: retried_jobs.len(),
        jobs: retried_jobs
            .into_iter()
            .map(|job| ReplicationTargetRetryJobPayload {
                job_id: job.job_id,
                status: job.status.as_str(),
                bucket: job.object.bucket,
                key: job.object.key,
            })
            .collect(),
    }))
}

async fn inspect_object_status(
    State(state): State<AppState>,
    Query(query): Query<ObjectStatusQuery>,
) -> Result<Json<ObjectStatusPayload>, ApiError> {
    let bucket = query.bucket.trim().to_string();
    let key = query.key.trim().to_string();
    if bucket.is_empty() || key.is_empty() {
        return Err(
            BlobError::Configuration("bucket and key are both required".to_string()).into(),
        );
    }
    Ok(Json(
        object_status_payload_for(&state, &bucket, &key).await?,
    ))
}

async fn clear_object_action_history_api(
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    clear_object_action_history(&state)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn run_object_action(
    State(state): State<AppState>,
    Json(input): Json<ObjectActionInput>,
) -> Result<StatusCode, ApiError> {
    let (primary_provider, backend) = current_primary_backend(&state)?;
    let input_for_history = input.clone();
    let description = object_action_description(&input_for_history);
    let warnings = object_action_warnings(&input_for_history, primary_provider);
    let refs = object_action_targets(&input_for_history);
    let before_snapshots = capture_object_action_snapshots(&state, &refs).await;
    let mut replication_jobs = Vec::new();
    let audit = object_action_audit_fields(&input_for_history);
    let action_result: Result<(), BlobError> = match input {
        ObjectActionInput::Rename {
            bucket,
            key,
            new_key,
            ..
        } => {
            let bucket = bucket.trim().to_string();
            let key = key.trim().to_string();
            let new_key = new_key.trim().to_string();
            if key == new_key {
                Ok(())
            } else {
                let source_object = backend.head_object(&bucket, &key).await?;
                backend
                    .rename_object(RenameObjectRequest {
                        container: bucket.clone(),
                        key: key.clone(),
                        new_key: new_key.clone(),
                    })
                    .await?;
                replication_jobs.extend(enqueue_replication_put_for_object(
                    &state,
                    primary_provider,
                    &bucket,
                    &new_key,
                    &source_object,
                )?);
                replication_jobs.extend(enqueue_replication_delete_for_object(
                    &state,
                    primary_provider,
                    &bucket,
                    &key,
                )?);
                Ok(())
            }
        }
        ObjectActionInput::Copy {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => {
            let source_bucket = source_bucket.trim().to_string();
            let source_key = source_key.trim().to_string();
            let destination_bucket = destination_bucket.trim().to_string();
            let destination_key = destination_key.trim().to_string();
            let source_object = backend.head_object(&source_bucket, &source_key).await?;
            backend
                .copy_object(CopyObjectRequest {
                    source_container: source_bucket,
                    source_key,
                    destination_container: destination_bucket.clone(),
                    destination_key: destination_key.clone(),
                })
                .await?;
            replication_jobs.extend(enqueue_replication_put_for_object(
                &state,
                primary_provider,
                &destination_bucket,
                &destination_key,
                &source_object,
            )?);
            Ok(())
        }
        ObjectActionInput::Move {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => {
            let source_bucket = source_bucket.trim().to_string();
            let source_key = source_key.trim().to_string();
            let destination_bucket = destination_bucket.trim().to_string();
            let destination_key = destination_key.trim().to_string();
            if source_bucket == destination_bucket && source_key == destination_key {
                Ok(())
            } else {
                let source_object = backend.head_object(&source_bucket, &source_key).await?;
                backend
                    .move_object(MoveObjectRequest {
                        source_container: source_bucket.clone(),
                        source_key: source_key.clone(),
                        destination_container: destination_bucket.clone(),
                        destination_key: destination_key.clone(),
                    })
                    .await?;
                replication_jobs.extend(enqueue_replication_put_for_object(
                    &state,
                    primary_provider,
                    &destination_bucket,
                    &destination_key,
                    &source_object,
                )?);
                replication_jobs.extend(enqueue_replication_delete_for_object(
                    &state,
                    primary_provider,
                    &source_bucket,
                    &source_key,
                )?);
                Ok(())
            }
        }
    };

    let after_snapshots = capture_object_action_snapshots(&state, &refs).await;
    let outcome = if action_result.is_ok() {
        "success"
    } else {
        "failed"
    };
    let message = action_result
        .as_ref()
        .err()
        .map(|error| error.to_string())
        .unwrap_or_else(|| format!("Completed {description}"));
    record_object_action_history(
        &state,
        object_action_history_entry(
            primary_provider,
            &input_for_history,
            outcome,
            message,
            audit,
            warnings,
            &before_snapshots,
            &after_snapshots,
        ),
    );

    action_result?;
    persist_replication_jobs(&state, &replication_jobs, "object action");
    Ok(StatusCode::NO_CONTENT)
}

async fn list_browser_flow_catalogs(
    State(state): State<AppState>,
) -> Result<Json<Vec<BrowserFlowCatalogSummaryPayload>>, ApiError> {
    Ok(Json(browser_flow_catalog_summary_payloads(&state)))
}

async fn get_browser_flow_catalog(
    State(state): State<AppState>,
    Query(query): Query<BrowserFlowCatalogQuery>,
) -> Result<Json<BrowserFlowCatalogPayload>, ApiError> {
    let provider = query.provider.trim();
    let surface = query.surface.trim();
    if provider.is_empty() || surface.is_empty() {
        return Err(
            BlobError::Configuration("provider and surface are both required".to_string()).into(),
        );
    }

    let entry = state
        .browser_flow_catalogs
        .entries()
        .iter()
        .find(|entry| entry.catalog.provider == provider && entry.catalog.surface == surface)
        .ok_or_else(|| {
            BlobError::NotFound(format!(
                "browser flow catalog not found for {provider}/{surface}"
            ))
        })?;

    Ok(Json(BrowserFlowCatalogPayload {
        provider: entry.catalog.provider.clone(),
        surface: entry.catalog.surface.clone(),
        source_path: entry.source_path.display().to_string(),
        catalog: entry.catalog.clone(),
    }))
}

async fn get_browser_flow_by_id(
    State(state): State<AppState>,
    Path(flow_id): Path<String>,
) -> Result<Json<BrowserFlowPayload>, ApiError> {
    let flow_id = flow_id.trim();
    if flow_id.is_empty() {
        return Err(BlobError::Configuration("flow_id is required".to_string()).into());
    }

    let entry = state
        .browser_flow_catalogs
        .entries()
        .iter()
        .find(|entry| entry.catalog.find_flow(flow_id).is_some())
        .ok_or_else(|| BlobError::NotFound(format!("browser flow not found: {flow_id}")))?;
    let flow = entry
        .catalog
        .find_flow(flow_id)
        .expect("flow should exist after lookup")
        .clone();

    Ok(Json(BrowserFlowPayload {
        provider: entry.catalog.provider.clone(),
        surface: entry.catalog.surface.clone(),
        flow,
    }))
}

async fn run_browser_flow_dry_run(
    State(state): State<AppState>,
    Json(input): Json<BrowserFlowDryRunInput>,
) -> Result<Json<BrowserFlowDryRunPayload>, ApiError> {
    let (provider, surface, flow_id) =
        require_browser_flow_coordinates(&input.provider, &input.surface, &input.flow_id)?;

    let plan = state.browser_flow_catalogs.bind_flow(
        &provider,
        &surface,
        &flow_id,
        &BrowserFlowBindingContext {
            inputs: input.inputs,
            runtime: input.runtime,
        },
    )?;
    let report = DryRunBrowserFlowExecutor.execute(&plan).await?;

    Ok(Json(BrowserFlowDryRunPayload {
        provider: plan.provider,
        surface: plan.surface,
        flow_id: plan.flow.id,
        report,
    }))
}

async fn run_browser_flow_session(
    State(state): State<AppState>,
    Json(input): Json<BrowserFlowSessionRunInput>,
) -> Result<Json<BrowserFlowSessionRunPayload>, ApiError> {
    let cdp_request = BrowserFlowSessionRunInput {
        provider: input.provider.clone(),
        surface: input.surface.clone(),
        flow_id: input.flow_id.clone(),
        auth_session_id: input.auth_session_id.clone(),
        inputs: BTreeMap::new(),
        runtime: BTreeMap::new(),
        cdp_endpoint_url: input.cdp_endpoint_url.clone(),
        cdp_target_selector: input.cdp_target_selector.clone(),
        cdp_target_timeout_ms: input.cdp_target_timeout_ms,
    };
    let (provider, surface, flow_id) =
        require_browser_flow_coordinates(&input.provider, &input.surface, &input.flow_id)?;
    let (_, flow) = browser_flow_catalog_and_flow(&state, &provider, &surface, &flow_id)?;
    let auth_session = upsert_browser_flow_auth_session(
        &state,
        input.auth_session_id.clone(),
        provider.clone(),
        surface.clone(),
        flow_id.clone(),
        input.inputs,
        input.runtime,
    );
    let mut merged_inputs = auth_session.inputs.clone();
    merge_answered_prompts_into_inputs(&state, &auth_session.session_id, &mut merged_inputs);
    let missing_inputs = missing_required_browser_flow_inputs(flow, &merged_inputs);
    if !missing_inputs.is_empty() {
        let _ = update_browser_flow_auth_session(&state, &auth_session.session_id, |session| {
            session.set_status(BROWSER_FLOW_AUTH_SESSION_STATUS_AWAITING_INPUT);
        })?;
        let prompts =
            ensure_auth_capture_prompts_for_inputs(&state, &auth_session, &missing_inputs);
        return Ok(Json(BrowserFlowSessionRunPayload {
            provider,
            surface,
            flow_id,
            auth_session_id: Some(auth_session.session_id),
            status: BROWSER_FLOW_AUTH_SESSION_STATUS_AWAITING_INPUT.to_string(),
            prompts,
            cdp_endpoint_url: String::new(),
            cdp_target_selector: None,
            cdp_target_timeout_ms: None,
            report: None,
        }));
    }
    let _ = update_browser_flow_auth_session(&state, &auth_session.session_id, |session| {
        session.set_status(BROWSER_FLOW_AUTH_SESSION_STATUS_RESUMED);
    })?;
    let cdp = resolve_browser_flow_cdp_config(&state, &cdp_request)?;
    let session = CdpBrowserFlowSession::connect(&cdp).await?;
    run_browser_flow_prerequisite_if_needed(
        &state,
        &provider,
        &surface,
        flow,
        &auth_session.session_id,
        &merged_inputs,
        &auth_session.runtime,
        &session,
    )
    .await?;
    let session_after_prerequisite =
        browser_flow_auth_session_snapshot(&state, &auth_session.session_id)?;
    let plan = browser_flow_plan(
        state.browser_flow_catalogs.as_ref(),
        &provider,
        &surface,
        &flow_id,
        merged_inputs,
        session_after_prerequisite.runtime.clone(),
    )?;
    let report = execute_browser_flow_plan_with_output_capture(
        &state,
        &auth_session.session_id,
        &plan,
        &session,
    )
    .await?;
    let _ = update_browser_flow_auth_session(&state, &auth_session.session_id, |session| {
        session.set_completed(report.clone());
    })?;

    Ok(Json(BrowserFlowSessionRunPayload {
        provider,
        surface,
        flow_id,
        auth_session_id: Some(auth_session.session_id),
        status: BROWSER_FLOW_AUTH_SESSION_STATUS_COMPLETED.to_string(),
        prompts: Vec::new(),
        cdp_endpoint_url: cdp.endpoint_url,
        cdp_target_selector: cdp.target_selector,
        cdp_target_timeout_ms: cdp.target_timeout_ms,
        report: Some(report),
    }))
}

async fn get_browser_flow_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<BrowserFlowAuthSessionPayload>, ApiError> {
    let session = state
        .auth
        .browser_flow_sessions
        .lock()
        .expect("browser flow auth session store poisoned")
        .get(&session_id)
        .cloned()
        .ok_or_else(|| {
            BlobError::NotFound(format!("browser flow auth session not found: {session_id}"))
        })?;
    Ok(Json(browser_flow_auth_session_payload(&state, &session)))
}

async fn get_onedrive_policy(
    State(state): State<AppState>,
) -> Result<Json<OnedrivePolicy>, ApiError> {
    Ok(Json(current_onedrive_policy(&state)))
}

async fn get_auth_capture_policy(
    State(state): State<AppState>,
) -> Result<Json<AuthCapturePolicyPayload>, ApiError> {
    Ok(Json(current_auth_capture_policy_payload(&state)))
}

async fn update_onedrive_policy(
    State(state): State<AppState>,
    Json(input): Json<OnedrivePolicyInput>,
) -> Result<Json<OnedrivePolicy>, ApiError> {
    let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
    control_plane.onedrive_policy = OnedrivePolicy::from_input(input);
    persist_control_plane_state(&state.config.control_plane_file, &control_plane)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    Ok(Json(control_plane.onedrive_policy.clone()))
}

async fn update_auth_capture_policy(
    State(state): State<AppState>,
    Json(input): Json<AuthCapturePolicyInput>,
) -> Result<Json<AuthCapturePolicyPayload>, ApiError> {
    let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
    control_plane.auth_capture_policy.apply_input(input);
    persist_control_plane_state(&state.config.control_plane_file, &control_plane)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    Ok(Json(control_plane.auth_capture_policy.payload()))
}

async fn list_auth_capture_prompts(
    State(state): State<AppState>,
) -> Result<Json<Vec<AuthCapturePrompt>>, ApiError> {
    let mut prompts = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned")
        .values()
        .cloned()
        .map(|prompt| prompt.sanitized())
        .collect::<Vec<_>>();
    prompts.sort_by_key(|prompt| prompt.created_at_unix_ms);
    prompts.reverse();
    Ok(Json(prompts))
}

async fn create_auth_capture_prompt(
    State(state): State<AppState>,
    Json(input): Json<AuthCapturePromptCreateInput>,
) -> Result<Json<AuthCapturePrompt>, ApiError> {
    let prompt = AuthCapturePrompt::from_input(input);
    state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned")
        .insert(prompt.prompt_id.clone(), prompt.clone());
    Ok(Json(prompt.sanitized()))
}

async fn get_auth_capture_prompt(
    State(state): State<AppState>,
    Path(prompt_id): Path<String>,
) -> Result<Json<AuthCapturePrompt>, ApiError> {
    let prompt = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned")
        .get(&prompt_id)
        .cloned()
        .ok_or_else(|| {
            BlobError::NotFound(format!("auth capture prompt not found: {prompt_id}"))
        })?;
    Ok(Json(prompt))
}

async fn reply_auth_capture_prompt(
    State(state): State<AppState>,
    Path(prompt_id): Path<String>,
    Json(input): Json<AuthCapturePromptReplyInput>,
) -> Result<Json<AuthCapturePrompt>, ApiError> {
    let mut prompts = state
        .auth
        .capture_prompts
        .lock()
        .expect("auth capture prompt store poisoned");
    let prompt = prompts.get_mut(&prompt_id).ok_or_else(|| {
        BlobError::NotFound(format!("auth capture prompt not found: {prompt_id}"))
    })?;
    prompt.answer(input.value.trim().to_string());
    let sanitized = prompt.sanitized();
    let session_id = prompt.session_id.clone();
    drop(prompts);
    if let Some(session_id) = session_id {
        let _ = update_browser_flow_auth_session(&state, &session_id, |session| {
            session.set_status(BROWSER_FLOW_AUTH_SESSION_STATUS_ANSWERED);
        });
    }
    Ok(Json(sanitized))
}

async fn update_topology(
    State(state): State<AppState>,
    Json(input): Json<TopologyUpdateInput>,
) -> Result<Json<DesiredTopologyPayload>, ApiError> {
    let current = runtime_topology(&state);
    let topology = TopologyPolicy::from_input(TopologyInput {
        primary_provider: input.primary_provider,
        sync_targets: input.sync_targets,
        fallback_read_order: input.fallback_read_order,
        onedrive_enabled: state.config.onedrive.enabled,
        replication_mode: current.replication_mode,
    })
    .map_err(|error| BlobError::Configuration(error.to_string()))?;

    let payload = DesiredTopologyPayload {
        primary_provider: topology.primary_provider_name(),
        sync_targets: topology.sync_target_names(),
        fallback_read_order: topology.fallback_read_order_names(),
        restart_required: false,
    };

    let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
    control_plane.topology = topology;
    persist_control_plane_state(&state.config.control_plane_file, &control_plane)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    Ok(Json(payload))
}

async fn onedrive_auth_status(
    State(state): State<AppState>,
) -> Result<Json<OneDriveAuthStatusPayload>, ApiError> {
    Ok(Json(read_onedrive_auth_status(&state)))
}

async fn get_provider_credentials(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Json<ProviderCredentialPayload>, ApiError> {
    let provider = ProviderId::parse(&provider)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    Ok(Json(current_provider_credential_payload(&state, provider)?))
}

async fn update_provider_credentials(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Json(input): Json<ProviderCredentialInput>,
) -> Result<Json<ProviderCredentialPayload>, ApiError> {
    let provider = ProviderId::parse(&provider)
        .map_err(|error| BlobError::Configuration(error.to_string()))?;
    let record = ProviderCredentialRecord::from(input);
    persist_provider_credential_record(&state.config, provider, &record)?;
    rebuild_backend_for_provider(&state, provider)?;
    Ok(Json(current_provider_credential_payload(&state, provider)?))
}

async fn start_onedrive_web_login(State(state): State<AppState>) -> Result<Response, ApiError> {
    let effective_config = effective_onedrive_config_from_app(&state.config);
    let state_token = random_urlsafe_token(18);
    let code_verifier = random_urlsafe_token(48);
    let code_challenge = pkce_code_challenge(&code_verifier);
    let url = build_onedrive_authorize_url(&effective_config, &state_token, &code_challenge)?;

    state
        .auth
        .pending_pkce
        .lock()
        .expect("pkce store poisoned")
        .insert(
            state_token,
            PendingPkceLogin {
                code_verifier,
                created_at_unix_ms: current_unix_ms(),
            },
        );

    Ok((
        StatusCode::FOUND,
        [(
            LOCATION,
            HeaderValue::from_str(&url).expect("redirect url should be valid"),
        )],
    )
        .into_response())
}

async fn handle_onedrive_callback(
    State(state): State<AppState>,
    Query(query): Query<OneDriveCallbackQuery>,
) -> Html<String> {
    let result = async {
        let effective_config = effective_onedrive_config_from_app(&state.config);
        if let Some(error) = query.error.as_deref() {
            let description = query
                .error_description
                .as_deref()
                .unwrap_or("unknown error");
            return Err(BlobError::Upstream(format!(
                "OneDrive authorization failed: {error} ({description})"
            )));
        }

        let code = query
            .code
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                BlobError::Configuration("callback query is missing code".to_string())
            })?;
        let state_token = query
            .state
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                BlobError::Configuration("callback query is missing state".to_string())
            })?;

        let pkce = state
            .auth
            .pending_pkce
            .lock()
            .expect("pkce store poisoned")
            .remove(state_token)
            .ok_or_else(|| {
                BlobError::Configuration("pkce state was not found or has expired".to_string())
            })?;

        let session = exchange_authorization_code(
            &state.auth.http_client,
            &effective_config,
            code,
            &pkce.code_verifier,
        )
        .await?;
        let session_file = onedrive_session_file(&effective_config).map_err(|error| error.0)?;
        persist_oauth_session(session_file, &session)?;
        Ok::<_, BlobError>((session_file.to_string(), pkce.created_at_unix_ms))
    }
    .await;

    match result {
        Ok((session_file, created_at_unix_ms)) => Html(format!(
            "<html><body><h1>OneDrive Connected</h1><p>OAuth session saved to <code>{}</code>.</p><p>PKCE state created at unix_ms={}</p><p>You can return to the admin UI now.</p></body></html>",
            xml_escape(&session_file),
            created_at_unix_ms
        )),
        Err(error) => Html(format!(
            "<html><body><h1>OneDrive Authorization Failed</h1><pre>{}</pre></body></html>",
            xml_escape(&error.to_string())
        )),
    }
}

async fn start_onedrive_device_flow(
    State(state): State<AppState>,
) -> Result<Json<OneDriveDeviceFlowPayload>, ApiError> {
    let effective_config = effective_onedrive_config_from_app(&state.config);
    if !effective_config.enabled {
        return Err(BlobError::Configuration("onedrive provider disabled".to_string()).into());
    }
    let session_file = onedrive_session_file(&effective_config)?;
    let flow = request_device_code(&state.auth.http_client, &effective_config)
        .await
        .map_err(ApiError::from)?;
    let flow_id = random_urlsafe_token(16);
    let record = DeviceFlowRecord {
        device_code: flow.device_code,
        interval_secs: flow.interval.unwrap_or(5).max(2),
        expires_at_unix_ms: current_unix_ms() + flow.expires_in.saturating_mul(1000),
        payload: OneDriveDeviceFlowPayload {
            flow_id: flow_id.clone(),
            verification_uri: flow.verification_uri,
            verification_uri_complete: flow.verification_uri_complete,
            user_code: flow.user_code,
            message: flow.message,
            expires_in: flow.expires_in,
            interval: flow.interval.unwrap_or(5).max(2),
            status: "pending",
            error: None,
            completed_at_unix_ms: None,
        },
    };

    state
        .auth
        .device_flows
        .lock()
        .expect("device flow store poisoned")
        .insert(flow_id.clone(), record.clone());

    let auth_state = state.auth.clone();
    let session_file = session_file.to_string();
    tokio::spawn(async move {
        run_device_flow_poller(auth_state, effective_config, &flow_id, &session_file).await;
    });

    Ok(Json(record.payload))
}

async fn get_onedrive_device_flow(
    State(state): State<AppState>,
    Path(flow_id): Path<String>,
) -> Result<Json<OneDriveDeviceFlowPayload>, ApiError> {
    state
        .auth
        .device_flows
        .lock()
        .expect("device flow store poisoned")
        .get(&flow_id)
        .map(|record| Json(record.payload.clone()))
        .ok_or_else(|| BlobError::NotFound(format!("device flow not found: {flow_id}")).into())
}

async fn run_device_flow_poller(
    auth: Arc<AuthBrokerState>,
    onedrive_config: OneDriveConfig,
    flow_id: &str,
    session_file: &str,
) {
    loop {
        let snapshot = {
            auth.device_flows
                .lock()
                .expect("device flow store poisoned")
                .get(flow_id)
                .cloned()
        };
        let Some(record) = snapshot else {
            return;
        };

        if current_unix_ms() >= record.expires_at_unix_ms {
            if let Some(flow) = auth
                .device_flows
                .lock()
                .expect("device flow store poisoned")
                .get_mut(flow_id)
            {
                flow.payload.status = "failed";
                flow.payload.error = Some("device code expired".to_string());
            }
            return;
        }

        match poll_device_code_once(&auth.http_client, &onedrive_config, &record.device_code).await
        {
            Ok(Ok(session)) => {
                let result = persist_oauth_session(session_file, &session)
                    .map_err(|error| error.to_string());
                if let Some(flow) = auth
                    .device_flows
                    .lock()
                    .expect("device flow store poisoned")
                    .get_mut(flow_id)
                {
                    match result {
                        Ok(()) => {
                            flow.payload.status = "completed";
                            flow.payload.error = None;
                            flow.payload.completed_at_unix_ms = Some(current_unix_ms());
                        }
                        Err(error) => {
                            flow.payload.status = "failed";
                            flow.payload.error = Some(error);
                        }
                    }
                }
                return;
            }
            Ok(Err("slow_down")) => {
                if let Some(flow) = auth
                    .device_flows
                    .lock()
                    .expect("device flow store poisoned")
                    .get_mut(flow_id)
                {
                    flow.interval_secs = flow.interval_secs.saturating_add(5);
                    flow.payload.interval = flow.interval_secs;
                }
            }
            Ok(Err("authorization_pending")) => {}
            Ok(Err(_)) => {}
            Err(error) => {
                if let Some(flow) = auth
                    .device_flows
                    .lock()
                    .expect("device flow store poisoned")
                    .get_mut(flow_id)
                {
                    flow.payload.status = "failed";
                    flow.payload.error = Some(error.to_string());
                }
                return;
            }
        }

        sleep(Duration::from_secs(record.interval_secs)).await;
    }
}

fn build_backend(config: &AppConfig, provider: ProviderId) -> Result<DynBackend, BlobError> {
    match provider {
        ProviderId::Stub => Ok(Arc::new(StubBackend::new())),
        ProviderId::Unicom => {
            let credentials = load_provider_credential_record_or_default(config, provider);
            Ok(Arc::new(UnicomBlobAdapter::new(UnicomConfig {
                base_url: env_or("CCBG_UNICOM_BASE_URL", "https://panservice.mail.wo.cn"),
                token_source: override_token_source(
                    resolve_token_source("CCBG_UNICOM"),
                    &credentials.token,
                ),
                outbound_ip_family: env_outbound_ip_family(
                    "CCBG_UNICOM_IP_FAMILY",
                    OutboundIpFamily::Auto,
                )?,
                cookie_header: credentials.cookie_header.or_else(|| {
                    env_opt_or_file(
                        "CCBG_UNICOM_COOKIE_HEADER",
                        "CCBG_UNICOM_COOKIE_HEADER_FILE",
                    )
                }),
                user_agent: env_or(
                    "CCBG_UNICOM_USER_AGENT",
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
                ),
                request_timeout_secs: env_u64("CCBG_UNICOM_TIMEOUT_SECS", 30),
                request_origin: Some(env_or("CCBG_UNICOM_REQUEST_ORIGIN", "https://pan.wo.cn")),
                request_referer: Some(env_or("CCBG_UNICOM_REQUEST_REFERER", "https://pan.wo.cn/")),
                request_header_client_id: env_or("CCBG_UNICOM_HEADER_CLIENT_ID", "1001000021"),
                request_header_app_version: env_or("CCBG_UNICOM_APP_VERSION", "5g-h5"),
                dispatcher_client_id: env_or("CCBG_UNICOM_DISPATCHER_CLIENT_ID", "1001000021"),
                dispatcher_channel: env_or("CCBG_UNICOM_DISPATCHER_CHANNEL", "wohome"),
                dispatcher_secret: env_or("CCBG_UNICOM_DISPATCHER_SECRET", "Py1J67PAQoCb8Iel"),
                health_probe_operation: env_or("CCBG_UNICOM_AUTH_PROBE_OPERATION", "QueryAllFiles"),
                health_probe_style: env_or("CCBG_UNICOM_AUTH_PROBE_STYLE", "wohome-secret"),
                health_probe_body_json: env_or(
                    "CCBG_UNICOM_AUTH_PROBE_BODY_JSON",
                    "{\"spaceType\":\"0\",\"parentDirectoryId\":\"0\",\"pageNum\":0,\"pageSize\":50,\"sortRule\":0}",
                ),
                family_id: credentials
                    .family_id
                    .or_else(|| env_opt("CCBG_UNICOM_FAMILY_ID")),
                family_space_type: env_or("CCBG_UNICOM_FAMILY_SPACE_TYPE", "1"),
                family_root_directory_id: env_or("CCBG_UNICOM_FAMILY_ROOT_DIRECTORY_ID", "0"),
                upload_url: env_or(
                    "CCBG_UNICOM_UPLOAD_URL",
                    "https://bjupload.pan.wo.cn:32443/openapi/client/upload2C",
                ),
                upload_chunk_size_bytes: env_u64(
                    "CCBG_UNICOM_UPLOAD_CHUNK_SIZE_BYTES",
                    8 * 1024 * 1024,
                ),
                native_capability_catalog_path: Some(
                    env_opt("CCBG_UNICOM_NATIVE_CAPABILITY_CATALOG_FILE").unwrap_or_else(|| {
                        provider_capability_catalog_path(config, provider, "native")
                            .display()
                            .to_string()
                    }),
                ),
            })?))
        }
        ProviderId::Telecom => {
            let credentials = load_provider_credential_record_or_default(config, provider);
            Ok(Arc::new(TelecomBlobAdapter::new(TelecomConfig {
                base_url: env_or("CCBG_TELECOM_BASE_URL", "https://cloud.189.cn"),
                token_source: override_token_source(
                    resolve_token_source("CCBG_TELECOM"),
                    &credentials.token,
                ),
                outbound_ip_family: env_outbound_ip_family(
                    "CCBG_TELECOM_IP_FAMILY",
                    OutboundIpFamily::Auto,
                )?,
                browser_id: credentials.browser_id.or_else(|| {
                    env_opt_or_file("CCBG_TELECOM_BROWSER_ID", "CCBG_TELECOM_BROWSER_ID_FILE")
                }),
                cookie_header: credentials.cookie_header.or_else(|| {
                    env_opt_or_file(
                        "CCBG_TELECOM_COOKIE_HEADER",
                        "CCBG_TELECOM_COOKIE_HEADER_FILE",
                    )
                }),
                user_agent: env_or(
                    "CCBG_TELECOM_USER_AGENT",
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
                ),
                request_timeout_secs: env_u64("CCBG_TELECOM_TIMEOUT_SECS", 30),
                sign_type: env_or("CCBG_TELECOM_SIGN_TYPE", "1"),
                root_folder_id: credentials
                    .root_folder_id
                    .unwrap_or_else(|| env_or("CCBG_TELECOM_ROOT_FOLDER_ID", "-11")),
                page_size: env_usize("CCBG_TELECOM_PAGE_SIZE", 60),
            })?))
        }
        ProviderId::Mobile => {
            let credentials = load_provider_credential_record_or_default(config, provider);
            Ok(Arc::new(MobileBlobAdapter::new(MobileConfig {
                base_url: env_or("CCBG_MOBILE_BASE_URL", "https://yun.139.com"),
                token_source: override_token_source(
                    resolve_token_source("CCBG_MOBILE"),
                    &credentials.token,
                ),
                outbound_ip_family: env_outbound_ip_family(
                    "CCBG_MOBILE_IP_FAMILY",
                    OutboundIpFamily::Auto,
                )?,
                cookie_header: credentials
                    .cookie_header
                    .or_else(|| env_opt("CCBG_MOBILE_COOKIE_HEADER")),
                user_agent: env_or(
                    "CCBG_MOBILE_USER_AGENT",
                    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
                ),
                request_timeout_secs: env_u64("CCBG_MOBILE_TIMEOUT_SECS", 30),
            })))
        }
        ProviderId::Onedrive => Ok(Arc::new(OneDriveBlobAdapter::new(
            effective_onedrive_config_from_app(config),
        ))),
    }
}

fn build_all_backends(config: &AppConfig) -> Result<Vec<ConfiguredBackend>, BlobError> {
    [
        ProviderId::Stub,
        ProviderId::Unicom,
        ProviderId::Telecom,
        ProviderId::Mobile,
        ProviderId::Onedrive,
    ]
    .into_iter()
    .map(|provider| {
        Ok(ConfiguredBackend {
            provider,
            backend: build_backend(config, provider)?,
        })
    })
    .collect()
}

fn backend_for_provider_from(
    backends: &[ConfiguredBackend],
    provider: ProviderId,
) -> Option<DynBackend> {
    backends
        .iter()
        .find(|backend| backend.provider == provider)
        .map(|backend| backend.backend.clone())
}

fn backends_snapshot(state: &AppState) -> Vec<ConfiguredBackend> {
    state
        .backends
        .lock()
        .expect("backend registry poisoned")
        .clone()
}

fn replace_backend_in_registry(
    backends: &mut Vec<ConfiguredBackend>,
    provider: ProviderId,
    backend: DynBackend,
) {
    if let Some(configured) = backends.iter_mut().find(|item| item.provider == provider) {
        configured.backend = backend;
        return;
    }

    backends.push(ConfiguredBackend { provider, backend });
}

fn set_backend_in_registry(state: &AppState, provider: ProviderId, backend: DynBackend) {
    let mut backends = state.backends.lock().expect("backend registry poisoned");
    replace_backend_in_registry(&mut backends, provider, backend);
}

fn rebuild_backend_for_provider(state: &AppState, provider: ProviderId) -> Result<(), BlobError> {
    if provider == ProviderId::Stub {
        return Err(BlobError::Configuration(
            "stub provider does not support credential storage".to_string(),
        ));
    }

    let backend = build_backend(&state.config, provider)?;
    set_backend_in_registry(state, provider, backend);
    Ok(())
}

fn runtime_topology(state: &AppState) -> TopologyPolicy {
    control_plane_snapshot(state).topology
}

fn backend_for_provider(state: &AppState, provider: ProviderId) -> Result<DynBackend, BlobError> {
    let backends = backends_snapshot(state);
    backend_for_provider_from(&backends, provider).ok_or_else(|| {
        BlobError::Configuration(format!(
            "backend is not configured for provider {}",
            provider.as_str()
        ))
    })
}

fn backend_for_provider_name(state: &AppState, provider: &str) -> Result<DynBackend, String> {
    let provider = ProviderId::parse(provider)
        .map_err(|error| format!("invalid provider name {provider}: {error}"))?;
    backend_for_provider(state, provider).map_err(|error| error.to_string())
}

fn current_primary_backend(state: &AppState) -> Result<(ProviderId, DynBackend), BlobError> {
    let topology = runtime_topology(state);
    let provider = topology.primary_provider;
    Ok((provider, backend_for_provider(state, provider)?))
}

fn sync_backends_for_topology(
    state: &AppState,
    topology: &TopologyPolicy,
) -> Vec<ConfiguredBackend> {
    let backends = backends_snapshot(state);
    topology
        .sync_targets
        .iter()
        .filter_map(|provider| {
            backend_for_provider_from(&backends, *provider).map(|backend| ConfiguredBackend {
                provider: *provider,
                backend,
            })
        })
        .collect()
}

fn ordered_fallback_backends(state: &AppState) -> Vec<ConfiguredBackend> {
    let topology = runtime_topology(state);
    let sync_backends = sync_backends_for_topology(state, &topology);
    topology
        .fallback_read_order
        .iter()
        .filter_map(|provider| {
            if *provider == ProviderId::Onedrive && !current_onedrive_policy(state).fallback_enabled
            {
                return None;
            }
            sync_backends
                .iter()
                .find(|backend| backend.provider == *provider)
                .cloned()
        })
        .collect()
}

fn remember_read_error(first_non_not_found: &mut Option<BlobError>, error: BlobError) {
    if first_non_not_found.is_none() && !matches!(error, BlobError::NotFound(_)) {
        *first_non_not_found = Some(error);
    }
}

fn fallback_not_found(bucket: &str, key: Option<&str>) -> BlobError {
    match key {
        Some(key) => BlobError::NotFound(format!("object not found: {bucket}/{key}")),
        None => BlobError::NotFound(format!("container not found: {bucket}")),
    }
}

fn apply_read_source_headers(headers: &mut HeaderMap, source: ReadSource) {
    headers.insert(
        SOURCE_PROVIDER_HEADER,
        HeaderValue::from_static(source.provider.as_str()),
    );

    if let Some(fallback_from) = source.fallback_from {
        headers.insert(
            FALLBACK_FROM_HEADER,
            HeaderValue::from_static(fallback_from.as_str()),
        );
    }
}

fn ensure_object_within_in_memory_limit(config: &AppConfig, size: u64) -> Result<(), S3Error> {
    if size <= config.max_in_memory_object_bytes as u64 {
        Ok(())
    } else {
        Err(S3Error::entity_too_large(format!(
            "Object size {size} exceeds the configured in-memory limit of {} bytes. The current data path is non-streaming; lower object size or raise CCBG_MAX_IN_MEMORY_OBJECT_BYTES.",
            config.max_in_memory_object_bytes
        )))
    }
}

fn ensure_replication_object_within_in_memory_limit(
    config: &AppConfig,
    job: &ReplicationJob,
) -> Result<(), String> {
    match job.object.size {
        Some(size) if size > config.max_in_memory_object_bytes as u64 => Err(format!(
            "object size {size} exceeds in-memory limit {} for non-streaming replication",
            config.max_in_memory_object_bytes
        )),
        _ => Ok(()),
    }
}

fn provider_allowed_for_replication(
    state: &AppState,
    provider: ProviderId,
    operation: ReplicationOperation,
    bucket: &str,
    key: &str,
) -> Result<bool, BlobError> {
    if provider != ProviderId::Onedrive {
        return Ok(true);
    }

    let policy = current_onedrive_policy(state);
    if matches!(operation, ReplicationOperation::Put) {
        return Ok(policy.replication_enabled && policy.matches_object(bucket, key));
    }

    if policy.replication_enabled && policy.matches_object(bucket, key) {
        return Ok(true);
    }

    let existing = state
        .metadata_store
        .latest_job_for_object(provider.as_str(), bucket, key)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;
    Ok(existing.is_some())
}

fn provider_allowed_for_fallback_bucket(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
) -> bool {
    if provider != ProviderId::Onedrive {
        return true;
    }

    let policy = current_onedrive_policy(state);
    policy.fallback_enabled && policy.matches_bucket(bucket)
}

fn provider_allowed_for_fallback_object(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
    key: &str,
) -> bool {
    if provider != ProviderId::Onedrive {
        return true;
    }

    let policy = current_onedrive_policy(state);
    policy.fallback_enabled && policy.matches_object(bucket, key)
}

fn effective_topology_for_replication(
    state: &AppState,
    operation: ReplicationOperation,
    bucket: &str,
    key: &str,
) -> Result<TopologyPolicy, BlobError> {
    let mut topology = runtime_topology(state);
    let mut effective_targets = Vec::with_capacity(topology.sync_targets.len());
    for provider in &topology.sync_targets {
        if provider_allowed_for_replication(state, *provider, operation.clone(), bucket, key)? {
            effective_targets.push(*provider);
        }
    }
    topology.sync_targets = effective_targets;
    topology.fallback_read_order = topology
        .fallback_read_order
        .into_iter()
        .filter(|provider| topology.sync_targets.contains(provider))
        .collect();
    Ok(topology)
}

fn enqueue_replication_put_for_object(
    state: &AppState,
    source_provider: ProviderId,
    bucket: &str,
    key: &str,
    object: &blob_core::ObjectInfo,
) -> Result<Vec<ReplicationJob>, BlobError> {
    let effective_topology =
        effective_topology_for_replication(state, ReplicationOperation::Put, bucket, key)?;
    Ok(state.replication.enqueue_put(
        &effective_topology,
        Some(source_provider.as_str().to_string()),
        bucket.to_string(),
        key.to_string(),
        object.etag.clone(),
        object.size,
        object.content_type.clone(),
    ))
}

fn enqueue_replication_delete_for_object(
    state: &AppState,
    source_provider: ProviderId,
    bucket: &str,
    key: &str,
) -> Result<Vec<ReplicationJob>, BlobError> {
    let effective_topology =
        effective_topology_for_replication(state, ReplicationOperation::Delete, bucket, key)?;
    Ok(state.replication.enqueue_delete(
        &effective_topology,
        Some(source_provider.as_str().to_string()),
        bucket.to_string(),
        key.to_string(),
    ))
}

fn persist_replication_jobs(state: &AppState, jobs: &[ReplicationJob], reason: &'static str) {
    if let Err(error) = state.metadata_store.enqueue_jobs(jobs) {
        warn!(
            reason,
            queued_jobs = jobs.len(),
            error = %error,
            "failed to persist replication jobs"
        );
    } else if !jobs.is_empty() {
        info!(
            reason,
            queued_jobs = jobs.len(),
            "replication jobs enqueued"
        );
    }
}

#[derive(Debug, Clone)]
struct ObjectActionTargetRef {
    label: String,
    bucket: String,
    key: String,
}

fn object_action_name(input: &ObjectActionInput) -> &'static str {
    match input {
        ObjectActionInput::Rename { .. } => "rename",
        ObjectActionInput::Copy { .. } => "copy",
        ObjectActionInput::Move { .. } => "move",
    }
}

fn object_action_description(input: &ObjectActionInput) -> String {
    match input {
        ObjectActionInput::Rename {
            bucket,
            key,
            new_key,
            ..
        } => format!("{bucket}/{key} -> {bucket}/{new_key}"),
        ObjectActionInput::Copy {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => format!("{source_bucket}/{source_key} -> {destination_bucket}/{destination_key}"),
        ObjectActionInput::Move {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => format!("{source_bucket}/{source_key} -> {destination_bucket}/{destination_key}"),
    }
}

fn object_parent_path(key: &str) -> &str {
    let trimmed = key.trim().trim_matches('/');
    trimmed
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn object_action_warnings(input: &ObjectActionInput, primary_provider: ProviderId) -> Vec<String> {
    match input {
        ObjectActionInput::Rename {
            bucket: _,
            key,
            new_key,
            ..
        } => {
            let mut warnings = Vec::new();
            if key.trim() == new_key.trim() {
                warnings.push(
                    "This is a no-op rename. The gateway will return success without changing the object."
                        .to_string(),
                );
            }
            if matches!(primary_provider, ProviderId::Unicom)
                && object_parent_path(key) != object_parent_path(new_key)
            {
                warnings.push(
                    "Current Unicom rename only supports staying in the same parent directory. Use move for cross-directory changes."
                        .to_string(),
                );
            }
            warnings
        }
        ObjectActionInput::Copy {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => {
            let mut warnings = Vec::new();
            if source_bucket.trim() == destination_bucket.trim()
                && source_key.trim() == destination_key.trim()
            {
                warnings.push(
                    "This copies an object onto the same bucket/key. The destination may be overwritten or treated as a provider-specific no-op."
                        .to_string(),
                );
            }
            if source_bucket.trim() != destination_bucket.trim() {
                warnings.push(
                    "This action crosses buckets/containers. Confirm the destination scope is intentional."
                        .to_string(),
                );
            }
            warnings.push(
                "Destination writes may overwrite an existing object at the destination key."
                    .to_string(),
            );
            warnings
        }
        ObjectActionInput::Move {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => {
            let mut warnings = Vec::new();
            if source_bucket.trim() == destination_bucket.trim()
                && source_key.trim() == destination_key.trim()
            {
                warnings.push(
                    "This is a no-op move. The gateway will return success without changing the object."
                        .to_string(),
                );
            }
            if source_bucket.trim() != destination_bucket.trim() {
                warnings.push(
                    "This action crosses buckets/containers. Confirm the destination scope is intentional."
                        .to_string(),
                );
            }
            warnings.push(
                "Destination writes may overwrite an existing object at the destination key."
                    .to_string(),
            );
            warnings
        }
    }
}

fn object_action_targets(input: &ObjectActionInput) -> Vec<ObjectActionTargetRef> {
    let targets = match input {
        ObjectActionInput::Rename {
            bucket,
            key,
            new_key,
            ..
        } => vec![
            ObjectActionTargetRef {
                label: "source before / old key after".to_string(),
                bucket: bucket.trim().to_string(),
                key: key.trim().to_string(),
            },
            ObjectActionTargetRef {
                label: "renamed target".to_string(),
                bucket: bucket.trim().to_string(),
                key: new_key.trim().to_string(),
            },
        ],
        ObjectActionInput::Copy {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        }
        | ObjectActionInput::Move {
            source_bucket,
            source_key,
            destination_bucket,
            destination_key,
            ..
        } => vec![
            ObjectActionTargetRef {
                label: "source".to_string(),
                bucket: source_bucket.trim().to_string(),
                key: source_key.trim().to_string(),
            },
            ObjectActionTargetRef {
                label: "destination".to_string(),
                bucket: destination_bucket.trim().to_string(),
                key: destination_key.trim().to_string(),
            },
        ],
    };

    let mut deduped = Vec::with_capacity(targets.len());
    for target in targets {
        if deduped.iter().any(|existing: &ObjectActionTargetRef| {
            existing.bucket == target.bucket && existing.key == target.key
        }) {
            continue;
        }
        deduped.push(target);
    }
    deduped
}

fn object_status_delta(before: &ObjectStatusPayload, after: &ObjectStatusPayload) -> Vec<String> {
    let mut changes = Vec::new();
    let before_source = before.gateway_read_source.unwrap_or("none");
    let after_source = after.gateway_read_source.unwrap_or("none");
    if before_source != after_source {
        changes.push(format!(
            "gateway source: {} -> {}",
            before_source, after_source
        ));
    }
    let before_error = before.gateway_error.as_deref().unwrap_or("none");
    let after_error = after.gateway_error.as_deref().unwrap_or("none");
    if before_error != after_error {
        changes.push(format!("gateway error: {before_error} -> {after_error}"));
    }
    let before_existing = before
        .provider_states
        .iter()
        .filter(|state| state.exists)
        .count();
    let after_existing = after
        .provider_states
        .iter()
        .filter(|state| state.exists)
        .count();
    if before_existing != after_existing {
        changes.push(format!(
            "exists on providers: {} -> {}",
            before_existing, after_existing
        ));
    }
    let before_readable = before
        .provider_states
        .iter()
        .filter(|state| state.readable_via_gateway)
        .count();
    let after_readable = after
        .provider_states
        .iter()
        .filter(|state| state.readable_via_gateway)
        .count();
    if before_readable != after_readable {
        changes.push(format!(
            "gateway-readable providers: {} -> {}",
            before_readable, after_readable
        ));
    }

    let before_index = object_status_index(before);
    let after_index = object_status_index(after);
    let mut providers: Vec<String> = before_index.keys().cloned().collect();
    for provider in after_index.keys() {
        if !providers.iter().any(|existing| existing == provider) {
            providers.push(provider.clone());
        }
    }

    for provider in providers {
        let prev = before_index.get(&provider);
        let next = after_index.get(&provider);
        let prev_exists = prev.map(|state| state.exists).unwrap_or(false);
        let next_exists = next.map(|state| state.exists).unwrap_or(false);
        if prev_exists != next_exists {
            changes.push(format!(
                "{} exists: {} -> {}",
                provider,
                yes_no(prev_exists),
                yes_no(next_exists)
            ));
        }
        let prev_readable = prev
            .map(|state| state.readable_via_gateway)
            .unwrap_or(false);
        let next_readable = next
            .map(|state| state.readable_via_gateway)
            .unwrap_or(false);
        if prev_readable != next_readable {
            changes.push(format!(
                "{} gateway-readable: {} -> {}",
                provider,
                yes_no(prev_readable),
                yes_no(next_readable)
            ));
        }
    }

    changes
}

fn object_status_index(
    payload: &ObjectStatusPayload,
) -> HashMap<String, &ObjectProviderStatusPayload> {
    payload
        .provider_states
        .iter()
        .map(|state| (state.provider.to_string(), state))
        .collect()
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn object_action_history_summary(
    before: &ObjectStatusPayload,
    after: &ObjectStatusPayload,
    label: &str,
    bucket: &str,
    key: &str,
) -> ObjectActionHistoryReferencePayload {
    ObjectActionHistoryReferencePayload {
        label: label.to_string(),
        bucket: bucket.to_string(),
        key: key.to_string(),
        changes: object_status_delta(before, after),
    }
}

fn object_action_history_entry(
    primary_provider: ProviderId,
    input: &ObjectActionInput,
    outcome: &str,
    message: impl Into<String>,
    audit: ObjectActionAuditFields,
    warnings: Vec<String>,
    before_snapshots: &HashMap<String, ObjectStatusPayload>,
    after_snapshots: &HashMap<String, ObjectStatusPayload>,
) -> ObjectActionHistoryEntryPayload {
    let references = object_action_targets(input)
        .into_iter()
        .map(|target| {
            let id = format!("{}/{}", target.bucket, target.key);
            let before = before_snapshots.get(&id);
            let after = after_snapshots.get(&id);
            let changes = match (before, after) {
                (Some(before), Some(after)) => object_action_history_summary(
                    before,
                    after,
                    &target.label,
                    &target.bucket,
                    &target.key,
                ),
                _ => ObjectActionHistoryReferencePayload {
                    label: target.label,
                    bucket: target.bucket,
                    key: target.key,
                    changes: vec!["missing before/after snapshot".to_string()],
                },
            };
            changes
        })
        .collect();

    ObjectActionHistoryEntryPayload {
        executed_at_unix_ms: current_unix_ms(),
        primary_provider: primary_provider.as_str().to_string(),
        action: object_action_name(input).to_string(),
        description: object_action_description(input),
        outcome: outcome.to_string(),
        message: message.into(),
        operator: audit.operator,
        ticket: audit.ticket,
        notes: audit.notes,
        warnings,
        references,
    }
}

fn record_object_action_history(state: &AppState, entry: ObjectActionHistoryEntryPayload) {
    let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
    control_plane.object_action_history.insert(0, entry);
    control_plane
        .object_action_history
        .truncate(state.config.object_action_history_limit);
    if let Err(error) =
        persist_control_plane_state(&state.config.control_plane_file, &control_plane)
    {
        warn!(
            error = %error,
            "failed to persist object action history to control plane"
        );
    }
}

fn clear_object_action_history(state: &AppState) -> Result<(), BlobError> {
    let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
    control_plane.object_action_history.clear();
    persist_control_plane_state(&state.config.control_plane_file, &control_plane)
        .map_err(|error| BlobError::Upstream(error.to_string()))
}

async fn capture_object_status_snapshot(
    state: &AppState,
    bucket: &str,
    key: &str,
) -> ObjectStatusPayload {
    match object_status_payload_for(state, bucket, key).await {
        Ok(payload) => payload,
        Err(error) => ObjectStatusPayload {
            bucket: bucket.to_string(),
            key: key.to_string(),
            primary_provider: runtime_topology(state).primary_provider.as_str(),
            gateway_read_source: None,
            gateway_fallback_from: None,
            gateway_error: Some(error.to_string()),
            provider_states: Vec::new(),
        },
    }
}

async fn capture_object_action_snapshots(
    state: &AppState,
    refs: &[ObjectActionTargetRef],
) -> HashMap<String, ObjectStatusPayload> {
    let mut snapshots = HashMap::with_capacity(refs.len());
    for target in refs {
        let snapshot = capture_object_status_snapshot(state, &target.bucket, &target.key).await;
        snapshots.insert(format!("{}/{}", target.bucket, target.key), snapshot);
    }
    snapshots
}

async fn object_status_payload_for(
    state: &AppState,
    bucket: &str,
    key: &str,
) -> Result<ObjectStatusPayload, BlobError> {
    let topology = runtime_topology(state);
    let mut providers = Vec::with_capacity(1 + topology.sync_targets.len());
    providers.push(topology.primary_provider);
    for provider in &topology.sync_targets {
        if !providers.contains(provider) {
            providers.push(*provider);
        }
    }

    let gateway_resolution = resolve_object_read(state, bucket, key).await;
    let (gateway_read_source, gateway_fallback_from, gateway_error) = match gateway_resolution {
        Ok(resolved) => (
            Some(resolved.source.provider.as_str()),
            resolved
                .source
                .fallback_from
                .map(|provider| provider.as_str()),
            None,
        ),
        Err(error) => (None, None, Some(error.to_string())),
    };

    let mut provider_states = Vec::with_capacity(providers.len());
    for provider in providers {
        let backend = backend_for_provider(state, provider)?;
        let roles = provider_roles(&topology, provider);
        let fallback_order_index = topology
            .fallback_read_order
            .iter()
            .position(|item| *item == provider)
            .map(|index| index + 1);
        let latest_replication_job = if provider == topology.primary_provider {
            None
        } else {
            state
                .metadata_store
                .latest_job_for_object(provider.as_str(), bucket, key)
                .map_err(|error| BlobError::Upstream(error.to_string()))?
        };
        let accepts_replication_put = if provider == topology.primary_provider {
            None
        } else {
            Some(provider_allowed_for_replication(
                state,
                provider,
                ReplicationOperation::Put,
                bucket,
                key,
            )?)
        };
        let fallback_gate = if provider == topology.primary_provider {
            None
        } else {
            Some(load_fallback_gate_for_object(state, provider, bucket, key)?)
        };

        let (exists, object_info, access_error) = match backend.head_object(bucket, key).await {
            Ok(info) => (true, Some(info), None),
            Err(BlobError::NotFound(_)) => (false, None, None),
            Err(error) => (false, None, Some(error.to_string())),
        };

        let readable_via_gateway = if provider == topology.primary_provider {
            exists
        } else {
            exists
                && matches!(fallback_gate, Some(FallbackObjectGate::Allowed))
                && fallback_order_index.is_some()
        };

        provider_states.push(ObjectProviderStatusPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            roles,
            fallback_order_index,
            exists,
            readable_via_gateway,
            accepts_replication_put,
            object_info,
            access_error,
            latest_replication_job,
            fallback_gate: fallback_gate.map(fallback_gate_name),
            fallback_reason: fallback_gate.map(fallback_gate_reason),
        });
    }

    Ok(ObjectStatusPayload {
        bucket: bucket.to_string(),
        key: key.to_string(),
        primary_provider: topology.primary_provider.as_str(),
        gateway_read_source,
        gateway_fallback_from,
        gateway_error,
        provider_states,
    })
}

fn bucket_has_readable_fallback_objects(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
) -> Result<bool, BlobError> {
    if !provider_allowed_for_fallback_bucket(state, provider, bucket) {
        return Ok(false);
    }

    let latest_jobs = state
        .metadata_store
        .latest_jobs_for_bucket(provider.as_str(), bucket)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;

    Ok(latest_jobs.into_iter().any(|job| {
        provider_allowed_for_fallback_object(state, provider, bucket, &job.object.key)
            && matches!(
                fallback_gate_for_job(Some(&job)),
                FallbackObjectGate::Allowed
            )
    }))
}

fn fallback_gate_for_job(job: Option<&ReplicationJob>) -> FallbackObjectGate {
    match job {
        Some(job)
            if matches!(job.operation, ReplicationOperation::Put)
                && matches!(job.status, ReplicationStatus::Completed) =>
        {
            FallbackObjectGate::Allowed
        }
        Some(job) if matches!(job.operation, ReplicationOperation::Delete) => {
            FallbackObjectGate::Deleted
        }
        Some(job) if matches!(job.operation, ReplicationOperation::Put) => {
            FallbackObjectGate::PendingPut
        }
        Some(_) | None => FallbackObjectGate::MissingMetadata,
    }
}

fn fallback_gate_reason(gate: FallbackObjectGate) -> &'static str {
    match gate {
        FallbackObjectGate::Allowed => "replication completed",
        FallbackObjectGate::MissingMetadata => "no completed replication metadata",
        FallbackObjectGate::PendingPut => "newer put is not completed on fallback target",
        FallbackObjectGate::Deleted => "newer delete prevents stale fallback reads",
        FallbackObjectGate::PolicyBlocked => "blocked by onedrive fallback policy",
    }
}

fn fallback_gate_is_deleted(gate: FallbackObjectGate) -> bool {
    matches!(gate, FallbackObjectGate::Deleted)
}

fn latest_jobs_by_key(jobs: Vec<ReplicationJob>) -> HashMap<String, ReplicationJob> {
    jobs.into_iter()
        .map(|job| (job.object.key.clone(), job))
        .collect()
}

async fn list_containers_with_fallback(
    state: &AppState,
) -> Result<ReadResult<Vec<blob_core::ContainerInfo>>, BlobError> {
    let (primary_provider, primary_backend) = current_primary_backend(state)?;
    let mut first_non_not_found = None;

    match primary_backend.list_containers().await {
        Ok(containers) => {
            return Ok(ReadResult {
                source: ReadSource {
                    provider: primary_provider,
                    fallback_from: None,
                },
                value: containers,
            });
        }
        Err(error) => remember_read_error(&mut first_non_not_found, error),
    }

    for backend in ordered_fallback_backends(state) {
        match backend.backend.list_containers().await {
            Ok(containers) => {
                let containers =
                    filter_containers_by_fallback_metadata(state, backend.provider, containers)?;
                if containers.is_empty() {
                    continue;
                }
                info!(
                    provider = backend.provider.as_str(),
                    fallback_from = primary_provider.as_str(),
                    "read request served from fallback backend"
                );
                return Ok(ReadResult {
                    source: ReadSource {
                        provider: backend.provider,
                        fallback_from: Some(primary_provider),
                    },
                    value: containers,
                });
            }
            Err(error) => remember_read_error(&mut first_non_not_found, error),
        }
    }

    Err(first_non_not_found
        .unwrap_or_else(|| BlobError::NotFound("no readable containers were found".to_string())))
}

fn filter_containers_by_fallback_metadata(
    state: &AppState,
    provider: ProviderId,
    containers: Vec<blob_core::ContainerInfo>,
) -> Result<Vec<blob_core::ContainerInfo>, BlobError> {
    let readable = state
        .metadata_store
        .fallback_readable_buckets(provider.as_str())
        .map_err(|error| BlobError::Upstream(error.to_string()))?
        .into_iter()
        .collect::<HashSet<_>>();

    Ok(containers
        .into_iter()
        .filter(|container| {
            readable.contains(&container.name)
                && provider_allowed_for_fallback_bucket(state, provider, &container.name)
        })
        .collect())
}

fn load_fallback_gate_for_object(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
    key: &str,
) -> Result<FallbackObjectGate, BlobError> {
    if !provider_allowed_for_fallback_object(state, provider, bucket, key) {
        return Ok(FallbackObjectGate::PolicyBlocked);
    }

    let latest_job = state
        .metadata_store
        .latest_job_for_object(provider.as_str(), bucket, key)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;
    Ok(fallback_gate_for_job(latest_job.as_ref()))
}

fn filter_objects_by_fallback_metadata(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
    objects: Vec<blob_core::ObjectInfo>,
) -> Result<Vec<blob_core::ObjectInfo>, BlobError> {
    let latest_jobs = state
        .metadata_store
        .latest_jobs_for_bucket(provider.as_str(), bucket)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;
    let latest_by_key = latest_jobs_by_key(latest_jobs);
    let mut skipped = 0usize;

    let filtered = objects
        .into_iter()
        .filter(|object| {
            if !provider_allowed_for_fallback_object(state, provider, bucket, &object.key) {
                skipped += 1;
                return false;
            }

            let gate = fallback_gate_for_job(latest_by_key.get(&object.key));
            let allowed = matches!(gate, FallbackObjectGate::Allowed);
            if !allowed {
                skipped += 1;
            }
            allowed
        })
        .collect::<Vec<_>>();

    if skipped > 0 {
        info!(
            bucket = %bucket,
            provider = provider.as_str(),
            skipped_objects = skipped,
            "filtered fallback object list using replication metadata"
        );
    }

    Ok(filtered)
}

async fn resolve_bucket_read_backend(
    state: &AppState,
    bucket: &str,
) -> Result<ResolvedReadBackend, BlobError> {
    let (primary_provider, primary_backend) = current_primary_backend(state)?;
    let mut first_non_not_found = None;

    match primary_backend.head_container(bucket).await {
        Ok(_) => {
            return Ok(ResolvedReadBackend {
                source: ReadSource {
                    provider: primary_provider,
                    fallback_from: None,
                },
                backend: primary_backend,
            });
        }
        Err(error) => remember_read_error(&mut first_non_not_found, error),
    }

    for backend in ordered_fallback_backends(state) {
        if !bucket_has_readable_fallback_objects(state, backend.provider, bucket)? {
            continue;
        }

        match backend.backend.head_container(bucket).await {
            Ok(_) => {
                info!(
                    bucket = %bucket,
                    provider = backend.provider.as_str(),
                    fallback_from = primary_provider.as_str(),
                    "bucket read resolved through fallback backend"
                );
                return Ok(ResolvedReadBackend {
                    source: ReadSource {
                        provider: backend.provider,
                        fallback_from: Some(primary_provider),
                    },
                    backend: backend.backend,
                });
            }
            Err(error) => remember_read_error(&mut first_non_not_found, error),
        }
    }

    Err(first_non_not_found.unwrap_or_else(|| fallback_not_found(bucket, None)))
}

async fn resolve_object_read(
    state: &AppState,
    bucket: &str,
    key: &str,
) -> Result<ResolvedObjectRead, BlobError> {
    let (primary_provider, primary_backend) = current_primary_backend(state)?;
    let mut first_non_not_found = None;
    let mut deleted_on_fallback = false;

    match primary_backend.head_object(bucket, key).await {
        Ok(object) => {
            return Ok(ResolvedObjectRead {
                source: ReadSource {
                    provider: primary_provider,
                    fallback_from: None,
                },
                backend: primary_backend,
                object,
            });
        }
        Err(error) => remember_read_error(&mut first_non_not_found, error),
    }

    for backend in ordered_fallback_backends(state) {
        let gate = load_fallback_gate_for_object(state, backend.provider, bucket, key)?;
        if !matches!(gate, FallbackObjectGate::Allowed) {
            deleted_on_fallback |= fallback_gate_is_deleted(gate);
            info!(
                bucket = %bucket,
                key = %key,
                provider = backend.provider.as_str(),
                reason = fallback_gate_reason(gate),
                "skipping fallback object head because replication state is not readable"
            );
            continue;
        }

        match backend.backend.head_object(bucket, key).await {
            Ok(object) => {
                info!(
                    bucket = %bucket,
                    key = %key,
                    provider = backend.provider.as_str(),
                    fallback_from = primary_provider.as_str(),
                    "object head served from fallback backend"
                );
                return Ok(ResolvedObjectRead {
                    source: ReadSource {
                        provider: backend.provider,
                        fallback_from: Some(primary_provider),
                    },
                    backend: backend.backend,
                    object,
                });
            }
            Err(error) => remember_read_error(&mut first_non_not_found, error),
        }
    }

    if deleted_on_fallback {
        return Err(fallback_not_found(bucket, Some(key)));
    }

    Err(first_non_not_found.unwrap_or_else(|| fallback_not_found(bucket, Some(key))))
}

async fn head_object_with_fallback(
    state: &AppState,
    bucket: &str,
    key: &str,
) -> Result<ReadResult<blob_core::ObjectInfo>, BlobError> {
    let resolved = resolve_object_read(state, bucket, key).await?;
    Ok(ReadResult {
        source: resolved.source,
        value: resolved.object,
    })
}

fn spawn_replication_workers(state: AppState, workers: usize) {
    for worker_id in 0..workers {
        tokio::spawn(replication_worker_loop(worker_id, state.clone()));
    }
}

async fn replication_worker_loop(worker_id: usize, state: AppState) {
    loop {
        if !process_next_replication_job(worker_id, &state).await {
            sleep(Duration::from_millis(250)).await;
            continue;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReplicationFailureKind {
    Retryable,
    Permanent,
}

#[derive(Debug, Clone)]
struct ReplicationFailure {
    kind: ReplicationFailureKind,
    message: String,
}

impl ReplicationFailure {
    fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: ReplicationFailureKind::Retryable,
            message: message.into(),
        }
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self {
            kind: ReplicationFailureKind::Permanent,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ReplicationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

fn replication_retry_policy(config: &AppConfig) -> ReplicationRetryPolicy {
    ReplicationRetryPolicy {
        max_attempts: config.replication_max_attempts.max(1),
        base_delay_ms: config.replication_base_retry_delay_ms,
        max_delay_ms: config.replication_max_retry_delay_ms,
    }
}

async fn process_next_replication_job(worker_id: usize, state: &AppState) -> bool {
    let Some(job) = state.replication.pop_next_ready() else {
        return false;
    };

    let attempts = job.attempts.saturating_add(1);
    match process_replication_job(state, &job).await {
        Ok(()) => {
            let mut completed_job = job.clone();
            completed_job.attempts = attempts;
            completed_job.last_error = None;
            state.replication.record_completed(completed_job);
            match state.metadata_store.mark_job_status(
                job.job_id,
                replication_engine::ReplicationStatus::Completed,
                attempts,
                None,
            ) {
                Ok(prune_result) => {
                    if prune_result.total_deleted() > 0 {
                        info!(
                            worker_id,
                            job_id = job.job_id,
                            deleted_completed_jobs = prune_result.deleted_completed_jobs,
                            deleted_failed_jobs = prune_result.deleted_failed_jobs,
                            "pruned metadata history after completed replication job"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        worker_id,
                        job_id = job.job_id,
                        error = %error,
                        "failed to persist completed replication job state"
                    );
                }
            }
            info!(
                worker_id,
                job_id = job.job_id,
                target = %job.target,
                bucket = %job.object.bucket,
                key = %job.object.key,
                attempts,
                "replication job completed"
            );
        }
        Err(error) => {
            let retry_policy = replication_retry_policy(&state.config);
            if error.kind == ReplicationFailureKind::Retryable
                && attempts < retry_policy.max_attempts
            {
                let retry_at = current_unix_ms() as u128
                    + u128::from(retry_policy.delay_for_attempt(attempts));
                let mut retry_job = job.clone();
                retry_job.status = ReplicationStatus::RetryScheduled;
                retry_job.attempts = attempts;
                retry_job.last_error = Some(error.to_string());
                retry_job.next_attempt_at_unix_ms = Some(retry_at);
                state.replication.schedule_retry(retry_job.clone());
                if let Err(store_error) = state.metadata_store.save_job(&retry_job) {
                    warn!(
                        worker_id,
                        job_id = retry_job.job_id,
                        error = %store_error,
                        "failed to persist scheduled replication retry"
                    );
                }
                warn!(
                    worker_id,
                    job_id = retry_job.job_id,
                    target = %retry_job.target,
                    bucket = %retry_job.object.bucket,
                    key = %retry_job.object.key,
                    attempts,
                    next_attempt_at_unix_ms = retry_at,
                    error = %error,
                    "replication job scheduled for retry"
                );
            } else {
                let error_message = error.to_string();
                let mut failed_job = job.clone();
                failed_job.attempts = attempts;
                state
                    .replication
                    .record_failed(failed_job, error_message.clone());
                match state.metadata_store.mark_job_status(
                    job.job_id,
                    replication_engine::ReplicationStatus::Failed,
                    attempts,
                    Some(&error_message),
                ) {
                    Ok(prune_result) => {
                        if prune_result.total_deleted() > 0 {
                            info!(
                                worker_id,
                                job_id = job.job_id,
                                deleted_completed_jobs = prune_result.deleted_completed_jobs,
                                deleted_failed_jobs = prune_result.deleted_failed_jobs,
                                "pruned metadata history after failed replication job"
                            );
                        }
                    }
                    Err(store_error) => {
                        warn!(
                            worker_id,
                            job_id = job.job_id,
                            error = %store_error,
                            "failed to persist failed replication job state"
                        );
                    }
                }
                warn!(
                    worker_id,
                    job_id = job.job_id,
                    target = %job.target,
                    bucket = %job.object.bucket,
                    key = %job.object.key,
                    attempts,
                    error = %error,
                    "replication job failed"
                );
            }
        }
    }

    true
}

async fn process_replication_job(
    state: &AppState,
    job: &replication_engine::ReplicationJob,
) -> Result<(), ReplicationFailure> {
    ensure_job_is_current(state, job)?;
    let backend =
        backend_for_provider_name(state, &job.target).map_err(ReplicationFailure::permanent)?;

    match job.operation {
        replication_engine::ReplicationOperation::Put => {
            if !backend.capabilities().write {
                return Err(ReplicationFailure::permanent(format!(
                    "target {} does not support write",
                    job.target
                )));
            }
            ensure_replication_object_within_in_memory_limit(&state.config, job)
                .map_err(ReplicationFailure::permanent)?;
            let source_provider = job.source_provider.as_deref().ok_or_else(|| {
                ReplicationFailure::permanent(format!(
                    "replication job {} is missing source_provider",
                    job.job_id
                ))
            })?;
            let source_backend = backend_for_provider_name(state, source_provider)
                .map_err(ReplicationFailure::permanent)?;

            let source_object = source_backend
                .get_object(&job.object.bucket, &job.object.key)
                .await
                .map_err(replication_source_read_failure)?;

            backend
                .put_object(PutObjectRequest {
                    container: job.object.bucket.clone(),
                    key: job.object.key.clone(),
                    body: source_object.body,
                    size: Some(source_object.info.size),
                    content_type: source_object.info.content_type,
                })
                .await
                .map_err(replication_target_write_failure)?;
        }
        replication_engine::ReplicationOperation::Delete => {
            if !backend.capabilities().delete {
                return Err(ReplicationFailure::permanent(format!(
                    "target {} does not support delete",
                    job.target
                )));
            }

            match backend
                .delete_object(&job.object.bucket, &job.object.key)
                .await
            {
                Ok(()) | Err(BlobError::NotFound(_)) => {}
                Err(error) => return Err(replication_target_delete_failure(error)),
            }
        }
    }

    Ok(())
}

fn ensure_job_is_current(
    state: &AppState,
    job: &replication_engine::ReplicationJob,
) -> Result<(), ReplicationFailure> {
    let latest = state
        .metadata_store
        .latest_job_for_object(&job.target, &job.object.bucket, &job.object.key)
        .map_err(|error| {
            ReplicationFailure::retryable(format!(
                "failed to inspect latest replication job state: {error}"
            ))
        })?;

    if let Some(latest) = latest {
        if latest.job_id > job.job_id {
            return Err(ReplicationFailure::permanent(format!(
                "replication job {} was superseded by newer job {} for {}/{} on {}",
                job.job_id, latest.job_id, job.object.bucket, job.object.key, job.target
            )));
        }
    }

    Ok(())
}

fn replication_source_read_failure(error: BlobError) -> ReplicationFailure {
    match error {
        BlobError::Upstream(message) => {
            ReplicationFailure::retryable(format!("failed to read source object: {message}"))
        }
        BlobError::BodyStream(message) => {
            ReplicationFailure::retryable(format!("failed to read source object stream: {message}"))
        }
        BlobError::NotFound(message) => {
            ReplicationFailure::permanent(format!("failed to read source object: {message}"))
        }
        BlobError::Configuration(message) => {
            ReplicationFailure::permanent(format!("failed to read source object: {message}"))
        }
        BlobError::NotImplemented(message) => {
            ReplicationFailure::permanent(format!("failed to read source object: {message}"))
        }
    }
}

fn replication_target_write_failure(error: BlobError) -> ReplicationFailure {
    match error {
        BlobError::Upstream(message)
        | BlobError::NotFound(message)
        | BlobError::BodyStream(message) => {
            ReplicationFailure::retryable(format!("failed to write target object: {message}"))
        }
        BlobError::Configuration(message) => {
            ReplicationFailure::permanent(format!("failed to write target object: {message}"))
        }
        BlobError::NotImplemented(message) => {
            ReplicationFailure::permanent(format!("failed to write target object: {message}"))
        }
    }
}

fn replication_target_delete_failure(error: BlobError) -> ReplicationFailure {
    match error {
        BlobError::Upstream(message) | BlobError::BodyStream(message) => {
            ReplicationFailure::retryable(format!("failed to delete target object: {message}"))
        }
        BlobError::Configuration(message) => {
            ReplicationFailure::permanent(format!("failed to delete target object: {message}"))
        }
        BlobError::NotImplemented(message) => {
            ReplicationFailure::permanent(format!("failed to delete target object: {message}"))
        }
        BlobError::NotFound(message) => {
            ReplicationFailure::permanent(format!("failed to delete target object: {message}"))
        }
    }
}

fn resolve_token_source(prefix: &str) -> TokenSource {
    let file_key = format!("{prefix}_TOKEN_FILE");
    let token_key = format!("{prefix}_TOKEN");

    if let Ok(path) = env::var(&file_key) {
        if !path.trim().is_empty() {
            return TokenSource::File { path };
        }
    }

    if let Ok(token) = env::var(&token_key) {
        return TokenSource::Static { bearer: token };
    }

    TokenSource::EnvVar { key: token_key }
}

fn load_control_plane_state(
    path: &str,
    default_state: ControlPlaneState,
    onedrive_enabled: bool,
) -> Result<ControlPlaneState> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let mut state: ControlPlaneState =
                serde_json::from_str(&raw).context("invalid control plane JSON")?;
            state.topology = TopologyPolicy::from_input(TopologyInput {
                primary_provider: state.topology.primary_provider,
                sync_targets: state.topology.sync_targets.clone(),
                fallback_read_order: state.topology.fallback_read_order.clone(),
                onedrive_enabled,
                replication_mode: state.topology.replication_mode,
            })
            .context("invalid saved topology in control plane file")?;
            state.onedrive_policy.memory_buckets =
                normalize_bucket_list(&state.onedrive_policy.memory_buckets);
            state.onedrive_policy.memory_prefixes =
                normalize_prefix_list(&state.onedrive_policy.memory_prefixes);
            state.auth_capture_policy.broker_url =
                normalize_secret_field(state.auth_capture_policy.broker_url);
            state.auth_capture_policy.cdp_endpoint_url =
                normalize_secret_field(state.auth_capture_policy.cdp_endpoint_url);
            state.auth_capture_policy.cdp_target_selector =
                normalize_secret_field(state.auth_capture_policy.cdp_target_selector);
            state.auth_capture_policy.cdp_target_timeout_ms = state
                .auth_capture_policy
                .cdp_target_timeout_ms
                .filter(|value| *value > 0);
            state.auth_capture_policy.llm_endpoint =
                normalize_secret_field(state.auth_capture_policy.llm_endpoint);
            state.auth_capture_policy.llm_model_id =
                normalize_secret_field(state.auth_capture_policy.llm_model_id);
            state.auth_capture_policy.llm_api_key =
                normalize_secret_field(state.auth_capture_policy.llm_api_key);
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_state),
        Err(error) => Err(anyhow::Error::new(error).context("failed to read control plane file")),
    }
}

fn persist_control_plane_state(path: &str, state: &ControlPlaneState) -> Result<()> {
    if let Some(parent) = FsPath::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create control plane dir {parent:?}"))?;
        }
    }

    let body =
        serde_json::to_string_pretty(state).context("failed to encode control plane JSON")?;
    fs::write(path, body).with_context(|| format!("failed to write control plane file {path}"))?;
    Ok(())
}

fn normalize_secret_field(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn provider_credentials_path(config: &AppConfig, provider: ProviderId) -> PathBuf {
    FsPath::new(&config.credentials_dir).join(format!("{}.json", provider.as_str()))
}

fn provider_capability_catalog_path(
    config: &AppConfig,
    provider: ProviderId,
    variant: &str,
) -> PathBuf {
    FsPath::new(&config.provider_capability_catalog_dir).join(format!(
        "{}-{}.json",
        provider.as_str(),
        variant
    ))
}

fn load_provider_credential_record(
    config: &AppConfig,
    provider: ProviderId,
) -> Result<ProviderCredentialRecord, BlobError> {
    if provider == ProviderId::Stub {
        return Ok(ProviderCredentialRecord::default());
    }

    let path = provider_credentials_path(config, provider);
    match fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<ProviderCredentialRecord>(&raw)
            .map(|record| record.normalize())
            .map_err(|error| {
                BlobError::Configuration(format!(
                    "failed to parse provider credential file {}: {error}",
                    path.display()
                ))
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(ProviderCredentialRecord::default())
        }
        Err(error) => Err(BlobError::Configuration(format!(
            "failed to read provider credential file {}: {error}",
            path.display()
        ))),
    }
}

fn load_provider_credential_record_or_default(
    config: &AppConfig,
    provider: ProviderId,
) -> ProviderCredentialRecord {
    match load_provider_credential_record(config, provider) {
        Ok(record) => record,
        Err(error) => {
            warn!(
                provider = provider.as_str(),
                error = %error,
                "failed to load provider credential file, falling back to env defaults"
            );
            ProviderCredentialRecord::default()
        }
    }
}

fn persist_provider_credential_record(
    config: &AppConfig,
    provider: ProviderId,
    record: &ProviderCredentialRecord,
) -> Result<(), BlobError> {
    if provider == ProviderId::Stub {
        return Err(BlobError::Configuration(
            "stub provider does not support credential storage".to_string(),
        ));
    }

    let path = provider_credentials_path(config, provider);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                BlobError::Configuration(format!(
                    "failed to create provider credential dir {}: {error}",
                    parent.display()
                ))
            })?;
        }
    }

    if record.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(BlobError::Configuration(format!(
                    "failed to remove provider credential file {}: {error}",
                    path.display()
                )));
            }
        }
        return Ok(());
    }

    let body = serde_json::to_string_pretty(record).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to encode provider credential file {}: {error}",
            path.display()
        ))
    })?;
    fs::write(&path, body).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to write provider credential file {}: {error}",
            path.display()
        ))
    })
}

fn override_token_source(base: TokenSource, override_token: &Option<String>) -> TokenSource {
    match override_token {
        Some(token) => TokenSource::Static {
            bearer: token.clone(),
        },
        None => base,
    }
}

fn effective_onedrive_config_from_app(config: &AppConfig) -> OneDriveConfig {
    let credentials = load_provider_credential_record_or_default(config, ProviderId::Onedrive);
    let mut resolved = config.onedrive.clone();
    if let Some(client_id) = credentials.client_id {
        resolved.client_id = Some(client_id);
    }
    if let Some(tenant) = credentials.tenant {
        resolved.tenant = tenant;
    }
    if let Some(drive_id) = credentials.drive_id {
        resolved.drive_id = Some(drive_id);
    }
    if let Some(redirect_url) = credentials.redirect_url {
        resolved.redirect_url = Some(redirect_url);
    }
    if let Some(token) = credentials.token {
        resolved.token_source = TokenSource::Static { bearer: token };
        // Manual token injection should take precedence over any stale OAuth session file.
        resolved.session_file = None;
    }
    resolved
}

fn default_provider_credential_payload(
    config: &AppConfig,
    provider: ProviderId,
) -> ProviderCredentialPayload {
    let storage_path = provider_credentials_path(config, provider)
        .display()
        .to_string();
    match provider {
        ProviderId::Unicom => ProviderCredentialPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            storage_path,
            token: resolve_token_source("CCBG_UNICOM").load().ok(),
            browser_id: None,
            cookie_header: env_opt_or_file(
                "CCBG_UNICOM_COOKIE_HEADER",
                "CCBG_UNICOM_COOKIE_HEADER_FILE",
            ),
            family_id: env_opt("CCBG_UNICOM_FAMILY_ID"),
            root_folder_id: None,
            client_id: None,
            tenant: None,
            drive_id: None,
            redirect_url: None,
            session_file: None,
        },
        ProviderId::Telecom => ProviderCredentialPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            storage_path,
            token: resolve_token_source("CCBG_TELECOM").load().ok(),
            browser_id: env_opt_or_file("CCBG_TELECOM_BROWSER_ID", "CCBG_TELECOM_BROWSER_ID_FILE"),
            cookie_header: env_opt_or_file(
                "CCBG_TELECOM_COOKIE_HEADER",
                "CCBG_TELECOM_COOKIE_HEADER_FILE",
            ),
            family_id: None,
            root_folder_id: env_opt("CCBG_TELECOM_ROOT_FOLDER_ID"),
            client_id: None,
            tenant: None,
            drive_id: None,
            redirect_url: None,
            session_file: None,
        },
        ProviderId::Mobile => ProviderCredentialPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            storage_path,
            token: resolve_token_source("CCBG_MOBILE").load().ok(),
            browser_id: None,
            cookie_header: env_opt("CCBG_MOBILE_COOKIE_HEADER"),
            family_id: None,
            root_folder_id: None,
            client_id: None,
            tenant: None,
            drive_id: None,
            redirect_url: None,
            session_file: None,
        },
        ProviderId::Onedrive => ProviderCredentialPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            storage_path,
            token: config.onedrive.token_source.load().ok(),
            browser_id: None,
            cookie_header: None,
            family_id: None,
            root_folder_id: None,
            client_id: config.onedrive.client_id.clone(),
            tenant: Some(config.onedrive.tenant.clone()),
            drive_id: config.onedrive.drive_id.clone(),
            redirect_url: config.onedrive.redirect_url.clone(),
            session_file: config.onedrive.session_file.clone(),
        },
        ProviderId::Stub => ProviderCredentialPayload {
            provider: provider.as_str(),
            label: provider_label(provider),
            storage_path,
            token: None,
            browser_id: None,
            cookie_header: None,
            family_id: None,
            root_folder_id: None,
            client_id: None,
            tenant: None,
            drive_id: None,
            redirect_url: None,
            session_file: None,
        },
    }
}

fn current_provider_credential_payload(
    state: &AppState,
    provider: ProviderId,
) -> Result<ProviderCredentialPayload, BlobError> {
    if provider == ProviderId::Stub {
        return Err(BlobError::Configuration(
            "stub provider does not support credential storage".to_string(),
        ));
    }

    let stored = load_provider_credential_record(&state.config, provider)?;
    let mut payload = default_provider_credential_payload(&state.config, provider);
    if stored.token.is_some() {
        payload.token = stored.token;
    }
    if stored.browser_id.is_some() {
        payload.browser_id = stored.browser_id;
    }
    if stored.cookie_header.is_some() {
        payload.cookie_header = stored.cookie_header;
    }
    if stored.family_id.is_some() {
        payload.family_id = stored.family_id;
    }
    if stored.root_folder_id.is_some() {
        payload.root_folder_id = stored.root_folder_id;
    }
    if stored.client_id.is_some() {
        payload.client_id = stored.client_id;
    }
    if stored.tenant.is_some() {
        payload.tenant = stored.tenant;
    }
    if stored.drive_id.is_some() {
        payload.drive_id = stored.drive_id;
    }
    if stored.redirect_url.is_some() {
        payload.redirect_url = stored.redirect_url;
    }
    Ok(payload)
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn env_opt_or_file(key: &str, file_key: &str) -> Option<String> {
    if let Some(value) = env_opt(key) {
        return Some(value);
    }

    let path = env_opt(file_key)?;
    let contents = fs::read_to_string(&path).ok()?;
    let trimmed = contents.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn env_outbound_ip_family(
    key: &str,
    default: OutboundIpFamily,
) -> Result<OutboundIpFamily, BlobError> {
    match env::var(key) {
        Ok(raw) => OutboundIpFamily::parse(&raw)
            .map_err(|error| BlobError::Configuration(format!("invalid {key}: {error}"))),
        Err(_) => Ok(default),
    }
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .ok()
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

fn env_csv_list(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn normalize_bucket_list(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let bucket = value.trim().to_ascii_lowercase();
        if !bucket.is_empty() && seen.insert(bucket.clone()) {
            normalized.push(bucket);
        }
    }
    normalized
}

fn normalize_prefix_list(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let prefix = value
            .trim()
            .trim_start_matches('/')
            .trim_end_matches('/')
            .to_string();
        if !prefix.is_empty() && seen.insert(prefix.clone()) {
            normalized.push(prefix);
        }
    }
    normalized
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

async fn index(State(state): State<AppState>) -> Json<IndexPayload> {
    let snapshot = state.replication.snapshot();
    let topology = runtime_topology(&state);
    let backend_name = backend_for_provider(&state, topology.primary_provider)
        .map(|backend| backend.name())
        .unwrap_or("unknown");

    Json(IndexPayload {
        service: "carrier-cloud-blob-gateway",
        backend: backend_name,
        primary_provider: topology.primary_provider_name(),
        sync_targets: topology.sync_target_names(),
        fallback_read_order: topology.fallback_read_order_names(),
        replication: ReplicationQueueSummary {
            pending_jobs: snapshot.pending_count,
            recent_jobs: snapshot.recent_count,
        },
        endpoints: vec![
            "/",
            "/{bucket}",
            "/{bucket}/{key}",
            "/healthz",
            "/__ccbg",
            "/__ccbg/providers",
            "/__ccbg/replication",
            "/v1/containers",
            "/v1/objects",
        ],
    })
}

async fn list_provider_health(
    State(state): State<AppState>,
) -> Result<Json<Vec<BackendPayload>>, ApiError> {
    Ok(Json(provider_health_payloads(&state).await?))
}

async fn replication_snapshot(
    State(state): State<AppState>,
) -> Result<Json<ReplicationStatePayload>, ApiError> {
    Ok(Json(replication_state_payload(&state)?))
}

async fn healthz(
    State(state): State<AppState>,
) -> Result<Json<blob_core::ServiceHealth>, ApiError> {
    let (_, backend) = current_primary_backend(&state)?;
    Ok(Json(backend.health().await?))
}

#[derive(Debug, Serialize)]
struct ExtendedHealthPayload {
    status: &'static str,
    ready: bool,
    checked_at_unix_ms: u64,
    runtime: RuntimeStatusPayload,
    monitoring: MonitoringSummaryPayload,
    alerts: Vec<AdminAlertPayload>,
}

async fn metrics_healthz(
    State(state): State<AppState>,
) -> Result<Json<ExtendedHealthPayload>, ApiError> {
    let replication_state = replication_state_payload(&state)?;
    let provider_health = provider_health_payloads(&state).await?;
    let onedrive_auth = read_onedrive_auth_status(&state);
    let alerts = build_admin_alerts(&state, &provider_health, &replication_state, &onedrive_auth);
    let object_action_history = control_plane_snapshot(&state).object_action_history;
    let monitoring = monitoring_summary_payload(
        &provider_health,
        &replication_state,
        &object_action_history,
        &alerts,
    );
    let ready = provider_health.iter().all(|provider| {
        provider.role != "primary"
            || !matches!(provider.health.status, blob_core::HealthStatus::Unavailable)
    });

    Ok(Json(ExtendedHealthPayload {
        status: if ready { "ok" } else { "degraded" },
        ready,
        checked_at_unix_ms: current_unix_ms(),
        runtime: runtime_status_payload(&state),
        monitoring,
        alerts,
    }))
}

async fn metrics_readyz(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    let provider_health = provider_health_payloads(&state).await?;
    let primary_ready = provider_health.iter().all(|provider| {
        provider.role != "primary"
            || !matches!(provider.health.status, blob_core::HealthStatus::Unavailable)
    });

    Ok(if primary_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    })
}

async fn metrics_prometheus(State(state): State<AppState>) -> Result<Response, ApiError> {
    let runtime = runtime_status_payload(&state);
    let replication_state = replication_state_payload(&state)?;
    let provider_health = provider_health_payloads(&state).await?;
    let onedrive_auth = read_onedrive_auth_status(&state);
    let alerts = build_admin_alerts(&state, &provider_health, &replication_state, &onedrive_auth);
    let object_action_history = control_plane_snapshot(&state).object_action_history;
    let monitoring = monitoring_summary_payload(
        &provider_health,
        &replication_state,
        &object_action_history,
        &alerts,
    );

    let mut lines = Vec::new();
    lines.push("# HELP ccbg_up Carrier Cloud Blob Gateway process health indicator.".to_string());
    lines.push("# TYPE ccbg_up gauge".to_string());
    lines.push("ccbg_up 1".to_string());
    lines.push("# HELP ccbg_uptime_ms Gateway uptime in milliseconds.".to_string());
    lines.push("# TYPE ccbg_uptime_ms gauge".to_string());
    lines.push(format!("ccbg_uptime_ms {}", runtime.uptime_ms));
    lines.push("# HELP ccbg_admin_alerts_open Current open alert count.".to_string());
    lines.push("# TYPE ccbg_admin_alerts_open gauge".to_string());
    lines.push(format!(
        "ccbg_admin_alerts_open {}",
        monitoring.open_alert_count
    ));
    lines.push(
        "# HELP ccbg_data_plane_concurrency_configured Configured max in-flight data-plane requests."
            .to_string(),
    );
    lines.push("# TYPE ccbg_data_plane_concurrency_configured gauge".to_string());
    lines.push(format!(
        "ccbg_data_plane_concurrency_configured {}",
        runtime.data_plane_max_in_flight
    ));
    lines.push(
        "# HELP ccbg_data_plane_concurrency_available Currently available data-plane permits."
            .to_string(),
    );
    lines.push("# TYPE ccbg_data_plane_concurrency_available gauge".to_string());
    lines.push(format!(
        "ccbg_data_plane_concurrency_available {}",
        state.data_plane_concurrency.semaphore.available_permits()
    ));
    lines.push("# HELP ccbg_provider_health Provider health status by role and provider (healthy=2,degraded=1,unavailable=0).".to_string());
    lines.push("# TYPE ccbg_provider_health gauge".to_string());
    for provider in &provider_health {
        let value = match provider.health.status {
            blob_core::HealthStatus::Healthy => 2,
            blob_core::HealthStatus::Degraded => 1,
            blob_core::HealthStatus::Unavailable => 0,
        };
        lines.push(format!(
            "ccbg_provider_health{{provider=\"{}\",role=\"{}\"}} {}",
            provider.provider, provider.role, value
        ));
    }
    lines.push("# HELP ccbg_replication_jobs Replication jobs by persisted status.".to_string());
    lines.push("# TYPE ccbg_replication_jobs gauge".to_string());
    lines.push(format!(
        "ccbg_replication_jobs{{status=\"pending\"}} {}",
        monitoring.replication.pending_jobs
    ));
    lines.push(format!(
        "ccbg_replication_jobs{{status=\"retry_scheduled\"}} {}",
        monitoring.replication.retry_scheduled_jobs
    ));
    lines.push(format!(
        "ccbg_replication_jobs{{status=\"failed\"}} {}",
        monitoring.replication.failed_jobs
    ));
    lines.push(format!(
        "ccbg_replication_jobs{{status=\"completed\"}} {}",
        monitoring.replication.completed_jobs
    ));
    lines.push("# HELP ccbg_object_action_entries Object action history counts.".to_string());
    lines.push("# TYPE ccbg_object_action_entries gauge".to_string());
    lines.push(format!(
        "ccbg_object_action_entries{{outcome=\"total\"}} {}",
        monitoring.object_actions.total_entries
    ));
    lines.push(format!(
        "ccbg_object_action_entries{{outcome=\"successful\"}} {}",
        monitoring.object_actions.successful_entries
    ));
    lines.push(format!(
        "ccbg_object_action_entries{{outcome=\"failed\"}} {}",
        monitoring.object_actions.failed_entries
    ));
    lines.push(
        "# HELP ccbg_object_action_unique_operators Unique operators in retained action history."
            .to_string(),
    );
    lines.push("# TYPE ccbg_object_action_unique_operators gauge".to_string());
    lines.push(format!(
        "ccbg_object_action_unique_operators {}",
        monitoring.object_actions.unique_operators
    ));
    lines.push(
        "# HELP ccbg_replication_target_jobs Replication job counters per target.".to_string(),
    );
    lines.push("# TYPE ccbg_replication_target_jobs gauge".to_string());
    for target in &replication_state.target_statuses {
        lines.push(format!(
            "ccbg_replication_target_jobs{{target=\"{}\",status=\"queued\"}} {}",
            target.provider, target.queued_count
        ));
        lines.push(format!(
            "ccbg_replication_target_jobs{{target=\"{}\",status=\"pending\"}} {}",
            target.provider, target.pending_count
        ));
        lines.push(format!(
            "ccbg_replication_target_jobs{{target=\"{}\",status=\"retry_scheduled\"}} {}",
            target.provider, target.retry_scheduled_count
        ));
        lines.push(format!(
            "ccbg_replication_target_jobs{{target=\"{}\",status=\"completed\"}} {}",
            target.provider, target.completed_count
        ));
        lines.push(format!(
            "ccbg_replication_target_jobs{{target=\"{}\",status=\"failed\"}} {}",
            target.provider, target.failed_count
        ));
    }

    Ok((
        [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        lines.join("\n") + "\n",
    )
        .into_response())
}

async fn list_containers(
    State(state): State<AppState>,
) -> Result<Json<Vec<blob_core::ContainerInfo>>, DataPlaneApiError> {
    let _permit =
        try_acquire_data_plane_permit(&state).map_err(DataPlaneApiError::from_s3_error)?;
    Ok(Json(list_containers_with_fallback(&state).await?.value))
}

async fn list_objects(
    State(state): State<AppState>,
    Query(query): Query<ObjectsQuery>,
) -> Result<Json<Vec<blob_core::ObjectInfo>>, DataPlaneApiError> {
    let _permit =
        try_acquire_data_plane_permit(&state).map_err(DataPlaneApiError::from_s3_error)?;
    let (_, backend) = current_primary_backend(&state)?;
    let request = ListObjectsRequest {
        container: query.container,
        prefix: query.prefix,
        limit: query.limit,
    };

    Ok(Json(backend.list_objects(request).await?))
}

async fn list_buckets(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;

    let read = list_containers_with_fallback(&state)
        .await
        .map_err(map_backend_error_to_s3)?;
    let containers = read.value;

    let mut body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str(&format!(
        "<ListAllMyBucketsResult xmlns=\"{S3_NS}\"><Owner><ID>ccbg</ID><DisplayName>ccbg</DisplayName></Owner><Buckets>"
    ));

    for bucket in containers {
        body.push_str(&format!(
            "<Bucket><Name>{}</Name><CreationDate>{}</CreationDate></Bucket>",
            xml_escape(&bucket.name),
            DEFAULT_TIMESTAMP
        ));
    }

    body.push_str("</Buckets></ListAllMyBucketsResult>");
    let mut response = xml_response(StatusCode::OK, body);
    apply_read_source_headers(response.headers_mut(), read.source);
    Ok(response)
}

async fn head_bucket(
    State(state): State<AppState>,
    Path(bucket): Path<String>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;

    let read_backend = resolve_bucket_read_backend(&state, &bucket)
        .await
        .map_err(|error| map_bucket_error(error, &bucket))?;

    let mut response = StatusCode::OK.into_response();
    response.headers_mut().insert(
        "x-amz-bucket-region",
        HeaderValue::from_str(&state.config.s3_region)
            .expect("configured region should be a valid header value"),
    );
    apply_read_source_headers(response.headers_mut(), read_backend.source);
    Ok(response)
}

async fn list_objects_v2(
    State(state): State<AppState>,
    Path(bucket): Path<String>,
    Query(query): Query<ListObjectsV2Query>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;

    if let Some(list_type) = &query.list_type {
        if list_type != "2" {
            return Err(S3Error::not_implemented(
                "Only ListObjectsV2 is supported in the current S3 subset.",
            ));
        }
    }

    if query.continuation_token.is_some() {
        return Err(S3Error::not_implemented(
            "continuation-token is not supported yet.",
        ));
    }

    let read_backend = resolve_bucket_read_backend(&state, &bucket)
        .await
        .map_err(|error| map_bucket_error(error, &bucket))?;

    let max_keys = query.max_keys.unwrap_or(1000).min(1000);
    let backend_limit = if read_backend.source.fallback_from.is_some() {
        None
    } else {
        Some(max_keys)
    };
    let mut objects = read_backend
        .backend
        .list_objects(ListObjectsRequest {
            container: Some(bucket.clone()),
            prefix: query.prefix.clone(),
            limit: backend_limit,
        })
        .await
        .map_err(map_backend_error_to_s3)?;

    if read_backend.source.fallback_from.is_some() {
        objects = filter_objects_by_fallback_metadata(
            &state,
            read_backend.source.provider,
            &bucket,
            objects,
        )
        .map_err(map_backend_error_to_s3)?;
        objects.truncate(max_keys);
    }

    let mut body = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    body.push_str(&format!(
        "<ListBucketResult xmlns=\"{S3_NS}\"><Name>{}</Name><Prefix>{}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{}</MaxKeys><IsTruncated>false</IsTruncated>",
        xml_escape(&bucket),
        xml_escape(query.prefix.as_deref().unwrap_or("")),
        objects.len(),
        max_keys
    ));

    if let Some(delimiter) = &query.delimiter {
        body.push_str(&format!("<Delimiter>{}</Delimiter>", xml_escape(delimiter)));
    }

    for object in objects {
        body.push_str(&format!(
            "<Contents><Key>{}</Key><LastModified>{}</LastModified><ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
            xml_escape(&object.key),
            xml_escape(object.last_modified.as_deref().unwrap_or(DEFAULT_TIMESTAMP)),
            xml_escape(&quoted_etag(object.etag.as_deref())),
            object.size,
        ));
    }

    body.push_str("</ListBucketResult>");
    let mut response = xml_response(StatusCode::OK, body);
    apply_read_source_headers(response.headers_mut(), read_backend.source);
    Ok(response)
}

async fn head_object(
    State(state): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;

    let read = head_object_with_fallback(&state, &bucket, &key)
        .await
        .map_err(|error| map_object_error(error, &bucket, &key))?;
    let object = read.value;

    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&object.size.to_string()).expect("content length should be valid"),
    );
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(
            object
                .content_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
        )
        .expect("content type should be valid"),
    );
    if let Some(etag) = object.etag.as_deref() {
        headers.insert(
            ETAG,
            HeaderValue::from_str(&quoted_etag(Some(etag))).expect("etag should be valid"),
        );
    }
    if let Some(last_modified) = object.last_modified.as_deref() {
        headers.insert(
            LAST_MODIFIED,
            HeaderValue::from_str(last_modified).expect("last-modified should be valid"),
        );
    }
    apply_read_source_headers(&mut headers, read.source);

    Ok((StatusCode::OK, headers).into_response())
}

async fn get_object(
    State(state): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;

    let resolved = resolve_object_read(&state, &bucket, &key)
        .await
        .map_err(|error| map_object_error(error, &bucket, &key))?;
    ensure_object_within_in_memory_limit(&state.config, resolved.object.size)?;
    let object = resolved
        .backend
        .get_object(&bucket, &key)
        .await
        .map_err(|error| map_object_error(error, &bucket, &key))?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&object.info.size.to_string())
            .expect("content length should be valid"),
    );
    response_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(
            object
                .info
                .content_type
                .as_deref()
                .unwrap_or("application/octet-stream"),
        )
        .expect("content type should be valid"),
    );
    if let Some(etag) = object.info.etag.as_deref() {
        response_headers.insert(
            ETAG,
            HeaderValue::from_str(&quoted_etag(Some(etag))).expect("etag should be valid"),
        );
    }
    apply_read_source_headers(&mut response_headers, resolved.source);

    // Keep the permit alive until the response body stream is fully consumed.
    let body_stream = futures_util::stream::unfold(
        (permit, object.body.into_stream()),
        |(permit, mut inner)| async move {
            inner
                .next()
                .await
                .map(|item| (item.map_err(std::io::Error::other), (permit, inner)))
        },
    );
    Ok((
        StatusCode::OK,
        response_headers,
        Body::from_stream(body_stream),
    )
        .into_response())
}

async fn put_object(
    State(state): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    let content_length = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let payload_hash = headers
        .get("x-amz-content-sha256")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| S3Error::access_denied("Missing x-amz-content-sha256 header."))?
        .to_string();
    let (primary_provider, primary_backend) =
        current_primary_backend(&state).map_err(map_backend_error_to_s3)?;

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);

    if payload_hash == "UNSIGNED-PAYLOAD" {
        authorize_s3(&state.config, &method, &uri, &headers, None)?;
        let body_len = content_length.ok_or_else(|| {
            S3Error::access_denied(
                "UNSIGNED-PAYLOAD uploads must include a valid Content-Length header.",
            )
        })?;
        ensure_object_within_in_memory_limit(&state.config, body_len)?;

        let result = primary_backend
            .put_object(PutObjectRequest {
                container: bucket.clone(),
                key: key.clone(),
                body: ObjectBody::from_stream(
                    body.into_data_stream()
                        .map_err(|error| BlobError::BodyStream(error.to_string())),
                ),
                size: Some(body_len),
                content_type: content_type.clone(),
            })
            .await
            .map_err(map_backend_error_to_s3)?;

        let effective_topology =
            effective_topology_for_replication(&state, ReplicationOperation::Put, &bucket, &key)
                .map_err(map_backend_error_to_s3)?;
        let jobs = state.replication.enqueue_put(
            &effective_topology,
            Some(primary_provider.as_str().to_string()),
            bucket.clone(),
            key.clone(),
            result.etag.clone(),
            body_len,
            content_type,
        );
        if let Err(error) = state.metadata_store.enqueue_jobs(&jobs) {
            warn!(
                bucket = %bucket,
                key = %key,
                error = %error,
                "failed to persist replication jobs after put"
            );
        }
        if !jobs.is_empty() {
            info!(
                bucket = %bucket,
                key = %key,
                queued_jobs = jobs.len(),
                "replication jobs enqueued after put"
            );
        }

        let mut response = StatusCode::OK.into_response();
        if let Some(etag) = result.etag {
            response.headers_mut().insert(
                ETAG,
                HeaderValue::from_str(&quoted_etag(Some(&etag))).expect("etag should be valid"),
            );
        }
        return Ok(response);
    }

    let body = to_bytes(body, state.config.max_in_memory_object_bytes)
        .await
        .map_err(|error| {
            S3Error::entity_too_large(format!("request body exceeds in-memory limit: {error}"))
        })?;
    authorize_s3(&state.config, &method, &uri, &headers, Some(&body))?;
    ensure_object_within_in_memory_limit(&state.config, body.len() as u64)?;
    let body_len = body.len() as u64;

    let result = primary_backend
        .put_object(PutObjectRequest {
            container: bucket.clone(),
            key: key.clone(),
            body: body.into(),
            size: Some(body_len),
            content_type: content_type.clone(),
        })
        .await
        .map_err(map_backend_error_to_s3)?;

    let effective_topology =
        effective_topology_for_replication(&state, ReplicationOperation::Put, &bucket, &key)
            .map_err(map_backend_error_to_s3)?;
    let jobs = state.replication.enqueue_put(
        &effective_topology,
        Some(primary_provider.as_str().to_string()),
        bucket.clone(),
        key.clone(),
        result.etag.clone(),
        body_len,
        content_type,
    );
    if let Err(error) = state.metadata_store.enqueue_jobs(&jobs) {
        warn!(
            bucket = %bucket,
            key = %key,
            error = %error,
            "failed to persist replication jobs after put"
        );
    }
    if !jobs.is_empty() {
        info!(
            bucket = %bucket,
            key = %key,
            queued_jobs = jobs.len(),
            "replication jobs enqueued after put"
        );
    }

    let mut response = StatusCode::OK.into_response();
    if let Some(etag) = result.etag.as_deref() {
        response.headers_mut().insert(
            ETAG,
            HeaderValue::from_str(&quoted_etag(Some(etag))).expect("etag should be valid"),
        );
    }
    Ok(response)
}

async fn delete_object(
    State(state): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Response, S3Error> {
    let _permit = try_acquire_data_plane_permit(&state)?;
    authorize_s3(&state.config, &method, &uri, &headers, None)?;
    let (primary_provider, primary_backend) =
        current_primary_backend(&state).map_err(map_backend_error_to_s3)?;

    primary_backend
        .delete_object(&bucket, &key)
        .await
        .map_err(|error| map_object_error(error, &bucket, &key))?;

    let effective_topology =
        effective_topology_for_replication(&state, ReplicationOperation::Delete, &bucket, &key)
            .map_err(map_backend_error_to_s3)?;
    let jobs = state.replication.enqueue_delete(
        &effective_topology,
        Some(primary_provider.as_str().to_string()),
        bucket.clone(),
        key.clone(),
    );
    if let Err(error) = state.metadata_store.enqueue_jobs(&jobs) {
        warn!(
            bucket = %bucket,
            key = %key,
            error = %error,
            "failed to persist replication jobs after delete"
        );
    }
    if !jobs.is_empty() {
        info!(
            bucket = %bucket,
            key = %key,
            queued_jobs = jobs.len(),
            "replication jobs enqueued after delete"
        );
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

fn authorize_s3(
    config: &AppConfig,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: Option<&[u8]>,
) -> Result<(), S3Error> {
    let authorization = headers
        .get(AUTHORIZATION)
        .ok_or_else(|| S3Error::access_denied("Missing Authorization header."))?
        .to_str()
        .map_err(|_| S3Error::access_denied("Authorization header is not valid UTF-8."))?;

    let parsed = parse_authorization_header(authorization)?;

    if parsed.access_key != config.s3_access_key_id {
        return Err(S3Error::invalid_access_key());
    }

    if parsed.region != config.s3_region {
        return Err(S3Error::signature_mismatch(format!(
            "Credential should use region {}.",
            config.s3_region
        )));
    }

    if parsed.service != "s3" {
        return Err(S3Error::signature_mismatch(
            "Credential scope service must be s3.",
        ));
    }

    let amz_date = headers
        .get("x-amz-date")
        .ok_or_else(|| S3Error::access_denied("Missing x-amz-date header."))?
        .to_str()
        .map_err(|_| S3Error::access_denied("x-amz-date header is not valid UTF-8."))?;

    let payload_hash = headers
        .get("x-amz-content-sha256")
        .ok_or_else(|| S3Error::access_denied("Missing x-amz-content-sha256 header."))?
        .to_str()
        .map_err(|_| S3Error::access_denied("x-amz-content-sha256 header is not valid UTF-8."))?;

    if payload_hash != "UNSIGNED-PAYLOAD" {
        let expected_hash = sha256_hex(body.unwrap_or_default());
        if expected_hash != payload_hash {
            return Err(S3Error::signature_mismatch(
                "x-amz-content-sha256 does not match the request body.",
            ));
        }
    }

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.as_str(),
        canonical_uri(uri.path()),
        canonical_query_string(uri.query()),
        canonical_headers(headers, &parsed.signed_headers)?,
        parsed.signed_headers.join(";"),
        payload_hash
    );

    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{}/{}/{}/aws4_request\n{}",
        amz_date,
        parsed.date,
        parsed.region,
        parsed.service,
        sha256_hex(canonical_request.as_bytes())
    );

    let expected_signature = sign_v4(
        &config.s3_secret_access_key,
        &parsed.date,
        &parsed.region,
        &parsed.service,
        &string_to_sign,
    );

    if expected_signature != parsed.signature {
        return Err(S3Error::signature_mismatch(
            "The request signature we calculated does not match the signature you provided.",
        ));
    }

    Ok(())
}

fn parse_authorization_header(value: &str) -> Result<ParsedAuthorization, S3Error> {
    let Some(parameters) = value.strip_prefix("AWS4-HMAC-SHA256 ") else {
        return Err(S3Error::access_denied(
            "Only AWS4-HMAC-SHA256 Authorization is supported.",
        ));
    };

    let mut entries = BTreeMap::new();
    for item in parameters.split(',') {
        let trimmed = item.trim();
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(S3Error::signature_mismatch(
                "Authorization header is malformed.",
            ));
        };
        entries.insert(key.to_string(), value.to_string());
    }

    let credential = entries
        .get("Credential")
        .ok_or_else(|| S3Error::signature_mismatch("Credential is missing."))?;
    let signed_headers = entries
        .get("SignedHeaders")
        .ok_or_else(|| S3Error::signature_mismatch("SignedHeaders is missing."))?;
    let signature = entries
        .get("Signature")
        .ok_or_else(|| S3Error::signature_mismatch("Signature is missing."))?;

    let parts: Vec<_> = credential.split('/').collect();
    if parts.len() != 5 || parts[4] != "aws4_request" {
        return Err(S3Error::signature_mismatch(
            "Credential scope is malformed.",
        ));
    }

    Ok(ParsedAuthorization {
        access_key: parts[0].to_string(),
        date: parts[1].to_string(),
        region: parts[2].to_string(),
        service: parts[3].to_string(),
        signed_headers: signed_headers.split(';').map(ToString::to_string).collect(),
        signature: signature.to_string(),
    })
}

fn canonical_uri(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn canonical_query_string(query: Option<&str>) -> String {
    let Some(query) = query else {
        return String::new();
    };

    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut segments = part.splitn(2, '=');
            (
                segments.next().unwrap_or_default().to_string(),
                segments.next().unwrap_or_default().to_string(),
            )
        })
        .collect();

    pairs.sort();

    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn canonical_headers(headers: &HeaderMap, signed_headers: &[String]) -> Result<String, S3Error> {
    let mut parts = Vec::with_capacity(signed_headers.len());

    for name in signed_headers {
        let value = if name == "host" {
            headers
                .get(HOST)
                .ok_or_else(|| S3Error::signature_mismatch("Signed host header is missing."))?
        } else {
            headers.get(name.as_str()).ok_or_else(|| {
                S3Error::signature_mismatch(format!("Signed header {name} is missing."))
            })?
        };

        let normalized = normalize_header_value(
            value
                .to_str()
                .map_err(|_| S3Error::signature_mismatch("Signed header is not valid UTF-8."))?,
        );
        parts.push(format!("{name}:{normalized}\n"));
    }

    Ok(parts.concat())
}

fn normalize_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sign_v4(secret: &str, date: &str, region: &str, service: &str, string_to_sign: &str) -> String {
    let date_key = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, service.as_bytes());
    let signing_key = hmac_sha256(&service_key, b"aws4_request");
    hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()))
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac key should be valid");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn map_backend_error_to_s3(error: BlobError) -> S3Error {
    match error {
        BlobError::Configuration(message) => S3Error::internal_error(message),
        BlobError::Upstream(message) => S3Error::internal_error(message),
        BlobError::BodyStream(message) => S3Error::internal_error(message),
        BlobError::NotImplemented(message) => S3Error::not_implemented(message),
        BlobError::NotFound(message) => S3Error::internal_error(message),
    }
}

fn map_bucket_error(error: BlobError, bucket: &str) -> S3Error {
    match error {
        BlobError::NotFound(_) => S3Error::no_such_bucket(bucket),
        other => map_backend_error_to_s3(other),
    }
}

fn map_object_error(error: BlobError, bucket: &str, key: &str) -> S3Error {
    match error {
        BlobError::NotFound(_) => S3Error::no_such_key(bucket, key),
        other => map_backend_error_to_s3(other),
    }
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn quoted_etag(etag: Option<&str>) -> String {
    format!("\"{}\"", etag.unwrap_or(""))
}

fn xml_response(status: StatusCode, body: String) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/xml"));
    headers.insert("x-amz-request-id", HeaderValue::from_static(REQUEST_ID));

    (status, headers, body).into_response()
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use axum::body::to_bytes;
    use blob_core::{
        BackendCapabilities, BrowserFlowElement, BrowserFlowOperation, BrowserFlowPage,
        BrowserFlowRequest, ContainerInfo, HealthStatus, ObjectInfo, ServiceHealth,
        StorageScopeHealth, StorageScopeKind,
    };

    fn temp_db_path() -> String {
        std::env::temp_dir()
            .join(format!(
                "ccbg-gatewayd-test-{}-{}.db",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock should be valid")
                    .as_nanos()
            ))
            .display()
            .to_string()
    }

    struct FailingBackend {
        name: &'static str,
        message: String,
    }

    impl FailingBackend {
        fn new(name: &'static str, message: impl Into<String>) -> Self {
            Self {
                name,
                message: message.into(),
            }
        }

        fn error(&self) -> BlobError {
            BlobError::Upstream(self.message.clone())
        }
    }

    #[async_trait::async_trait]
    impl BlobBackend for FailingBackend {
        fn name(&self) -> &'static str {
            self.name
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
            Ok(ServiceHealth {
                backend: self.name().to_string(),
                status: HealthStatus::Unavailable,
                capabilities: self.capabilities(),
                scopes: Vec::new(),
                notes: vec![self.message.clone()],
            })
        }

        async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
            Err(self.error())
        }

        async fn list_objects(
            &self,
            _request: ListObjectsRequest,
        ) -> Result<Vec<ObjectInfo>, BlobError> {
            Err(self.error())
        }

        async fn get_object(
            &self,
            _container: &str,
            _key: &str,
        ) -> Result<blob_core::ObjectPayload, BlobError> {
            Err(self.error())
        }
    }

    struct FlakyWriteBackend {
        name: &'static str,
        message: String,
        remaining_write_failures: Mutex<u32>,
        inner: StubBackend,
    }

    impl FlakyWriteBackend {
        fn new(
            name: &'static str,
            remaining_write_failures: u32,
            message: impl Into<String>,
        ) -> Self {
            Self {
                name,
                message: message.into(),
                remaining_write_failures: Mutex::new(remaining_write_failures),
                inner: StubBackend::new(),
            }
        }

        fn maybe_fail_write(&self) -> Result<(), BlobError> {
            let mut remaining = self
                .remaining_write_failures
                .lock()
                .expect("flaky backend state poisoned");
            if *remaining == 0 {
                return Ok(());
            }
            *remaining -= 1;
            Err(BlobError::Upstream(self.message.clone()))
        }
    }

    struct ScopedStubBackend {
        name: &'static str,
        inner: StubBackend,
        scopes: Vec<StorageScopeHealth>,
        notes: Vec<String>,
    }

    impl ScopedStubBackend {
        fn new(name: &'static str, scopes: Vec<StorageScopeHealth>, notes: Vec<String>) -> Self {
            Self {
                name,
                inner: StubBackend::new(),
                scopes,
                notes,
            }
        }
    }

    #[async_trait::async_trait]
    impl BlobBackend for ScopedStubBackend {
        fn name(&self) -> &'static str {
            self.name
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
            Ok(ServiceHealth {
                backend: self.name().to_string(),
                status: HealthStatus::Healthy,
                capabilities: self.capabilities(),
                scopes: self.scopes.clone(),
                notes: self.notes.clone(),
            })
        }

        async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
            self.inner.list_containers().await
        }

        async fn list_objects(
            &self,
            request: ListObjectsRequest,
        ) -> Result<Vec<ObjectInfo>, BlobError> {
            self.inner.list_objects(request).await
        }

        async fn get_object(
            &self,
            container: &str,
            key: &str,
        ) -> Result<blob_core::ObjectPayload, BlobError> {
            self.inner.get_object(container, key).await
        }

        async fn put_object(
            &self,
            request: PutObjectRequest,
        ) -> Result<blob_core::PutObjectResult, BlobError> {
            self.inner.put_object(request).await
        }

        async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
            self.inner.delete_object(container, key).await
        }
    }

    #[derive(Debug, Default)]
    struct RecordingBrowserFlowSession {
        actions: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Debug, Clone, Default)]
    struct RecordingBrowserFlowOutputReader {
        values: Arc<Mutex<HashMap<String, serde_json::Value>>>,
        current_url: Arc<Mutex<Option<String>>>,
    }

    impl RecordingBrowserFlowSession {
        fn actions(&self) -> Vec<String> {
            self.actions
                .lock()
                .expect("recording browser flow session poisoned")
                .clone()
        }

        fn record(&self, action: String) {
            self.actions
                .lock()
                .expect("recording browser flow session poisoned")
                .push(action);
        }
    }

    #[async_trait::async_trait]
    impl BrowserFlowOutputReader for RecordingBrowserFlowOutputReader {
        async fn evaluate_output_script(
            &self,
            expression: &str,
        ) -> Result<serde_json::Value, BlobError> {
            self.values
                .lock()
                .expect("recording browser flow output reader poisoned")
                .get(expression)
                .cloned()
                .ok_or_else(|| {
                    BlobError::NotFound(format!(
                        "recording browser flow output not found for expression: {expression}"
                    ))
                })
        }

        async fn read_current_url(&self) -> Result<Option<String>, BlobError> {
            Ok(self
                .current_url
                .lock()
                .expect("recording browser flow output reader poisoned")
                .clone())
        }
    }

    #[async_trait::async_trait]
    impl BrowserFlowSession for RecordingBrowserFlowSession {
        async fn navigate(&self, url: &str) -> Result<(), BlobError> {
            self.record(format!("navigate:{url}"));
            Ok(())
        }

        async fn click(&self, element: &BrowserFlowElement) -> Result<(), BlobError> {
            self.record(format!("click:{}", element.id));
            Ok(())
        }

        async fn set_input(
            &self,
            element: &BrowserFlowElement,
            value: &str,
            dispatch_events: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!(
                "set_input:{}:{}:{}",
                element.id,
                value,
                dispatch_events.join(",")
            ));
            Ok(())
        }

        async fn invoke_operation(
            &self,
            operation: &BrowserFlowOperation,
        ) -> Result<(), BlobError> {
            self.record(format!("invoke_operation:{}", operation.id));
            Ok(())
        }

        async fn set_files(
            &self,
            element: &BrowserFlowElement,
            paths: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!("set_files:{}:{}", element.id, paths.join("|")));
            Ok(())
        }

        async fn dispatch_events(
            &self,
            element: &BrowserFlowElement,
            events: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!(
                "dispatch_events:{}:{}",
                element.id,
                events.join(",")
            ));
            Ok(())
        }

        async fn wait_for_request(
            &self,
            request: &BrowserFlowRequest,
            timeout_ms: Option<u64>,
        ) -> Result<(), BlobError> {
            self.record(format!(
                "wait_for_request:{}:{}",
                request.id,
                timeout_ms.unwrap_or_default()
            ));
            Ok(())
        }

        async fn wait_for_page(
            &self,
            page: &BrowserFlowPage,
            timeout_ms: Option<u64>,
        ) -> Result<(), BlobError> {
            self.record(format!(
                "wait_for_page:{}:{}",
                page.id,
                timeout_ms.unwrap_or_default()
            ));
            Ok(())
        }

        async fn wait(&self, duration_ms: u64) -> Result<(), BlobError> {
            self.record(format!("wait:{duration_ms}"));
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl BlobBackend for FlakyWriteBackend {
        fn name(&self) -> &'static str {
            self.name
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
            Ok(ServiceHealth {
                backend: self.name().to_string(),
                status: HealthStatus::Healthy,
                capabilities: self.capabilities(),
                scopes: Vec::new(),
                notes: vec!["test flaky backend".to_string()],
            })
        }

        async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
            self.inner.list_containers().await
        }

        async fn list_objects(
            &self,
            request: ListObjectsRequest,
        ) -> Result<Vec<ObjectInfo>, BlobError> {
            self.inner.list_objects(request).await
        }

        async fn get_object(
            &self,
            container: &str,
            key: &str,
        ) -> Result<blob_core::ObjectPayload, BlobError> {
            self.inner.get_object(container, key).await
        }

        async fn put_object(
            &self,
            request: PutObjectRequest,
        ) -> Result<blob_core::PutObjectResult, BlobError> {
            self.maybe_fail_write()?;
            self.inner.put_object(request).await
        }

        async fn delete_object(&self, container: &str, key: &str) -> Result<(), BlobError> {
            self.inner.delete_object(container, key).await
        }
    }

    fn test_config() -> Arc<AppConfig> {
        let topology = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Stub,
            sync_targets: vec![ProviderId::Onedrive],
            fallback_read_order: vec![ProviderId::Onedrive],
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect("test topology should validate");
        let browser_flow_catalog_dir = FsPath::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/browser-flows")
            .display()
            .to_string();
        let provider_capability_catalog_dir = FsPath::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/provider-capabilities")
            .display()
            .to_string();

        Arc::new(AppConfig {
            bind_addr: "127.0.0.1:61080".parse().expect("test addr should parse"),
            admin_mode: AdminMode::Web,
            admin_bind_addr: "127.0.0.1:61081".parse().expect("admin addr should parse"),
            auth_callback_bind_addr: "127.0.0.1:61082"
                .parse()
                .expect("callback addr should parse"),
            metrics_bind_addr: "127.0.0.1:61083"
                .parse()
                .expect("metrics addr should parse"),
            notify_webhook_url: None,
            notify_webhook_signing_secret: None,
            notify_poll_interval_seconds: 15,
            replication_failed_alert_threshold: 1,
            replication_failed_alert_min_age_ms: 0,
            control_plane_file: temp_db_path().replace(".db", "-control-plane.json"),
            credentials_dir: temp_db_path().replace(".db", "-provider-credentials"),
            browser_flow_catalog_dir,
            provider_capability_catalog_dir,
            topology,
            s3_access_key_id: "ccbg-test".to_string(),
            s3_secret_access_key: "ccbg-secret".to_string(),
            s3_region: "us-east-1".to_string(),
            metadata_db_path: temp_db_path(),
            metadata_snapshot_recent_limit: 32,
            metadata_retention: MetadataRetentionPolicy {
                completed_history_limit: 512,
                failed_history_limit: 256,
            },
            replication_workers: 0,
            data_plane_max_in_flight: 8,
            replication_recent_limit: 64,
            replication_max_attempts: 3,
            replication_base_retry_delay_ms: 0,
            replication_max_retry_delay_ms: 0,
            object_action_history_limit: DEFAULT_OBJECT_ACTION_HISTORY_LIMIT,
            max_in_memory_object_bytes: 8 * 1024 * 1024,
            onedrive: OneDriveConfig {
                enabled: true,
                tenant: "common".to_string(),
                client_id: Some("unit-test-client".to_string()),
                use_device_code: true,
                redirect_url: Some("http://127.0.0.1:61082/auth/onedrive/callback".to_string()),
                drive_id: Some("drive-test".to_string()),
                graph_base_url: "https://graph.microsoft.com/v1.0".to_string(),
                auth_base_url: DEFAULT_ONEDRIVE_AUTH_BASE_URL.to_string(),
                scopes: DEFAULT_ONEDRIVE_SCOPES.to_string(),
                session_file: Some("./data/onedrive-session.json".to_string()),
                token_source: TokenSource::Static {
                    bearer: "unit-test-token".to_string(),
                },
                root_prefix: Some("ccbg-tests".to_string()),
                user_agent: "carrier-cloud-blob-gateway-test".to_string(),
                request_timeout_secs: 5,
            },
        })
    }

    fn test_state() -> AppState {
        let config = test_config();
        let metadata_store = Arc::new(
            MetadataStore::open_with_options(
                &config.metadata_db_path,
                MetadataStoreOptions {
                    retention: config.metadata_retention,
                },
            )
            .expect("store should open"),
        );

        AppState {
            config: config.clone(),
            backends: Arc::new(Mutex::new(
                build_all_backends(&config).expect("test backends should build"),
            )),
            replication: Arc::new(ReplicationEngine::with_recent_limit(
                config.replication_recent_limit,
            )),
            metadata_store,
            auth: Arc::new(AuthBrokerState::new()),
            control_plane: Arc::new(Mutex::new(ControlPlaneState {
                topology: config.topology.clone(),
                onedrive_policy: OnedrivePolicy {
                    replication_enabled: true,
                    fallback_enabled: true,
                    scope_mode: OnedriveScopeMode::All,
                    memory_buckets: Vec::new(),
                    memory_prefixes: Vec::new(),
                    updated_at_unix_ms: current_unix_ms(),
                },
                auth_capture_policy: AuthCapturePolicy::from_env_defaults(),
                object_action_history: Vec::new(),
            })),
            notify_state: Arc::new(Mutex::new(NotifyState {
                last_alert_hash: None,
                last_attempt_at_unix_ms: None,
                last_success_at_unix_ms: None,
                last_error: None,
            })),
            browser_flow_catalogs: Arc::new(
                BrowserFlowCatalogCollection::from_json_dir(&config.browser_flow_catalog_dir)
                    .expect("test browser flow catalogs should load"),
            ),
            data_plane_concurrency: Arc::new(DataPlaneConcurrencyState {
                semaphore: Arc::new(Semaphore::new(config.data_plane_max_in_flight)),
            }),
            started_at_unix_ms: current_unix_ms().saturating_sub(5_000),
        }
    }

    #[derive(Debug, Clone)]
    struct RecordedWebhookRequest {
        body: String,
        event_id: Option<String>,
        signature_version: Option<String>,
        signature: Option<String>,
        timestamp: Option<String>,
    }

    async fn spawn_test_webhook_server() -> (
        String,
        Arc<Mutex<Vec<RecordedWebhookRequest>>>,
        oneshot::Sender<()>,
    ) {
        let received = Arc::new(Mutex::new(Vec::<RecordedWebhookRequest>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test webhook listener should bind");
        let addr = listener.local_addr().expect("local addr should load");
        let received_clone = received.clone();
        let app = Router::new().route(
            "/",
            any(move |request: Request<Body>| {
                let received = received_clone.clone();
                async move {
                    let event_id = request
                        .headers()
                        .get(NOTIFY_EVENT_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(ToString::to_string);
                    let signature_version = request
                        .headers()
                        .get(NOTIFY_SIGNATURE_VERSION_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(ToString::to_string);
                    let signature = request
                        .headers()
                        .get(NOTIFY_SIGNATURE_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(ToString::to_string);
                    let timestamp = request
                        .headers()
                        .get(NOTIFY_TIMESTAMP_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .map(ToString::to_string);
                    let bytes = to_bytes(request.into_body(), usize::MAX)
                        .await
                        .expect("webhook request body should read");
                    let body = String::from_utf8(bytes.to_vec())
                        .expect("webhook request body should be utf-8");
                    received
                        .lock()
                        .expect("received webhook list poisoned")
                        .push(RecordedWebhookRequest {
                            body,
                            event_id,
                            signature_version,
                            signature,
                            timestamp,
                        });
                    StatusCode::NO_CONTENT
                }
            }),
        );
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        (format!("http://{addr}"), received, shutdown_tx)
    }

    fn backend_for_test(state: &AppState, provider: ProviderId) -> DynBackend {
        let backends = backends_snapshot(state);
        backend_for_provider_from(&backends, provider)
            .expect("backend should exist in test registry")
    }

    fn replace_backend(state: &mut AppState, provider: ProviderId, backend: DynBackend) {
        let mut backends = state.backends.lock().expect("backend registry should lock");
        replace_backend_in_registry(&mut backends, provider, backend);
    }

    fn record_replication_state(
        state: &AppState,
        provider: ProviderId,
        operation: ReplicationOperation,
        status: ReplicationStatus,
        bucket: &str,
        key: &str,
        size: Option<u64>,
        content_type: Option<&str>,
    ) {
        let job_id = state
            .metadata_store
            .max_job_id()
            .expect("max job id should load")
            .unwrap_or(0)
            .saturating_add(1);

        let job = ReplicationJob {
            job_id,
            target: provider.as_str().to_string(),
            source_provider: Some(ProviderId::Stub.as_str().to_string()),
            operation,
            object: replication_engine::ReplicationObjectRef {
                bucket: bucket.to_string(),
                key: key.to_string(),
                etag: None,
                size,
                content_type: content_type.map(ToString::to_string),
            },
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms: 1,
            next_attempt_at_unix_ms: None,
            last_error: None,
        };

        state
            .metadata_store
            .enqueue_jobs(&[job])
            .expect("job should persist");

        if !matches!(status, ReplicationStatus::Pending) {
            state
                .metadata_store
                .mark_job_status(job_id, status, 1, None)
                .expect("job status should update");
        }
    }

    fn signed_headers(
        config: &AppConfig,
        method: &Method,
        uri: &Uri,
        body: &[u8],
        extra_headers: &[(&str, &str)],
    ) -> HeaderMap {
        let amz_date = "20260424T120000Z";
        let short_date = "20260424";
        let payload_hash = sha256_hex(body);

        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("127.0.0.1:61080"));
        headers.insert("x-amz-date", HeaderValue::from_static("20260424T120000Z"));
        headers.insert(
            "x-amz-content-sha256",
            HeaderValue::from_str(&payload_hash).expect("payload hash should be valid"),
        );

        let mut signed_headers = vec![
            "host".to_string(),
            "x-amz-content-sha256".to_string(),
            "x-amz-date".to_string(),
        ];

        for (name, value) in extra_headers {
            let header_name =
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("header name is valid");
            headers.insert(
                header_name,
                HeaderValue::from_str(value).expect("extra header should be valid"),
            );
            signed_headers.push(name.to_ascii_lowercase());
        }

        signed_headers.sort();
        signed_headers.dedup();

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri(uri.path()),
            canonical_query_string(uri.query()),
            canonical_headers(&headers, &signed_headers)
                .expect("canonical headers should build in tests"),
            signed_headers.join(";"),
            payload_hash
        );

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}/{}/s3/aws4_request\n{}",
            amz_date,
            short_date,
            config.s3_region,
            sha256_hex(canonical_request.as_bytes())
        );

        let signature = sign_v4(
            &config.s3_secret_access_key,
            short_date,
            &config.s3_region,
            "s3",
            &string_to_sign,
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/s3/aws4_request, SignedHeaders={}, Signature={}",
            config.s3_access_key_id,
            short_date,
            config.s3_region,
            signed_headers.join(";"),
            signature
        );

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&authorization).expect("authorization should be valid"),
        );

        headers
    }

    fn unsigned_payload_headers(
        config: &AppConfig,
        method: &Method,
        uri: &Uri,
        content_length: u64,
        extra_headers: &[(&str, &str)],
    ) -> HeaderMap {
        let amz_date = "20260424T120000Z";
        let short_date = "20260424";
        let payload_hash = "UNSIGNED-PAYLOAD";

        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("127.0.0.1:61080"));
        headers.insert("x-amz-date", HeaderValue::from_static("20260424T120000Z"));
        headers.insert(
            "x-amz-content-sha256",
            HeaderValue::from_static(payload_hash),
        );
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&content_length.to_string())
                .expect("content length should be valid"),
        );

        let mut signed_headers = vec![
            "content-length".to_string(),
            "host".to_string(),
            "x-amz-content-sha256".to_string(),
            "x-amz-date".to_string(),
        ];

        for (name, value) in extra_headers {
            let header_name =
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("header name is valid");
            headers.insert(
                header_name,
                HeaderValue::from_str(value).expect("extra header should be valid"),
            );
            signed_headers.push(name.to_ascii_lowercase());
        }

        signed_headers.sort();
        signed_headers.dedup();

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri(uri.path()),
            canonical_query_string(uri.query()),
            canonical_headers(&headers, &signed_headers)
                .expect("canonical headers should build in tests"),
            signed_headers.join(";"),
            payload_hash
        );

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}/{}/s3/aws4_request\n{}",
            amz_date,
            short_date,
            config.s3_region,
            sha256_hex(canonical_request.as_bytes())
        );

        let signature = sign_v4(
            &config.s3_secret_access_key,
            short_date,
            &config.s3_region,
            "s3",
            &string_to_sign,
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}/{}/s3/aws4_request, SignedHeaders={}, Signature={}",
            config.s3_access_key_id,
            short_date,
            config.s3_region,
            signed_headers.join(";"),
            signature
        );

        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&authorization).expect("authorization header should be valid"),
        );

        headers
    }

    #[tokio::test]
    async fn list_buckets_returns_s3_xml() {
        let state = test_state();
        let uri: Uri = "/".parse().expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);

        let response = list_buckets(State(state), Method::GET, OriginalUri(uri), headers)
            .await
            .expect("list buckets should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/xml")
        );
        assert_eq!(
            response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stub")
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be valid utf-8");

        assert!(body.contains("<ListAllMyBucketsResult"));
        assert!(body.contains("<Name>placeholder</Name>"));
    }

    #[tokio::test]
    async fn provider_health_includes_primary_and_sync_targets() {
        let Json(providers) = list_provider_health(State(test_state()))
            .await
            .expect("provider health should succeed");

        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].role, "primary");
        assert_eq!(providers[0].provider, "stub");
        assert_eq!(providers[1].role, "sync_target");
        assert_eq!(providers[1].provider, "onedrive");
    }

    #[tokio::test]
    async fn provider_test_returns_immediate_health_payload() {
        let Json(payload) = test_provider(State(test_state()), Path("stub".to_string()))
            .await
            .expect("provider test should succeed");

        assert_eq!(payload.provider, "stub");
        assert!(payload.roles.contains(&"primary"));
        assert!(matches!(payload.health.status, HealthStatus::Degraded));
    }

    #[tokio::test]
    async fn metrics_health_endpoints_expose_runtime_and_metrics() {
        let state = test_state();

        let Json(health) = metrics_healthz(State(state.clone()))
            .await
            .expect("metrics health should succeed");
        assert_eq!(health.status, "ok");
        assert!(health.ready);
        assert_eq!(health.runtime.metrics_bind_addr, "127.0.0.1:61083");
        assert_eq!(health.runtime.data_plane_max_in_flight, 8);
        assert_eq!(health.monitoring.provider_summary.total, 2);

        let ready_status = metrics_readyz(State(state.clone()))
            .await
            .expect("metrics ready should succeed");
        assert_eq!(ready_status, StatusCode::OK);

        let metrics_response = metrics_prometheus(State(state))
            .await
            .expect("metrics should succeed");
        assert_eq!(metrics_response.status(), StatusCode::OK);
        assert_eq!(
            metrics_response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
        let body = to_bytes(metrics_response.into_body(), usize::MAX)
            .await
            .expect("metrics body should read");
        let body = String::from_utf8(body.to_vec()).expect("metrics body should be utf-8");
        assert!(body.contains("ccbg_up 1"));
        assert!(body.contains("ccbg_admin_alerts_open"));
        assert!(body.contains("ccbg_data_plane_concurrency_configured 8"));
        assert!(body.contains("ccbg_data_plane_concurrency_available 8"));
        assert!(body.contains("ccbg_provider_health{provider=\"stub\",role=\"primary\"} 1"));
        assert!(body.contains("ccbg_replication_jobs{status=\"failed\"} 0"));
    }

    #[tokio::test]
    async fn metrics_readyz_reports_primary_unavailable() {
        let mut state = test_state();
        replace_backend(
            &mut state,
            ProviderId::Stub,
            Arc::new(FailingBackend::new("stub", "primary backend unavailable")),
        );

        let ready_status = metrics_readyz(State(state.clone()))
            .await
            .expect("metrics ready should succeed");
        assert_eq!(ready_status, StatusCode::SERVICE_UNAVAILABLE);

        let Json(health) = metrics_healthz(State(state))
            .await
            .expect("metrics health should succeed");
        assert_eq!(health.status, "degraded");
        assert!(!health.ready);
        assert!(
            health
                .alerts
                .iter()
                .any(|alert| alert.title.contains("unavailable"))
        );
    }

    #[tokio::test]
    async fn notify_webhook_posts_only_when_alert_state_changes() {
        let (webhook_url, received, shutdown) = spawn_test_webhook_server().await;
        let mut state = test_state();
        Arc::make_mut(&mut state.config).notify_webhook_url = Some(webhook_url);

        process_notify_tick(&state)
            .await
            .expect("first notify tick should succeed");
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            received
                .lock()
                .expect("received webhook list poisoned")
                .len(),
            1
        );

        process_notify_tick(&state)
            .await
            .expect("second notify tick should succeed");
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            received
                .lock()
                .expect("received webhook list poisoned")
                .len(),
            1
        );

        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "notify/changed.txt",
            Some(7),
            Some("text/plain"),
        );

        process_notify_tick(&state)
            .await
            .expect("changed notify tick should succeed");
        sleep(Duration::from_millis(50)).await;
        let received_guard = received.lock().expect("received webhook list poisoned");
        assert_eq!(received_guard.len(), 2);
        assert!(
            received_guard[1]
                .body
                .contains("latest failed replication object")
        );
        assert!(received_guard[1].event_id.is_some());
        assert!(received_guard[1].timestamp.is_some());
        assert!(received_guard[1].signature.is_none());
        assert!(received_guard[1].signature_version.is_none());
        drop(received_guard);

        let notify_status = current_notify_status_payload(&state);
        assert!(notify_status.webhook_enabled);
        assert!(!notify_status.signature_enabled);
        assert!(notify_status.last_success_at_unix_ms.is_some());
        assert!(notify_status.last_error.is_none());

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn notify_webhook_includes_hmac_signature_when_secret_is_configured() {
        let (webhook_url, received, shutdown) = spawn_test_webhook_server().await;
        let mut state = test_state();
        Arc::make_mut(&mut state.config).notify_webhook_url = Some(webhook_url);
        Arc::make_mut(&mut state.config).notify_webhook_signing_secret =
            Some("notify-secret".to_string());

        process_notify_tick(&state)
            .await
            .expect("signed notify tick should succeed");
        sleep(Duration::from_millis(50)).await;

        let received_guard = received.lock().expect("received webhook list poisoned");
        assert_eq!(received_guard.len(), 1);
        let request = &received_guard[0];
        let event_id = request
            .event_id
            .as_deref()
            .expect("signed webhook should include event id");
        assert!(
            request
                .body
                .contains(&format!("\"event_id\":\"{event_id}\""))
        );
        assert_eq!(request.signature_version.as_deref(), Some("v1"));
        let timestamp = request
            .timestamp
            .as_deref()
            .expect("signed webhook should include timestamp");
        let timestamp = timestamp
            .parse::<u64>()
            .expect("timestamp header should parse");
        let signature = request
            .signature
            .as_deref()
            .expect("signed webhook should include signature");
        assert_eq!(
            signature,
            sign_notify_payload("notify-secret", timestamp, request.body.as_bytes())
        );
        drop(received_guard);

        let notify_status = current_notify_status_payload(&state);
        assert!(notify_status.signature_enabled);

        let _ = shutdown.send(());
    }

    #[test]
    fn notify_signature_example_matches_reference_receiver() {
        let body = br#"{"event_id":"evt-123","alerts":[{"title":"provider unavailable"}]}"#;
        let timestamp = 1_710_000_000_123_u64;
        assert_eq!(
            sign_notify_payload("notify-secret", timestamp, body),
            "373958a9b493acf9954727491b8b6d6335c49d34d77eafe0faf5c23fb4c59dc4"
        );
    }

    #[tokio::test]
    async fn provider_health_surfaces_family_scope_for_unicom_primary() {
        let mut state = test_state();
        let unicom_backend: DynBackend = Arc::new(ScopedStubBackend::new(
            "unicom-cloud-drive",
            vec![
                StorageScopeHealth {
                    id: "root".to_string(),
                    label: "Personal".to_string(),
                    kind: StorageScopeKind::Personal,
                    writable: true,
                    root: Some("0".to_string()),
                    container: Some("root".to_string()),
                    object_count: Some(3),
                    capacity: None,
                    notes: vec!["personal scope ready".to_string()],
                },
                StorageScopeHealth {
                    id: "family-123".to_string(),
                    label: "Family".to_string(),
                    kind: StorageScopeKind::Family,
                    writable: true,
                    root: Some("0".to_string()),
                    container: Some("family".to_string()),
                    object_count: Some(2),
                    capacity: None,
                    notes: vec!["family scope ready".to_string()],
                },
            ],
            vec!["stubbed unicom health".to_string()],
        ));
        replace_backend(&mut state, ProviderId::Unicom, unicom_backend);

        let Json(_) = update_topology(
            State(state.clone()),
            Json(TopologyUpdateInput {
                primary_provider: ProviderId::Unicom,
                sync_targets: vec![ProviderId::Onedrive],
                fallback_read_order: vec![ProviderId::Onedrive],
            }),
        )
        .await
        .expect("topology update should succeed");

        let Json(providers) = list_provider_health(State(state))
            .await
            .expect("provider health should succeed");

        let unicom = providers
            .into_iter()
            .find(|payload| payload.provider == "unicom" && payload.role == "primary")
            .expect("unicom primary health payload should exist");
        assert!(matches!(
            unicom.health.status,
            HealthStatus::Healthy | HealthStatus::Degraded
        ));
        assert!(
            unicom
                .health
                .scopes
                .iter()
                .any(|scope| { scope.container.as_deref() == Some("root") && scope.writable })
        );
        assert!(
            unicom
                .health
                .scopes
                .iter()
                .any(|scope| { scope.container.as_deref() == Some("family") && scope.writable })
        );
    }

    #[tokio::test]
    async fn object_status_reports_fallback_readability() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "fallback/inspect.txt".to_string(),
                body: Bytes::from_static(b"inspect fallback").into(),
                size: Some(16),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("fallback object should exist");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "placeholder",
            "fallback/inspect.txt",
            Some(16),
            Some("text/plain"),
        );

        let Json(payload) = inspect_object_status(
            State(state),
            Query(ObjectStatusQuery {
                bucket: "placeholder".to_string(),
                key: "fallback/inspect.txt".to_string(),
            }),
        )
        .await
        .expect("object status should succeed");

        assert_eq!(payload.gateway_read_source, Some("onedrive"));
        assert_eq!(payload.gateway_fallback_from, Some("stub"));
        assert_eq!(payload.provider_states.len(), 2);
        let fallback = payload
            .provider_states
            .iter()
            .find(|item| item.provider == "onedrive")
            .expect("onedrive state should exist");
        assert!(fallback.exists);
        assert!(fallback.readable_via_gateway);
        assert_eq!(fallback.fallback_gate, Some("allowed"));
    }

    #[tokio::test]
    async fn admin_status_includes_provider_health_replication_and_alerts() {
        let mut state = test_state();
        replace_backend(
            &mut state,
            ProviderId::Onedrive,
            Arc::new(StubBackend::new()),
        );
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "alerts/failure.txt",
            Some(7),
            Some("text/plain"),
        );

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert_eq!(status.runtime.admin_mode, "web");
        assert_eq!(status.runtime.bind_addr, "127.0.0.1:61080");
        assert_eq!(status.runtime.admin_bind_addr, "127.0.0.1:61081");
        assert_eq!(status.runtime.auth_callback_bind_addr, "127.0.0.1:61082");
        assert_eq!(status.runtime.replication_workers, 0);
        assert_eq!(status.runtime.data_plane_max_in_flight, 8);
        assert_eq!(status.runtime.object_action_history_limit, 12);
        assert!(status.runtime.started_at_unix_ms > 0);
        assert!(status.runtime.uptime_ms >= 5_000);
        assert_eq!(status.monitoring.provider_summary.total, 2);
        assert_eq!(status.monitoring.provider_summary.healthy, 0);
        assert_eq!(status.monitoring.provider_summary.degraded, 2);
        assert_eq!(status.monitoring.provider_summary.unavailable, 0);
        assert_eq!(status.monitoring.replication.pending_jobs, 0);
        assert_eq!(status.monitoring.replication.retry_scheduled_jobs, 0);
        assert_eq!(status.monitoring.replication.failed_jobs, 1);
        assert_eq!(status.monitoring.replication.completed_jobs, 0);
        assert_eq!(status.monitoring.object_actions.total_entries, 0);
        assert_eq!(status.monitoring.object_actions.failed_entries, 0);
        assert_eq!(status.monitoring.latest_failed_objects.len(), 1);
        assert_eq!(
            status.monitoring.latest_failed_objects[0].target.as_deref(),
            Some("onedrive")
        );
        assert_eq!(
            status.monitoring.latest_failed_objects[0].object.as_deref(),
            Some("root/alerts/failure.txt")
        );
        assert!(status.monitoring.recent_failures.len() >= 1);
        assert_eq!(status.monitoring.recent_failures[0].kind, "replication_job");
        assert!(status.monitoring.open_alert_count >= 1);
        assert_eq!(status.operations_overview.primary_provider, "stub");
        assert_eq!(status.operations_overview.replication_mode, "async_backup");
        assert!(status.operations_overview.onedrive_async_backup_enabled);
        assert!(status.operations_overview.onedrive_fallback_enabled);
        assert_eq!(status.operations_overview.data_plane_max_in_flight, 8);
        assert_eq!(status.operations_overview.data_plane_permits_available, 8);
        assert_eq!(status.operations_overview.latest_failed_objects, 1);
        assert_eq!(status.operations_overview.replication_failed_alert_threshold, 1);
        assert_eq!(status.operations_overview.replication_failed_alert_min_age_ms, 0);
        assert!(status.operations_overview.data_plane_loopback_only);
        assert!(status.operations_overview.admin_loopback_only);
        assert!(status.operations_overview.auth_callback_loopback_only);
        assert!(status.operations_overview.metrics_loopback_only);
        assert!(!status.operations_overview.s3_secret_uses_default);
        assert_eq!(status.runtime_topology.primary_provider, "stub");
        assert_eq!(status.object_action_history_limit, 12);
        assert_eq!(status.provider_health.len(), 2);
        assert_eq!(status.replication_state.persisted.pending_count, 0);
        assert_eq!(status.replication_state.persisted.failed_count, 1);
        assert!(
            status
                .browser_flow_catalogs
                .iter()
                .any(|entry| entry.provider == "unicom" && entry.surface == "pan.wo.cn-web")
        );
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("Replication workers"))
        );
    }

    #[tokio::test]
    async fn replication_failed_alert_honors_minimum_failure_age() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).replication_failed_alert_min_age_ms = 60_000;

        let now_unix_ms = current_unix_ms();
        let job = ReplicationJob {
            job_id: 1,
            target: ProviderId::Onedrive.as_str().to_string(),
            source_provider: Some(ProviderId::Stub.as_str().to_string()),
            operation: ReplicationOperation::Put,
            object: replication_engine::ReplicationObjectRef {
                bucket: "root".to_string(),
                key: "alerts/too-fresh.txt".to_string(),
                etag: None,
                size: Some(7),
                content_type: Some("text/plain".to_string()),
            },
            status: ReplicationStatus::Failed,
            attempts: 1,
            enqueued_at_unix_ms: u128::from(now_unix_ms.saturating_sub(1_000)),
            next_attempt_at_unix_ms: None,
            last_error: Some("temporary outage".to_string()),
        };
        state
            .metadata_store
            .enqueue_jobs(&[job])
            .expect("fresh failed job should persist");

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert_eq!(status.monitoring.latest_failed_objects.len(), 1);
        assert!(
            status
                .alerts
                .iter()
                .all(|alert| !alert.title.contains("latest failed replication object"))
        );
    }

    #[tokio::test]
    async fn replication_failed_alert_honors_threshold() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).replication_failed_alert_threshold = 2;

        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "alerts/threshold-one.txt",
            Some(7),
            Some("text/plain"),
        );

        let Json(status_below_threshold) = admin_status(State(state.clone()))
            .await
            .expect("admin status should succeed below threshold");
        assert!(
            status_below_threshold
                .alerts
                .iter()
                .all(|alert| !alert.title.contains("latest failed replication object"))
        );

        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "alerts/threshold-two.txt",
            Some(7),
            Some("text/plain"),
        );

        let Json(status_above_threshold) = admin_status(State(state))
            .await
            .expect("admin status should succeed above threshold");
        assert!(
            status_above_threshold
                .alerts
                .iter()
                .any(|alert| alert.title.contains("latest failed replication object"))
        );
    }

    #[tokio::test]
    async fn operations_overview_surfaces_queue_age_and_notify_freshness() {
        let state = test_state();
        let now_unix_ms = current_unix_ms();

        let pending_job = ReplicationJob {
            job_id: 1,
            target: ProviderId::Onedrive.as_str().to_string(),
            source_provider: Some(ProviderId::Stub.as_str().to_string()),
            operation: ReplicationOperation::Put,
            object: replication_engine::ReplicationObjectRef {
                bucket: "root".to_string(),
                key: "ops/pending.txt".to_string(),
                etag: None,
                size: Some(5),
                content_type: Some("text/plain".to_string()),
            },
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms: u128::from(now_unix_ms.saturating_sub(30_000)),
            next_attempt_at_unix_ms: None,
            last_error: None,
        };
        state.replication.enqueue_existing_job(pending_job.clone());
        state
            .metadata_store
            .enqueue_jobs(&[pending_job])
            .expect("pending job should persist");

        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "ops/failed.txt",
            Some(5),
            Some("text/plain"),
        );

        {
            let mut notify_state = state.notify_state.lock().expect("notify state poisoned");
            notify_state.last_success_at_unix_ms = Some(now_unix_ms.saturating_sub(12_000));
        }

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert_eq!(status.operations_overview.pending_jobs, 1);
        assert_eq!(status.operations_overview.data_plane_max_in_flight, 8);
        assert_eq!(status.operations_overview.data_plane_permits_available, 8);
        assert_eq!(status.operations_overview.latest_failed_objects, 1);
        assert!(
            status
                .operations_overview
                .oldest_pending_job_age_ms
                .is_some_and(|age| age >= 25_000)
        );
        assert!(
            status
                .operations_overview
                .oldest_latest_failed_object_age_ms
                .is_some()
        );
        assert!(
            status
                .operations_overview
                .notify_last_success_age_ms
                .is_some_and(|age| age >= 10_000)
        );
    }

    #[tokio::test]
    async fn admin_alerts_warn_when_router_listeners_are_exposed_or_secret_is_default() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).admin_bind_addr =
            "0.0.0.0:61081".parse().expect("admin addr should parse");
        Arc::make_mut(&mut state.config).auth_callback_bind_addr =
            "0.0.0.0:61082".parse().expect("callback addr should parse");
        Arc::make_mut(&mut state.config).metrics_bind_addr =
            "0.0.0.0:61083".parse().expect("metrics addr should parse");
        Arc::make_mut(&mut state.config).s3_secret_access_key = "change-me".to_string();

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert!(!status.operations_overview.admin_loopback_only);
        assert!(!status.operations_overview.auth_callback_loopback_only);
        assert!(!status.operations_overview.metrics_loopback_only);
        assert!(status.operations_overview.s3_secret_uses_default);
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("Admin Web is exposed beyond loopback"))
        );
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("OAuth callback listener is exposed beyond loopback"))
        );
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("Metrics endpoint is exposed beyond loopback"))
        );
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("S3 secret is still using the example default"))
        );
    }

    #[tokio::test]
    async fn admin_alerts_warn_when_data_plane_concurrency_is_very_low() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).data_plane_max_in_flight = 2;
        state.data_plane_concurrency = Arc::new(DataPlaneConcurrencyState {
            semaphore: Arc::new(Semaphore::new(2)),
        });

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert_eq!(status.operations_overview.data_plane_max_in_flight, 2);
        assert_eq!(status.operations_overview.data_plane_permits_available, 2);
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("Data plane concurrency is set very low"))
        );
    }

    #[tokio::test]
    async fn data_plane_concurrency_rejects_excess_s3_requests_with_503() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).data_plane_max_in_flight = 1;
        let semaphore = Arc::new(Semaphore::new(1));
        let held_permit = semaphore
            .clone()
            .try_acquire_owned()
            .expect("initial permit should be available");
        state.data_plane_concurrency = Arc::new(DataPlaneConcurrencyState { semaphore });

        let uri: Uri = "/".parse().expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let error = list_buckets(State(state), Method::GET, OriginalUri(uri), headers)
            .await
            .expect_err("request should be rejected when semaphore is exhausted");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf-8");
        assert!(body.contains("<Code>ServiceUnavailable</Code>"));
        assert!(body.contains("too many concurrent data-plane requests"));
        assert!(body.contains("limit=1"));

        drop(held_permit);
    }

    #[tokio::test]
    async fn get_object_holds_data_plane_permit_until_body_is_consumed() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).data_plane_max_in_flight = 1;
        let semaphore = Arc::new(Semaphore::new(1));
        state.data_plane_concurrency = Arc::new(DataPlaneConcurrencyState {
            semaphore: semaphore.clone(),
        });

        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());
        primary_backend
            .put_object(PutObjectRequest {
                container: "root".to_string(),
                key: "docs/stream.txt".to_string(),
                body: Bytes::from_static(b"stream-body").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("stream object should be created");

        let uri: Uri = "/root/docs/stream.txt"
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let response = get_object(
            State(state),
            Path(("root".to_string(), "docs/stream.txt".to_string())),
            Method::GET,
            OriginalUri(uri),
            headers,
        )
        .await
        .expect("get object should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(semaphore.available_permits(), 0);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should read");
        assert_eq!(body.as_ref(), b"stream-body");
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn notify_webhook_payload_includes_latest_failed_objects_summary() {
        let (webhook_url, received, shutdown) = spawn_test_webhook_server().await;

        let mut state = test_state();
        Arc::make_mut(&mut state.config).notify_webhook_url = Some(webhook_url);
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "failed/webhook-summary.txt",
            Some(11),
            Some("text/plain"),
        );

        process_notify_tick(&state)
            .await
            .expect("notify webhook should send latest failed summary");

        let request = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let guard = received
                    .lock()
                    .expect("received requests should not poison");
                if let Some(request) = guard.first().cloned() {
                    break request;
                }
                drop(guard);
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("webhook request should arrive");

        let payload: serde_json::Value =
            serde_json::from_str(&request.body).expect("webhook payload should decode");
        let latest_failed_objects = payload["monitoring"]["latest_failed_objects"]
            .as_array()
            .expect("latest failed objects should be an array");
        assert_eq!(latest_failed_objects.len(), 1);
        assert_eq!(
            latest_failed_objects[0]["object"].as_str(),
            Some("root/failed/webhook-summary.txt")
        );
        assert_eq!(
            latest_failed_objects[0]["target"].as_str(),
            Some("onedrive")
        );

        let _ = shutdown.send(());
    }

    #[tokio::test]
    async fn browser_flow_catalog_listing_and_lookup_work() {
        let state = test_state();

        let Json(catalogs) = list_browser_flow_catalogs(State(state.clone()))
            .await
            .expect("catalog listing should succeed");
        assert!(
            catalogs
                .iter()
                .any(|entry| entry.provider == "unicom" && entry.flow_count >= 9)
        );

        let Json(catalog) = get_browser_flow_catalog(
            State(state),
            Query(BrowserFlowCatalogQuery {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
            }),
        )
        .await
        .expect("catalog lookup should succeed");
        assert_eq!(catalog.provider, "unicom");
        assert_eq!(catalog.surface, "pan.wo.cn-web");
        assert!(catalog.catalog.find_flow("unicom_copy_entry").is_some());
    }

    #[tokio::test]
    async fn browser_flow_lookup_by_id_returns_expected_flow() {
        let state = test_state();

        let Json(payload) =
            get_browser_flow_by_id(State(state), Path("unicom_move_entry".to_string()))
                .await
                .expect("flow lookup should succeed");
        assert_eq!(payload.provider, "unicom");
        assert_eq!(payload.surface, "pan.wo.cn-web");
        assert_eq!(payload.flow.id, "unicom_move_entry");
        assert_eq!(payload.flow.start_page, "file_list_all");
    }

    #[tokio::test]
    async fn browser_flow_dry_run_returns_execution_report() {
        let state = test_state();

        let Json(payload) = run_browser_flow_dry_run(
            State(state),
            Json(BrowserFlowDryRunInput {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
                flow_id: "unicom_personal_root_upload".to_string(),
                inputs: BTreeMap::from([(
                    "local_file".to_string(),
                    serde_json::Value::String("/tmp/example.txt".to_string()),
                )]),
                runtime: BTreeMap::from([(
                    "access_token".to_string(),
                    serde_json::Value::String("token-300".to_string()),
                )]),
            }),
        )
        .await
        .expect("browser flow dry run should succeed");

        assert_eq!(payload.provider, "unicom");
        assert_eq!(payload.surface, "pan.wo.cn-web");
        assert_eq!(payload.flow_id, "unicom_personal_root_upload");
        assert_eq!(payload.report.flow_id, "unicom_personal_root_upload");
        assert_eq!(payload.report.step_count, 4);
        assert_eq!(payload.report.steps[0].step_id, "attach-local-file");
        assert_eq!(
            payload.report.steps[2].detail.get("request_method"),
            Some(&serde_json::Value::String("POST".to_string()))
        );
    }

    #[tokio::test]
    async fn browser_flow_dry_run_rejects_missing_required_input() {
        let state = test_state();

        let error = run_browser_flow_dry_run(
            State(state),
            Json(BrowserFlowDryRunInput {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
                flow_id: "unicom_sms_login".to_string(),
                inputs: BTreeMap::new(),
                runtime: BTreeMap::new(),
            }),
        )
        .await
        .expect_err("browser flow dry run should reject missing required input");

        assert_eq!(error.into_response().status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn auth_capture_prompts_fill_missing_browser_flow_inputs() {
        let state = test_state();
        let session = upsert_browser_flow_auth_session(
            &state,
            Some("auth-session-1".to_string()),
            "unicom".to_string(),
            "pan.wo.cn-web".to_string(),
            "unicom_sms_login".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let (_, flow) =
            browser_flow_catalog_and_flow(&state, "unicom", "pan.wo.cn-web", "unicom_sms_login")
                .expect("flow should exist");
        let missing_inputs = missing_required_browser_flow_inputs(flow, &session.inputs);
        let prompts = ensure_auth_capture_prompts_for_inputs(&state, &session, &missing_inputs);

        assert_eq!(prompts.len(), 2);
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.input_id.as_deref() == Some("phone_number"))
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.input_id.as_deref() == Some("sms_code"))
        );

        let mut store = state
            .auth
            .capture_prompts
            .lock()
            .expect("auth capture prompt store poisoned");
        for prompt in store.values_mut() {
            if prompt.session_id.as_deref() != Some("auth-session-1") {
                continue;
            }
            match prompt.input_id.as_deref() {
                Some("phone_number") => prompt.answer("18513581767".to_string()),
                Some("sms_code") => prompt.answer("288750".to_string()),
                _ => {}
            }
        }
        drop(store);

        let mut merged = BTreeMap::new();
        merge_answered_prompts_into_inputs(&state, "auth-session-1", &mut merged);
        assert_eq!(
            merged.get("phone_number"),
            Some(&serde_json::Value::String("18513581767".to_string()))
        );
        assert_eq!(
            merged.get("sms_code"),
            Some(&serde_json::Value::String("288750".to_string()))
        );
    }

    #[tokio::test]
    async fn browser_flow_session_run_returns_prompts_when_inputs_are_missing() {
        let state = test_state();

        let Json(payload) = run_browser_flow_session(
            State(state.clone()),
            Json(BrowserFlowSessionRunInput {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
                flow_id: "unicom_sms_login".to_string(),
                auth_session_id: Some("auth-session-2".to_string()),
                inputs: BTreeMap::new(),
                runtime: BTreeMap::new(),
                cdp_endpoint_url: None,
                cdp_target_selector: None,
                cdp_target_timeout_ms: None,
            }),
        )
        .await
        .expect("session run should return prompt payload");

        assert_eq!(payload.status, "awaiting_input");
        assert_eq!(payload.auth_session_id.as_deref(), Some("auth-session-2"));
        assert!(payload.report.is_none());
        assert_eq!(payload.prompts.len(), 2);
        assert!(payload.cdp_endpoint_url.is_empty());

        let Json(prompts) = list_auth_capture_prompts(State(state.clone()))
            .await
            .expect("prompt list should succeed");
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.session_id.as_deref() == Some("auth-session-2"))
        );

        let Json(session_payload) =
            get_browser_flow_session(State(state), Path("auth-session-2".to_string()))
                .await
                .expect("session lookup should succeed");
        assert_eq!(
            session_payload.status,
            BROWSER_FLOW_AUTH_SESSION_STATUS_AWAITING_INPUT
        );
        assert_eq!(session_payload.prompts.len(), 2);
        assert!(session_payload.report.is_none());
    }

    #[tokio::test]
    async fn replying_to_prompt_marks_browser_flow_session_answered() {
        let state = test_state();
        let session = upsert_browser_flow_auth_session(
            &state,
            Some("auth-session-3".to_string()),
            "unicom".to_string(),
            "pan.wo.cn-web".to_string(),
            "unicom_sms_login".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let (_, flow) =
            browser_flow_catalog_and_flow(&state, "unicom", "pan.wo.cn-web", "unicom_sms_login")
                .expect("flow should exist");
        let missing_inputs = missing_required_browser_flow_inputs(flow, &session.inputs);
        let prompts = ensure_auth_capture_prompts_for_inputs(&state, &session, &missing_inputs);
        let phone_prompt = prompts
            .iter()
            .find(|prompt| prompt.input_id.as_deref() == Some("phone_number"))
            .expect("phone prompt should exist");

        let Json(reply) = reply_auth_capture_prompt(
            State(state.clone()),
            Path(phone_prompt.prompt_id.clone()),
            Json(AuthCapturePromptReplyInput {
                value: "18513581767".to_string(),
            }),
        )
        .await
        .expect("reply should succeed");
        assert_eq!(reply.status, "answered");

        let Json(session_payload) =
            get_browser_flow_session(State(state), Path("auth-session-3".to_string()))
                .await
                .expect("session lookup should succeed");
        assert_eq!(
            session_payload.status,
            BROWSER_FLOW_AUTH_SESSION_STATUS_ANSWERED
        );
    }

    #[tokio::test]
    async fn browser_flow_session_executor_runs_upload_flow_with_mock_session() {
        let state = test_state();
        let session = RecordingBrowserFlowSession::default();

        let report = execute_browser_flow_session(
            state.browser_flow_catalogs.as_ref(),
            "unicom",
            "pan.wo.cn-web",
            "unicom_personal_root_upload",
            BTreeMap::from([(
                "local_file".to_string(),
                serde_json::Value::String("/tmp/example.txt".to_string()),
            )]),
            BTreeMap::from([(
                "access_token".to_string(),
                serde_json::Value::String("token-300".to_string()),
            )]),
            RecordingBrowserFlowSession {
                actions: session.actions.clone(),
            },
        )
        .await
        .expect("browser flow session run should succeed");

        assert_eq!(report.flow_id, "unicom_personal_root_upload");
        assert_eq!(report.step_count, 4);
        assert_eq!(
            session.actions(),
            vec![
                "set_files:file_list.global_uploader_input:/tmp/example.txt".to_string(),
                "dispatch_events:file_list.global_uploader_input:input,change".to_string(),
                "wait_for_request:upload2c:60000".to_string(),
                "wait_for_request:wohome_query_all_files:30000".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn capture_browser_flow_outputs_reads_script_values_and_url() {
        let state = test_state();
        let plan = browser_flow_plan(
            state.browser_flow_catalogs.as_ref(),
            "unicom",
            "pan.wo.cn-web",
            "unicom_sms_login",
            BTreeMap::from([
                (
                    "phone_number".to_string(),
                    serde_json::Value::String("18513581767".to_string()),
                ),
                (
                    "sms_code".to_string(),
                    serde_json::Value::String("288750".to_string()),
                ),
            ]),
            BTreeMap::new(),
        )
        .expect("login flow plan should bind");
        let reader = RecordingBrowserFlowOutputReader {
            values: Arc::new(Mutex::new(HashMap::from([
                (
                    "(() => { const listVm = document.querySelector('.file-list-container')?.__vue__ || document.querySelector('.file-list-container')?.__vueParentComponent?.proxy; return listVm?.$store?.state?.user?.token ?? null; })()".to_string(),
                    serde_json::Value::String("token-123".to_string()),
                ),
                (
                    "(() => sessionStorage.getItem('familyId') || null)()".to_string(),
                    serde_json::Value::String("family-42".to_string()),
                ),
                (
                    "(() => { const listVm = document.querySelector('.file-list-container')?.__vue__ || document.querySelector('.file-list-container')?.__vueParentComponent?.proxy; return listVm?.$store?.state?.user?.clientId ?? null; })()".to_string(),
                    serde_json::Value::String("1001000021".to_string()),
                ),
            ]))),
            current_url: Arc::new(Mutex::new(Some(
                "https://pan.wo.cn/pan/file_list/all".to_string(),
            ))),
        };

        let captured = capture_browser_flow_outputs(&plan, &reader)
            .await
            .expect("output capture should succeed");

        assert_eq!(
            captured.get("access_token"),
            Some(&serde_json::Value::String("token-123".to_string()))
        );
        assert_eq!(
            captured.get("family_id"),
            Some(&serde_json::Value::String("family-42".to_string()))
        );
        assert_eq!(
            captured.get("client_id"),
            Some(&serde_json::Value::String("1001000021".to_string()))
        );
        assert_eq!(
            captured.get("current_url"),
            Some(&serde_json::Value::String(
                "https://pan.wo.cn/pan/file_list/all".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn capture_browser_flow_outputs_reads_prepare_upload_runtime_values() {
        let state = test_state();
        let plan = browser_flow_plan(
            state.browser_flow_catalogs.as_ref(),
            "unicom",
            "pan.wo.cn-web",
            "unicom_prepare_personal_root_upload",
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .expect("prepare upload flow plan should bind");
        let reader = RecordingBrowserFlowOutputReader {
            values: Arc::new(Mutex::new(HashMap::from([
                (
                    "(() => { const root = document.querySelector('.uploader-container'); const vm = root && (root.__vue__ || root.__vueParentComponent?.proxy); return vm?.params?.fileInfo?.batchNo ?? null; })()".to_string(),
                    serde_json::Value::String("batch-123".to_string()),
                ),
                (
                    "(() => { const root = document.querySelector('.uploader-container'); const vm = root && (root.__vue__ || root.__vueParentComponent?.proxy); return vm?.params?.fileInfo?.directoryId ?? null; })()".to_string(),
                    serde_json::Value::String("0".to_string()),
                ),
                (
                    "(() => { const root = document.querySelector('.uploader-container'); const vm = root && (root.__vue__ || root.__vueParentComponent?.proxy); return vm?.params?.fileInfo?.spaceType ?? null; })()".to_string(),
                    serde_json::Value::String("0".to_string()),
                ),
            ]))),
            current_url: Arc::new(Mutex::new(None)),
        };

        let captured = capture_browser_flow_outputs(&plan, &reader)
            .await
            .expect("prepare upload output capture should succeed");

        assert_eq!(
            captured.get("batch_no"),
            Some(&serde_json::Value::String("batch-123".to_string()))
        );
        assert_eq!(
            captured.get("directory_id"),
            Some(&serde_json::Value::String("0".to_string()))
        );
        assert_eq!(
            captured.get("personal_space_type"),
            Some(&serde_json::Value::String("0".to_string()))
        );
    }

    #[tokio::test]
    async fn capture_browser_flow_outputs_reads_current_session_runtime_values() {
        let state = test_state();
        let plan = browser_flow_plan(
            state.browser_flow_catalogs.as_ref(),
            "unicom",
            "pan.wo.cn-web",
            "unicom_capture_current_session",
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .expect("capture current session flow plan should bind");
        let reader = RecordingBrowserFlowOutputReader {
            values: Arc::new(Mutex::new(HashMap::from([
                (
                    "(() => { const listVm = document.querySelector('.file-list-container')?.__vue__ || document.querySelector('.file-list-container')?.__vueParentComponent?.proxy; return listVm?.$store?.state?.user?.token ?? null; })()".to_string(),
                    serde_json::Value::String("token-current".to_string()),
                ),
                (
                    "(() => sessionStorage.getItem('familyId') || null)()".to_string(),
                    serde_json::Value::String("family-current".to_string()),
                ),
                (
                    "(() => { const listVm = document.querySelector('.file-list-container')?.__vue__ || document.querySelector('.file-list-container')?.__vueParentComponent?.proxy; return listVm?.$store?.state?.user?.clientId ?? null; })()".to_string(),
                    serde_json::Value::String("1001000021".to_string()),
                ),
            ]))),
            current_url: Arc::new(Mutex::new(Some(
                "https://pan.wo.cn/pan/file_list/all".to_string(),
            ))),
        };

        let captured = capture_browser_flow_outputs(&plan, &reader)
            .await
            .expect("current session output capture should succeed");

        assert_eq!(
            captured.get("access_token"),
            Some(&serde_json::Value::String("token-current".to_string()))
        );
        assert_eq!(
            captured.get("family_id"),
            Some(&serde_json::Value::String("family-current".to_string()))
        );
        assert_eq!(
            captured.get("client_id"),
            Some(&serde_json::Value::String("1001000021".to_string()))
        );
        assert_eq!(
            captured.get("current_url"),
            Some(&serde_json::Value::String(
                "https://pan.wo.cn/pan/file_list/all".to_string()
            ))
        );
    }

    #[test]
    fn browser_flow_auth_session_runtime_is_reused_by_follow_up_flow() {
        let state = test_state();
        let session = upsert_browser_flow_auth_session(
            &state,
            Some("auth-session-runtime".to_string()),
            "unicom".to_string(),
            "pan.wo.cn-web".to_string(),
            "unicom_sms_login".to_string(),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let merged = merge_browser_flow_auth_session_runtime(
            &state,
            &session.session_id,
            BTreeMap::from([(
                "access_token".to_string(),
                serde_json::Value::String("token-from-login".to_string()),
            )]),
        )
        .expect("runtime merge should succeed");
        assert_eq!(
            merged.runtime.get("access_token"),
            Some(&serde_json::Value::String("token-from-login".to_string()))
        );

        let plan = browser_flow_plan(
            state.browser_flow_catalogs.as_ref(),
            "unicom",
            "pan.wo.cn-web",
            "unicom_create_directory",
            BTreeMap::from([(
                "directory_name".to_string(),
                serde_json::Value::String("probe-dir".to_string()),
            )]),
            merged.runtime.clone(),
        )
        .expect("follow-up flow should bind with reused runtime");

        let request = plan
            .find_request("wohome_create_directory")
            .expect("create-directory request should exist");
        let access_header = request
            .required_headers
            .iter()
            .find(|header| header.name == "accesstoken")
            .expect("accesstoken header should exist");
        assert_eq!(
            access_header.value_template.as_deref(),
            Some("token-from-login")
        );
    }

    #[test]
    fn upload_flow_prerequisite_is_detected_from_runtime_state() {
        let state = test_state();
        let (_, upload_flow) = browser_flow_catalog_and_flow(
            &state,
            "unicom",
            "pan.wo.cn-web",
            "unicom_personal_root_upload",
        )
        .expect("upload flow should exist");
        let (_, prepare_flow) = browser_flow_catalog_and_flow(
            &state,
            "unicom",
            "pan.wo.cn-web",
            "unicom_prepare_personal_root_upload",
        )
        .expect("prepare flow should exist");
        let (_, capture_flow) = browser_flow_catalog_and_flow(
            &state,
            "unicom",
            "pan.wo.cn-web",
            "unicom_capture_current_session",
        )
        .expect("capture flow should exist");

        assert!(!browser_flow_prerequisite_is_satisfied(
            upload_flow,
            prepare_flow,
            &BTreeMap::from([(
                "access_token".to_string(),
                serde_json::Value::String("token-300".to_string()),
            )]),
        ));

        assert!(browser_flow_prerequisite_is_satisfied(
            upload_flow,
            prepare_flow,
            &BTreeMap::from([
                (
                    "batch_no".to_string(),
                    serde_json::Value::String("batch-123".to_string()),
                ),
                (
                    "directory_id".to_string(),
                    serde_json::Value::String("0".to_string()),
                ),
                (
                    "personal_space_type".to_string(),
                    serde_json::Value::String("0".to_string()),
                ),
            ]),
        ));

        assert!(!browser_flow_prerequisite_is_satisfied(
            prepare_flow,
            capture_flow,
            &BTreeMap::new(),
        ));

        assert!(browser_flow_prerequisite_is_satisfied(
            prepare_flow,
            capture_flow,
            &BTreeMap::from([
                (
                    "access_token".to_string(),
                    serde_json::Value::String("token-300".to_string()),
                ),
                (
                    "family_id".to_string(),
                    serde_json::Value::String("family-42".to_string()),
                ),
                (
                    "client_id".to_string(),
                    serde_json::Value::String("1001000021".to_string()),
                ),
                (
                    "current_url".to_string(),
                    serde_json::Value::String("https://pan.wo.cn/pan/file_list/all".to_string()),
                ),
            ]),
        ));
    }

    #[test]
    fn browser_flow_session_run_uses_request_or_policy_cdp_config() {
        let state = test_state();
        {
            let mut control_plane = state.control_plane.lock().expect("control plane poisoned");
            control_plane.auth_capture_policy.cdp_endpoint_url =
                Some("http://127.0.0.1:9222".to_string());
            control_plane.auth_capture_policy.cdp_target_selector =
                Some("url:https://pan.wo.cn/*".to_string());
            control_plane.auth_capture_policy.cdp_target_timeout_ms = Some(15000);
        }

        let fallback = resolve_browser_flow_cdp_config(
            &state,
            &BrowserFlowSessionRunInput {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
                flow_id: "unicom_sms_login".to_string(),
                auth_session_id: None,
                inputs: BTreeMap::new(),
                runtime: BTreeMap::new(),
                cdp_endpoint_url: None,
                cdp_target_selector: None,
                cdp_target_timeout_ms: None,
            },
        )
        .expect("policy-backed cdp config should resolve");
        assert_eq!(fallback.endpoint_url, "http://127.0.0.1:9222");
        assert_eq!(
            fallback.target_selector.as_deref(),
            Some("url:https://pan.wo.cn/*")
        );
        assert_eq!(fallback.target_timeout_ms, Some(15000));

        let override_config = resolve_browser_flow_cdp_config(
            &state,
            &BrowserFlowSessionRunInput {
                provider: "unicom".to_string(),
                surface: "pan.wo.cn-web".to_string(),
                flow_id: "unicom_sms_login".to_string(),
                auth_session_id: None,
                inputs: BTreeMap::new(),
                runtime: BTreeMap::new(),
                cdp_endpoint_url: Some("http://127.0.0.1:9333".to_string()),
                cdp_target_selector: Some("title:pan.wo.cn".to_string()),
                cdp_target_timeout_ms: Some(9000),
            },
        )
        .expect("request override cdp config should resolve");
        assert_eq!(override_config.endpoint_url, "http://127.0.0.1:9333");
        assert_eq!(
            override_config.target_selector.as_deref(),
            Some("title:pan.wo.cn")
        );
        assert_eq!(override_config.target_timeout_ms, Some(9000));
    }

    #[tokio::test]
    async fn auth_capture_policy_round_trip_preserves_cdp_fields() {
        let state = test_state();

        let Json(saved) = update_auth_capture_policy(
            State(state.clone()),
            Json(AuthCapturePolicyInput {
                enabled: true,
                broker_url: Some("http://auth-broker.internal:61200".to_string()),
                cdp_endpoint_url: Some("http://127.0.0.1:9222".to_string()),
                cdp_target_selector: Some("url:https://pan.wo.cn/*".to_string()),
                cdp_target_timeout_ms: Some(15000),
                llm_analysis_enabled: true,
                llm_endpoint: Some("http://llm.internal:1234/v1".to_string()),
                llm_model_id: Some("test-model".to_string()),
                llm_api_key: Some("rotate-me".to_string()),
                clear_llm_api_key: false,
            }),
        )
        .await
        .expect("auth capture policy update should succeed");

        assert!(saved.enabled);
        assert_eq!(
            saved.cdp_endpoint_url.as_deref(),
            Some("http://127.0.0.1:9222")
        );
        assert_eq!(
            saved.cdp_target_selector.as_deref(),
            Some("url:https://pan.wo.cn/*")
        );
        assert_eq!(saved.cdp_target_timeout_ms, Some(15000));
        assert!(saved.llm_api_key_present);

        let Json(loaded) = get_auth_capture_policy(State(state))
            .await
            .expect("auth capture policy lookup should succeed");
        assert_eq!(loaded.cdp_endpoint_url, saved.cdp_endpoint_url);
        assert_eq!(loaded.cdp_target_selector, saved.cdp_target_selector);
        assert_eq!(loaded.cdp_target_timeout_ms, saved.cdp_target_timeout_ms);
    }

    #[test]
    fn web_login_url_contains_pkce_and_scope() {
        let state = "state-token";
        let verifier = "verifier-token";
        let url = build_onedrive_authorize_url(
            &test_config().onedrive,
            state,
            &pkce_code_challenge(verifier),
        )
        .expect("auth url should build");

        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state%2Dtoken"));
        assert!(url.contains("scope="));
    }

    #[test]
    fn auth_status_reads_session_file_metadata() {
        let mut state = test_state();
        let session_file = temp_db_path().replace(".db", "-onedrive.json");
        persist_oauth_session(
            &session_file,
            &OneDriveOAuthSession {
                access_token: "session-token".to_string(),
                refresh_token: Some("refresh-token".to_string()),
                token_type: "Bearer".to_string(),
                scope: Some(DEFAULT_ONEDRIVE_SCOPES.to_string()),
                expires_at_unix: Some(current_unix_ms() / 1000 + 3600),
            },
        )
        .expect("session should persist");
        Arc::make_mut(&mut state.config).onedrive.session_file = Some(session_file.clone());

        let auth = read_onedrive_auth_status(&state);
        assert_eq!(auth.token_state, "session_ready");
        assert!(auth.has_refresh_token);
        assert_eq!(auth.session_file.as_deref(), Some(session_file.as_str()));

        let _ = fs::remove_file(session_file);
    }

    #[tokio::test]
    async fn provider_credentials_round_trip_and_hot_reload_onedrive() {
        let mut state = test_state();
        replace_backend(
            &mut state,
            ProviderId::Onedrive,
            Arc::new(FailingBackend::new("stale-onedrive", "stale backend")),
        );

        let Json(saved) = update_provider_credentials(
            State(state.clone()),
            Path("onedrive".to_string()),
            Json(ProviderCredentialInput {
                token: Some("manual-override-token".to_string()),
                browser_id: None,
                cookie_header: None,
                family_id: None,
                root_folder_id: None,
                client_id: Some("override-client".to_string()),
                tenant: Some("organizations".to_string()),
                drive_id: Some("override-drive".to_string()),
                redirect_url: Some("http://127.0.0.1:61082/auth/onedrive/callback".to_string()),
            }),
        )
        .await
        .expect("provider credentials update should succeed");

        assert_eq!(saved.provider, "onedrive");
        assert_eq!(saved.token.as_deref(), Some("manual-override-token"));
        assert_eq!(saved.client_id.as_deref(), Some("override-client"));
        assert_eq!(saved.tenant.as_deref(), Some("organizations"));
        assert_eq!(saved.drive_id.as_deref(), Some("override-drive"));
        assert_eq!(
            backend_for_test(&state, ProviderId::Onedrive).name(),
            "onedrive"
        );
        assert_eq!(
            read_onedrive_auth_status(&state).token_state,
            "inline_token"
        );

        let Json(current) =
            get_provider_credentials(State(state.clone()), Path("onedrive".to_string()))
                .await
                .expect("provider credentials get should succeed");
        assert_eq!(current.token.as_deref(), Some("manual-override-token"));

        let credential_path = provider_credentials_path(&state.config, ProviderId::Onedrive);
        let stored = fs::read_to_string(&credential_path).expect("credential file should exist");
        assert!(stored.contains("manual-override-token"));
    }

    #[tokio::test]
    async fn provider_credentials_round_trip_and_hot_reload_unicom_family_id() {
        let state = test_state();

        let Json(saved) = update_provider_credentials(
            State(state.clone()),
            Path("unicom".to_string()),
            Json(ProviderCredentialInput {
                token: Some("manual-unicom-token".to_string()),
                browser_id: None,
                cookie_header: Some("foo=bar; session=1".to_string()),
                family_id: Some(" family-123 ".to_string()),
                root_folder_id: None,
                client_id: None,
                tenant: None,
                drive_id: None,
                redirect_url: None,
            }),
        )
        .await
        .expect("unicom provider credentials update should succeed");

        assert_eq!(saved.provider, "unicom");
        assert_eq!(saved.token.as_deref(), Some("manual-unicom-token"));
        assert_eq!(saved.cookie_header.as_deref(), Some("foo=bar; session=1"));
        assert_eq!(saved.family_id.as_deref(), Some("family-123"));
        assert_eq!(
            backend_for_test(&state, ProviderId::Unicom).name(),
            "unicom-cloud-drive"
        );

        let Json(current) =
            get_provider_credentials(State(state.clone()), Path("unicom".to_string()))
                .await
                .expect("unicom provider credentials get should succeed");
        assert_eq!(current.family_id.as_deref(), Some("family-123"));
        assert_eq!(current.token.as_deref(), Some("manual-unicom-token"));

        let credential_path = provider_credentials_path(&state.config, ProviderId::Unicom);
        let stored = fs::read_to_string(&credential_path).expect("credential file should exist");
        assert!(stored.contains("manual-unicom-token"));
        assert!(stored.contains("family-123"));
    }

    #[tokio::test]
    async fn object_actions_api_rename_copy_and_move_against_primary_backend() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed family object should be created");

        let rename_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Rename {
                bucket: "family".to_string(),
                key: "shared/note.txt".to_string(),
                new_key: "shared/renamed.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("rename should succeed");
        assert_eq!(rename_status, StatusCode::NO_CONTENT);

        let copy_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Copy {
                source_bucket: "family".to_string(),
                source_key: "shared/renamed.txt".to_string(),
                destination_bucket: "root".to_string(),
                destination_key: "docs/copied.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("copy should succeed");
        assert_eq!(copy_status, StatusCode::NO_CONTENT);

        let move_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Move {
                source_bucket: "root".to_string(),
                source_key: "docs/copied.txt".to_string(),
                destination_bucket: "family".to_string(),
                destination_key: "shared/moved.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("move should succeed");
        assert_eq!(move_status, StatusCode::NO_CONTENT);

        assert!(
            primary_backend
                .get_object("family", "shared/renamed.txt")
                .await
                .is_ok()
        );
        assert!(
            primary_backend
                .get_object("family", "shared/moved.txt")
                .await
                .is_ok()
        );
        assert!(
            primary_backend
                .get_object("root", "docs/copied.txt")
                .await
                .is_err()
        );

        let rename_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "family", "shared/renamed.txt")
            .expect("rename metadata should load")
            .expect("rename metadata should exist");
        assert!(matches!(rename_job.operation, ReplicationOperation::Put));
        assert_eq!(rename_job.source_provider.as_deref(), Some("stub"));
        assert_eq!(rename_job.object.size, Some(11));

        let renamed_delete_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "family", "shared/note.txt")
            .expect("rename delete metadata should load")
            .expect("rename delete metadata should exist");
        assert!(matches!(
            renamed_delete_job.operation,
            ReplicationOperation::Delete
        ));

        let copied_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "docs/copied.txt")
            .expect("copy metadata should load")
            .expect("copy metadata should exist");
        assert!(matches!(copied_job.operation, ReplicationOperation::Delete));

        let moved_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "family", "shared/moved.txt")
            .expect("move metadata should load")
            .expect("move metadata should exist");
        assert!(matches!(moved_job.operation, ReplicationOperation::Put));
        assert_eq!(moved_job.object.content_type.as_deref(), Some("text/plain"));
    }

    #[tokio::test]
    async fn object_action_history_persists_and_can_be_cleared() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed family object should be created");

        let rename_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Rename {
                bucket: "family".to_string(),
                key: "shared/note.txt".to_string(),
                new_key: "shared/renamed.txt".to_string(),
                operator: Some("alice".to_string()),
                ticket: Some("CHG-1".to_string()),
                notes: Some("rename for test".to_string()),
            }),
        )
        .await
        .expect("rename should succeed");
        assert_eq!(rename_status, StatusCode::NO_CONTENT);

        let history = control_plane_snapshot(&state).object_action_history;
        assert_eq!(history.len(), 1);
        let entry = &history[0];
        assert_eq!(entry.action, "rename");
        assert_eq!(entry.outcome, "success");
        assert_eq!(entry.primary_provider, "stub");
        assert_eq!(
            entry.description,
            "family/shared/note.txt -> family/shared/renamed.txt"
        );
        assert_eq!(entry.operator.as_deref(), Some("alice"));
        assert_eq!(entry.ticket.as_deref(), Some("CHG-1"));
        assert_eq!(entry.notes.as_deref(), Some("rename for test"));
        assert!(entry.references.iter().any(|reference| {
            reference.bucket == "family"
                && reference.key == "shared/note.txt"
                && !reference.changes.is_empty()
        }));
        assert!(entry.references.iter().any(|reference| {
            reference.bucket == "family"
                && reference.key == "shared/renamed.txt"
                && !reference.changes.is_empty()
        }));

        let persisted = load_control_plane_state(
            &state.config.control_plane_file,
            ControlPlaneState {
                topology: state.config.topology.clone(),
                onedrive_policy: OnedrivePolicy::from_env_defaults(&state.config.topology),
                auth_capture_policy: AuthCapturePolicy::from_env_defaults(),
                object_action_history: Vec::new(),
            },
            state.config.onedrive.enabled,
        )
        .expect("persisted control plane should load");
        assert_eq!(persisted.object_action_history.len(), 1);
        assert_eq!(persisted.object_action_history[0].action, "rename");

        let clear_status = clear_object_action_history_api(State(state.clone()))
            .await
            .expect("history clear should succeed");
        assert_eq!(clear_status, StatusCode::NO_CONTENT);
        assert!(
            control_plane_snapshot(&state)
                .object_action_history
                .is_empty()
        );

        let cleared = load_control_plane_state(
            &state.config.control_plane_file,
            ControlPlaneState {
                topology: state.config.topology.clone(),
                onedrive_policy: OnedrivePolicy::from_env_defaults(&state.config.topology),
                auth_capture_policy: AuthCapturePolicy::from_env_defaults(),
                object_action_history: Vec::new(),
            },
            state.config.onedrive.enabled,
        )
        .expect("cleared control plane should load");
        assert!(cleared.object_action_history.is_empty());
    }

    #[tokio::test]
    async fn object_action_history_respects_configured_limit() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).object_action_history_limit = 2;
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed family object should be created");

        run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Rename {
                bucket: "family".to_string(),
                key: "shared/note.txt".to_string(),
                new_key: "shared/renamed.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("rename should succeed");

        run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Copy {
                source_bucket: "family".to_string(),
                source_key: "shared/renamed.txt".to_string(),
                destination_bucket: "root".to_string(),
                destination_key: "docs/copied.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("copy should succeed");

        run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Move {
                source_bucket: "root".to_string(),
                source_key: "docs/copied.txt".to_string(),
                destination_bucket: "family".to_string(),
                destination_key: "shared/moved.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("move should succeed");

        let history = control_plane_snapshot(&state).object_action_history;
        assert_eq!(history.len(), 2);
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.action.as_str())
                .collect::<Vec<_>>(),
            vec!["move", "copy"]
        );

        let persisted = load_control_plane_state(
            &state.config.control_plane_file,
            ControlPlaneState {
                topology: state.config.topology.clone(),
                onedrive_policy: OnedrivePolicy::from_env_defaults(&state.config.topology),
                auth_capture_policy: AuthCapturePolicy::from_env_defaults(),
                object_action_history: Vec::new(),
            },
            state.config.onedrive.enabled,
        )
        .expect("persisted control plane should load");
        assert_eq!(persisted.object_action_history.len(), 2);
        assert_eq!(persisted.object_action_history[0].action, "move");
        assert_eq!(persisted.object_action_history[1].action, "copy");
    }

    #[tokio::test]
    async fn object_action_noop_does_not_enqueue_replication_jobs() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("seed family object should be created");

        let rename_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Rename {
                bucket: "family".to_string(),
                key: "shared/note.txt".to_string(),
                new_key: "shared/note.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("noop rename should succeed");
        assert_eq!(rename_status, StatusCode::NO_CONTENT);

        let move_status = run_object_action(
            State(state.clone()),
            Json(ObjectActionInput::Move {
                source_bucket: "family".to_string(),
                source_key: "shared/note.txt".to_string(),
                destination_bucket: "family".to_string(),
                destination_key: "shared/note.txt".to_string(),
                operator: None,
                ticket: None,
                notes: None,
            }),
        )
        .await
        .expect("noop move should succeed");
        assert_eq!(move_status, StatusCode::NO_CONTENT);

        assert_eq!(state.replication.snapshot().pending_count, 0);
        assert!(
            state
                .metadata_store
                .snapshot(16)
                .expect("metadata snapshot should load")
                .recent_jobs
                .is_empty()
        );
    }

    #[tokio::test]
    async fn admin_page_exposes_object_actions_panel() {
        let state = test_state();

        let response = admin_index(State(state)).await.into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("admin page body should read");
        let html = String::from_utf8(body.to_vec()).expect("admin page should be utf-8");

        assert!(html.contains("<h2>Runtime</h2>"));
        assert!(html.contains("id=\"runtime-summary\""));
        assert!(html.contains("id=\"runtime-json\""));
        assert!(html.contains("<h2>Monitoring Summary</h2>"));
        assert!(html.contains("id=\"monitoring-summary\""));
        assert!(html.contains("id=\"monitoring-failures\""));
        assert!(html.contains("renderMonitoringSummary"));
        assert!(html.contains("<h2>Operations Overview</h2>"));
        assert!(html.contains("id=\"operations-overview\""));
        assert!(html.contains("id=\"operations-overview-notes\""));
        assert!(html.contains("renderOperationsOverview"));
        assert!(html.contains("id=\"replication-feedback\""));
        assert!(html.contains("id=\"replication-failed-target-filter\""));
        assert!(html.contains("id=\"replication-failed-object-filter\""));
        assert!(html.contains("id=\"replication-failed-start-filter\""));
        assert!(html.contains("id=\"replication-failed-end-filter\""));
        assert!(html.contains("id=\"replication-failed-summary\""));
        assert!(html.contains("id=\"replication-failed\""));
        assert!(html.contains("id=\"export-replication-failed-json\""));
        assert!(html.contains("id=\"export-replication-failed-csv\""));
        assert!(html.contains("data-retry-replication-job"));
        assert!(html.contains("retryReplicationJob"));
        assert!(html.contains("data-retry-replication-target"));
        assert!(html.contains("retryFailedReplicationTarget"));
        assert!(html.contains("filteredReplicationFailedJobs"));
        assert!(html.contains("downloadReplicationFailedJobs"));
        assert!(html.contains("downloadReplicationFailedJobsCsv"));
        assert!(html.contains("id=\"status-auto-refresh-enabled\""));
        assert!(html.contains("id=\"status-auto-refresh-interval-seconds\""));
        assert!(html.contains("id=\"status-refresh-summary\""));
        assert!(html.contains("<h2>Object Actions</h2>"));
        assert!(html.contains("id=\"object-action-kind\""));
        assert!(html.contains("id=\"run-object-action\""));
        assert!(html.contains("runObjectAction"));
        assert!(html.contains("id=\"object-action-operator\""));
        assert!(html.contains("id=\"object-action-ticket\""));
        assert!(html.contains("id=\"object-action-notes\""));
        assert!(html.contains("id=\"object-action-preview-summary\""));
        assert!(html.contains("id=\"object-action-inspection-summary\""));
        assert!(html.contains("renderObjectActionInspection"));
        assert!(html.contains("renderProviderStateComparisons"));
        assert!(html.contains("No before/after object inspection captured yet."));
        assert!(html.contains("id=\"object-action-history-summary\""));
        assert!(html.contains("id=\"object-action-history-action-filter\""));
        assert!(html.contains("id=\"object-action-history-outcome-filter\""));
        assert!(html.contains("id=\"object-action-history-provider-filter\""));
        assert!(html.contains("id=\"object-action-history-operator-filter\""));
        assert!(html.contains("id=\"object-action-history-object-filter\""));
        assert!(html.contains("id=\"object-action-history-start-filter\""));
        assert!(html.contains("id=\"object-action-history-end-filter\""));
        assert!(html.contains("renderObjectActionHistory"));
        assert!(html.contains("startStatusAutoRefresh"));
        assert!(html.contains("stopStatusAutoRefresh"));
        assert!(html.contains("id=\"clear-object-action-history\""));
        assert!(html.contains("id=\"export-object-action-history\""));
        assert!(html.contains("id=\"export-object-action-history-csv\""));
        assert!(html.contains("Clear Shared History"));
        assert!(html.contains("downloadObjectActionHistory"));
        assert!(html.contains("downloadObjectActionHistoryCsv"));
        assert!(html.contains("Auto-refreshing dashboard"));
        assert!(html.contains("renderObjectActionHistory(status.object_action_history || [])"));
        assert!(!html.contains("function loadObjectActionHistory("));
        assert!(html.contains("Changes"));
        assert!(html.contains("Provider Changes"));
        assert!(html.contains("objectStatusDelta"));
        assert!(html.contains("Execution Preview"));
        assert!(
            html.contains(
                "Current Unicom rename only supports staying in the same parent directory"
            )
        );
        assert!(html.contains("/api/object-actions"));
    }

    #[tokio::test]
    async fn stub_provider_credentials_are_rejected() {
        let state = test_state();
        let error = get_provider_credentials(State(state), Path("stub".to_string()))
            .await
            .expect_err("stub credentials should be rejected");
        assert!(
            error
                .0
                .to_string()
                .contains("does not support credential storage")
        );
    }

    #[tokio::test]
    async fn replication_snapshot_includes_persisted_jobs() {
        let state = test_state();
        let bucket = "placeholder".to_string();
        let key = "notes/persisted.txt".to_string();
        let body = Bytes::from_static(b"persist me");
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");

        let put_headers = signed_headers(
            &state.config,
            &Method::PUT,
            &uri,
            &body,
            &[("content-type", "text/plain")],
        );
        put_object(
            State(state.clone()),
            Path((bucket, key)),
            Method::PUT,
            OriginalUri(uri),
            put_headers,
            body.into(),
        )
        .await
        .expect("put should succeed");

        let Json(snapshot) = replication_snapshot(State(state))
            .await
            .expect("replication snapshot should succeed");
        assert_eq!(snapshot.in_memory.pending_count, 1);
        assert_eq!(snapshot.in_memory.retry_scheduled_count, 0);
        assert_eq!(snapshot.persisted.pending_count, 1);
        assert_eq!(snapshot.persisted.retry_scheduled_count, 0);
        assert_eq!(snapshot.persisted.recent_jobs.len(), 1);
        assert_eq!(snapshot.target_statuses.len(), 1);
        assert_eq!(snapshot.target_statuses[0].provider, "onedrive");
        assert_eq!(snapshot.target_statuses[0].queued_count, 1);
        assert!(snapshot.latest_failed_jobs.is_empty());
    }

    #[tokio::test]
    async fn replication_snapshot_exposes_latest_failed_jobs_only() {
        let state = test_state();
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "failed/current.txt",
            Some(5),
            Some("text/plain"),
        );
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "failed/stale.txt",
            Some(5),
            Some("text/plain"),
        );
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "root",
            "failed/stale.txt",
            Some(5),
            Some("text/plain"),
        );

        let Json(snapshot) = replication_snapshot(State(state))
            .await
            .expect("replication snapshot should succeed");

        assert_eq!(snapshot.latest_failed_jobs.len(), 1);
        assert_eq!(snapshot.latest_failed_jobs[0].target, "onedrive");
        assert_eq!(
            snapshot.latest_failed_jobs[0].object.key,
            "failed/current.txt"
        );
    }

    #[tokio::test]
    async fn retry_replication_job_api_requeues_latest_failed_job() {
        let state = test_state();
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "retry/manual.txt",
            Some(5),
            Some("text/plain"),
        );

        let failed_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "retry/manual.txt")
            .expect("latest failed job should load")
            .expect("failed job should exist");
        assert!(matches!(failed_job.status, ReplicationStatus::Failed));

        let Json(payload) =
            retry_replication_job_api(State(state.clone()), Path(failed_job.job_id))
                .await
                .expect("retry api should succeed");
        assert_eq!(payload.job_id, failed_job.job_id);
        assert_eq!(payload.status, "pending");
        assert_eq!(payload.target, "onedrive");

        let snapshot = state.replication.snapshot();
        assert_eq!(snapshot.pending_count, 1);
        assert!(matches!(
            snapshot.pending_jobs[0].status,
            ReplicationStatus::Pending
        ));
        assert_eq!(snapshot.pending_jobs[0].job_id, failed_job.job_id);
        assert_eq!(snapshot.pending_jobs[0].attempts, 0);
        assert!(snapshot.pending_jobs[0].last_error.is_none());

        let persisted = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "retry/manual.txt")
            .expect("retried job should load")
            .expect("retried job should still exist");
        assert!(matches!(persisted.status, ReplicationStatus::Pending));
        assert_eq!(persisted.job_id, failed_job.job_id);
        assert_eq!(persisted.attempts, 0);
        assert!(persisted.last_error.is_none());
    }

    #[tokio::test]
    async fn retry_replication_job_api_rejects_non_failed_job() {
        let state = test_state();
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "root",
            "retry/completed.txt",
            Some(5),
            Some("text/plain"),
        );

        let completed_job = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "retry/completed.txt")
            .expect("latest completed job should load")
            .expect("completed job should exist");

        let error = retry_replication_job_api(State(state), Path(completed_job.job_id))
            .await
            .expect_err("retry api should reject non-failed job");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn retry_replication_target_api_requeues_only_latest_failed_jobs_for_target() {
        let state = test_state();
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "retry/batch-latest.txt",
            Some(5),
            Some("text/plain"),
        );
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "retry/batch-stale.txt",
            Some(5),
            Some("text/plain"),
        );
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "root",
            "retry/batch-stale.txt",
            Some(5),
            Some("text/plain"),
        );
        record_replication_state(
            &state,
            ProviderId::Telecom,
            ReplicationOperation::Put,
            ReplicationStatus::Failed,
            "root",
            "retry/other-target.txt",
            Some(5),
            Some("text/plain"),
        );

        let Json(payload) =
            retry_replication_target_api(State(state.clone()), Path("onedrive".to_string()))
                .await
                .expect("target retry api should succeed");

        assert_eq!(payload.target, "onedrive");
        assert_eq!(payload.retried_jobs, 1);
        assert_eq!(payload.jobs.len(), 1);
        assert_eq!(payload.jobs[0].key, "retry/batch-latest.txt");
        assert_eq!(payload.jobs[0].status, "pending");

        let snapshot = state.replication.snapshot();
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.pending_jobs.len(), 1);
        assert_eq!(snapshot.pending_jobs[0].target, "onedrive");
        assert_eq!(
            snapshot.pending_jobs[0].object.key,
            "retry/batch-latest.txt"
        );

        let latest_onedrive = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "retry/batch-latest.txt")
            .expect("latest onedrive job should load")
            .expect("latest onedrive job should exist");
        assert!(matches!(latest_onedrive.status, ReplicationStatus::Pending));

        let stale = state
            .metadata_store
            .latest_job_for_object("onedrive", "root", "retry/batch-stale.txt")
            .expect("stale onedrive job should load")
            .expect("stale onedrive job should exist");
        assert!(matches!(stale.status, ReplicationStatus::Completed));

        let telecom = state
            .metadata_store
            .latest_job_for_object("telecom", "root", "retry/other-target.txt")
            .expect("telecom job should load")
            .expect("telecom job should exist");
        assert!(matches!(telecom.status, ReplicationStatus::Failed));
    }

    #[test]
    fn replication_engine_next_job_id_advances_past_metadata_history() {
        let state = test_state();
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "root",
            "history/existing.txt",
            Some(5),
            Some("text/plain"),
        );

        let next_job_id = state
            .metadata_store
            .max_job_id()
            .expect("max job id should load")
            .unwrap_or(0)
            .saturating_add(1);
        state.replication.ensure_next_job_id_at_least(next_job_id);

        let jobs = state.replication.enqueue_put(
            &runtime_topology(&state),
            Some("stub".to_string()),
            "root",
            "history/new.txt",
            None,
            7,
            Some("text/plain".to_string()),
        );
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, next_job_id);
    }

    #[tokio::test]
    async fn object_lifecycle_round_trip_works_for_signed_requests() {
        let state = test_state();
        let bucket = "placeholder".to_string();
        let key = "notes/hello.txt".to_string();
        let body = Bytes::from_static(b"hello from gatewayd");
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");

        let put_headers = signed_headers(
            &state.config,
            &Method::PUT,
            &uri,
            &body,
            &[("content-type", "text/plain")],
        );
        let put_response = put_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::PUT,
            OriginalUri(uri.clone()),
            put_headers,
            body.clone().into(),
        )
        .await
        .expect("put object should succeed");
        assert_eq!(put_response.status(), StatusCode::OK);
        assert!(put_response.headers().contains_key(ETAG));
        assert_eq!(state.replication.snapshot().pending_count, 1);
        assert_eq!(
            state
                .metadata_store
                .snapshot(16)
                .expect("snapshot should load")
                .pending_count,
            1
        );

        let head_headers = signed_headers(&state.config, &Method::HEAD, &uri, &[], &[]);
        let head_response = head_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::HEAD,
            OriginalUri(uri.clone()),
            head_headers,
        )
        .await
        .expect("head object should succeed");
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_eq!(
            head_response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok()),
            Some("19")
        );

        let get_headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let get_response = get_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::GET,
            OriginalUri(uri.clone()),
            get_headers,
        )
        .await
        .expect("get object should succeed");
        assert_eq!(get_response.status(), StatusCode::OK);
        let get_body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .expect("get body should read");
        assert_eq!(get_body.as_ref(), b"hello from gatewayd");

        let delete_headers = signed_headers(&state.config, &Method::DELETE, &uri, &[], &[]);
        let delete_response = delete_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::DELETE,
            OriginalUri(uri.clone()),
            delete_headers,
        )
        .await
        .expect("delete object should succeed");
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(state.replication.snapshot().pending_count, 2);
        assert_eq!(
            state
                .metadata_store
                .snapshot(16)
                .expect("snapshot should load")
                .pending_count,
            1
        );

        let missing_headers = signed_headers(&state.config, &Method::HEAD, &uri, &[], &[]);
        let error_response = head_object(
            State(state),
            Path((bucket, key)),
            Method::HEAD,
            OriginalUri(uri),
            missing_headers,
        )
        .await
        .expect_err("deleted object should no longer exist")
        .into_response();
        assert_eq!(error_response.status(), StatusCode::NOT_FOUND);

        let error_body = to_bytes(error_response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let error_body =
            String::from_utf8(error_body.to_vec()).expect("error body should be valid utf-8");
        assert!(error_body.contains("<Code>NoSuchKey</Code>"));
    }

    #[tokio::test]
    async fn family_bucket_object_lifecycle_round_trip_works_for_signed_requests() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend);

        let bucket = "family".to_string();
        let key = "shared/gatewayd.txt".to_string();
        let body = Bytes::from_static(b"hello family bucket");
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");

        let put_headers = signed_headers(
            &state.config,
            &Method::PUT,
            &uri,
            &body,
            &[("content-type", "text/plain")],
        );
        let put_response = put_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::PUT,
            OriginalUri(uri.clone()),
            put_headers,
            body.clone().into(),
        )
        .await
        .expect("family put object should succeed");
        assert_eq!(put_response.status(), StatusCode::OK);

        let get_headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let get_response = get_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::GET,
            OriginalUri(uri.clone()),
            get_headers,
        )
        .await
        .expect("family get object should succeed");
        assert_eq!(get_response.status(), StatusCode::OK);
        let get_body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .expect("family get body should read");
        assert_eq!(get_body.as_ref(), b"hello family bucket");

        let delete_headers = signed_headers(&state.config, &Method::DELETE, &uri, &[], &[]);
        let delete_response = delete_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::DELETE,
            OriginalUri(uri.clone()),
            delete_headers,
        )
        .await
        .expect("family delete object should succeed");
        assert_eq!(delete_response.status(), StatusCode::NO_CONTENT);

        let head_headers = signed_headers(&state.config, &Method::HEAD, &uri, &[], &[]);
        let error_response = head_object(
            State(state),
            Path((bucket, key)),
            Method::HEAD,
            OriginalUri(uri),
            head_headers,
        )
        .await
        .expect_err("deleted family object should no longer exist")
        .into_response();
        assert_eq!(error_response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn object_put_accepts_unsigned_payload_streaming_body() {
        let state = test_state();
        let bucket = "placeholder".to_string();
        let key = "notes/streamed.txt".to_string();
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = unsigned_payload_headers(
            &state.config,
            &Method::PUT,
            &uri,
            14,
            &[("content-type", "text/plain")],
        );
        let body = Body::from_stream(stream::iter([
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"streamed ")),
            Ok::<Bytes, std::io::Error>(Bytes::from_static(b"body!")),
        ]));

        let response = put_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::PUT,
            OriginalUri(uri.clone()),
            headers,
            body,
        )
        .await
        .expect("streaming unsigned payload put should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let get_headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let get_response = get_object(
            State(state),
            Path((bucket, key)),
            Method::GET,
            OriginalUri(uri),
            get_headers,
        )
        .await
        .expect("get streamed object should succeed");
        let get_body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .expect("get body should read");
        assert_eq!(get_body.as_ref(), b"streamed body!");
    }

    #[tokio::test]
    async fn oversized_object_reads_are_rejected_by_in_memory_limit() {
        let mut state = test_state();
        let mut config = (*state.config).clone();
        config.max_in_memory_object_bytes = 8;
        state.config = Arc::new(config);

        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "notes/large.txt".to_string(),
                body: Bytes::from_static(b"this object is larger than eight bytes").into(),
                size: Some(37),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("large object should exist");

        let bucket = "placeholder".to_string();
        let key = "notes/large.txt".to_string();
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);

        let response = get_object(
            State(state),
            Path((bucket, key)),
            Method::GET,
            OriginalUri(uri),
            headers,
        )
        .await
        .expect_err("large object should be rejected")
        .into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be valid utf-8");
        assert!(body.contains("<Code>EntityTooLarge</Code>"));
    }

    #[tokio::test]
    async fn object_reads_fallback_to_sync_target_and_mark_response_headers() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "fallback/cached.txt".to_string(),
                body: Bytes::from_static(b"read from fallback").into(),
                size: Some(18),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("fallback object should be created");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "placeholder",
            "fallback/cached.txt",
            Some(18),
            Some("text/plain"),
        );

        let bucket = "placeholder".to_string();
        let key = "fallback/cached.txt".to_string();
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");

        let head_headers = signed_headers(&state.config, &Method::HEAD, &uri, &[], &[]);
        let head_response = head_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::HEAD,
            OriginalUri(uri.clone()),
            head_headers,
        )
        .await
        .expect("head should fallback to sync target");
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_eq!(
            head_response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("onedrive")
        );
        assert_eq!(
            head_response
                .headers()
                .get(FALLBACK_FROM_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stub")
        );

        let get_headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let get_response = get_object(
            State(state),
            Path((bucket, key)),
            Method::GET,
            OriginalUri(uri),
            get_headers,
        )
        .await
        .expect("get should fallback to sync target");
        assert_eq!(get_response.status(), StatusCode::OK);
        assert_eq!(
            get_response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("onedrive")
        );
        assert_eq!(
            get_response
                .headers()
                .get(FALLBACK_FROM_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stub")
        );

        let body = to_bytes(get_response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        assert_eq!(body.as_ref(), b"read from fallback");
    }

    #[tokio::test]
    async fn bucket_reads_can_fallback_to_sync_target() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "archive".to_string(),
                key: "snapshots/day-1.txt".to_string(),
                body: Bytes::from_static(b"snapshot").into(),
                size: Some(8),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("archive object should be created");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "archive",
            "snapshots/day-1.txt",
            Some(8),
            Some("text/plain"),
        );

        let bucket = "archive".to_string();
        let bucket_uri: Uri = format!("/{bucket}").parse().expect("uri should parse");
        let head_headers = signed_headers(&state.config, &Method::HEAD, &bucket_uri, &[], &[]);
        let head_response = head_bucket(
            State(state.clone()),
            Path(bucket.clone()),
            Method::HEAD,
            OriginalUri(bucket_uri.clone()),
            head_headers,
        )
        .await
        .expect("head bucket should fallback");
        assert_eq!(head_response.status(), StatusCode::OK);
        assert_eq!(
            head_response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("onedrive")
        );
        assert_eq!(
            head_response
                .headers()
                .get(FALLBACK_FROM_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stub")
        );

        let list_uri: Uri = format!("/{bucket}?list-type=2")
            .parse()
            .expect("list uri should parse");
        let list_headers = signed_headers(&state.config, &Method::GET, &list_uri, &[], &[]);
        let list_response = list_objects_v2(
            State(state),
            Path(bucket),
            Query(ListObjectsV2Query {
                list_type: Some("2".to_string()),
                ..Default::default()
            }),
            Method::GET,
            OriginalUri(list_uri),
            list_headers,
        )
        .await
        .expect("list objects should fallback");
        assert_eq!(list_response.status(), StatusCode::OK);
        assert_eq!(
            list_response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("onedrive")
        );

        let body = to_bytes(list_response.into_body(), usize::MAX)
            .await
            .expect("list body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be valid utf-8");
        assert!(body.contains("<Name>archive</Name>"));
        assert!(body.contains("<Key>snapshots/day-1.txt</Key>"));
    }

    #[tokio::test]
    async fn list_buckets_can_fallback_when_primary_backend_is_unavailable() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(
            &mut state,
            ProviderId::Stub,
            Arc::new(FailingBackend::new(
                "stub",
                "carrier primary temporarily blocked",
            )),
        );
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "archive".to_string(),
                key: "snapshots/day-2.txt".to_string(),
                body: Bytes::from_static(b"snapshot").into(),
                size: Some(8),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("archive object should be created");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "archive",
            "snapshots/day-2.txt",
            Some(8),
            Some("text/plain"),
        );

        let uri: Uri = "/".parse().expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);
        let response = list_buckets(State(state), Method::GET, OriginalUri(uri), headers)
            .await
            .expect("list buckets should fallback");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(SOURCE_PROVIDER_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("onedrive")
        );
        assert_eq!(
            response
                .headers()
                .get(FALLBACK_FROM_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("stub")
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let body = String::from_utf8(body.to_vec()).expect("body should be valid utf-8");
        assert!(body.contains("<Name>archive</Name>"));
    }

    #[tokio::test]
    async fn gateway_handles_primary_backend_with_multiple_containers() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "root".to_string(),
                key: "docs/alpha.txt".to_string(),
                body: Bytes::from_static(b"alpha").into(),
                size: Some(5),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("root object should be created");
        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("family object should be created");

        let buckets_uri: Uri = "/".parse().expect("uri should parse");
        let buckets_headers = signed_headers(&state.config, &Method::GET, &buckets_uri, &[], &[]);
        let buckets_response = list_buckets(
            State(state.clone()),
            Method::GET,
            OriginalUri(buckets_uri),
            buckets_headers,
        )
        .await
        .expect("list buckets should succeed");
        assert_eq!(buckets_response.status(), StatusCode::OK);
        let buckets_body = to_bytes(buckets_response.into_body(), usize::MAX)
            .await
            .expect("bucket body should read");
        let buckets_body =
            String::from_utf8(buckets_body.to_vec()).expect("bucket body should be utf-8");
        assert!(buckets_body.contains("<Name>root</Name>"));
        assert!(buckets_body.contains("<Name>family</Name>"));

        let family_head_uri: Uri = "/family".parse().expect("family head uri should parse");
        let family_head_headers =
            signed_headers(&state.config, &Method::HEAD, &family_head_uri, &[], &[]);
        let family_head = head_bucket(
            State(state.clone()),
            Path("family".to_string()),
            Method::HEAD,
            OriginalUri(family_head_uri),
            family_head_headers,
        )
        .await
        .expect("family bucket head should succeed");
        assert_eq!(family_head.status(), StatusCode::OK);

        let family_list_uri: Uri = "/family?list-type=2"
            .parse()
            .expect("family list uri should parse");
        let family_list_headers =
            signed_headers(&state.config, &Method::GET, &family_list_uri, &[], &[]);
        let family_list = list_objects_v2(
            State(state),
            Path("family".to_string()),
            Query(ListObjectsV2Query {
                list_type: Some("2".to_string()),
                ..Default::default()
            }),
            Method::GET,
            OriginalUri(family_list_uri),
            family_list_headers,
        )
        .await
        .expect("family bucket list should succeed");
        assert_eq!(family_list.status(), StatusCode::OK);
        let family_body = to_bytes(family_list.into_body(), usize::MAX)
            .await
            .expect("family list body should read");
        let family_body =
            String::from_utf8(family_body.to_vec()).expect("family list body should be utf-8");
        assert!(family_body.contains("<Name>family</Name>"));
        assert!(family_body.contains("<Key>shared/note.txt</Key>"));
    }

    #[tokio::test]
    async fn containers_api_returns_multiple_primary_backend_containers() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "root".to_string(),
                key: "docs/alpha.txt".to_string(),
                body: Bytes::from_static(b"alpha").into(),
                size: Some(5),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("root object should be created");
        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("family object should be created");

        let Json(containers) = list_containers(State(state))
            .await
            .expect("containers api should succeed");
        assert!(containers.len() >= 2);
        assert!(containers.iter().any(|container| container.name == "root"));
        assert!(
            containers
                .iter()
                .any(|container| container.name == "family")
        );
    }

    #[tokio::test]
    async fn objects_api_lists_family_container_objects_from_primary_backend() {
        let mut state = test_state();
        let primary_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Stub, primary_backend.clone());

        primary_backend
            .put_object(PutObjectRequest {
                container: "family".to_string(),
                key: "shared/note.txt".to_string(),
                body: Bytes::from_static(b"family note").into(),
                size: Some(11),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("family object should be created");

        let Json(objects) = list_objects(
            State(state),
            Query(ObjectsQuery {
                container: Some("family".to_string()),
                prefix: None,
                limit: None,
            }),
        )
        .await
        .expect("objects api should succeed");

        assert!(objects.iter().any(|object| object.key == "shared/note.txt"));
    }

    #[tokio::test]
    async fn containers_api_rejects_when_data_plane_concurrency_is_exhausted() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).data_plane_max_in_flight = 1;
        let semaphore = Arc::new(Semaphore::new(1));
        let held_permit = semaphore
            .clone()
            .try_acquire_owned()
            .expect("initial permit should be available");
        state.data_plane_concurrency = Arc::new(DataPlaneConcurrencyState { semaphore });

        let error = list_containers(State(state))
            .await
            .expect_err("containers api should reject exhausted concurrency");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf-8");
        assert!(body.contains("too many concurrent data-plane requests"));
        assert!(body.contains("limit=1"));

        drop(held_permit);
    }

    #[tokio::test]
    async fn objects_api_rejects_when_data_plane_concurrency_is_exhausted() {
        let mut state = test_state();
        Arc::make_mut(&mut state.config).data_plane_max_in_flight = 1;
        let semaphore = Arc::new(Semaphore::new(1));
        let held_permit = semaphore
            .clone()
            .try_acquire_owned()
            .expect("initial permit should be available");
        state.data_plane_concurrency = Arc::new(DataPlaneConcurrencyState { semaphore });

        let error = list_objects(
            State(state),
            Query(ObjectsQuery {
                container: Some("family".to_string()),
                prefix: None,
                limit: None,
            }),
        )
        .await
        .expect_err("objects api should reject exhausted concurrency");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should read");
        let body = String::from_utf8(body.to_vec()).expect("error body should be utf-8");
        assert!(body.contains("too many concurrent data-plane requests"));
        assert!(body.contains("limit=1"));

        drop(held_permit);
    }

    #[tokio::test]
    async fn stale_fallback_object_is_blocked_when_delete_is_pending() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "deleted/stale.txt".to_string(),
                body: Bytes::from_static(b"stale backup").into(),
                size: Some(12),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("stale backup object should exist");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Delete,
            ReplicationStatus::Pending,
            "placeholder",
            "deleted/stale.txt",
            None,
            None,
        );

        let bucket = "placeholder".to_string();
        let key = "deleted/stale.txt".to_string();
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);

        let response = get_object(
            State(state),
            Path((bucket, key)),
            Method::GET,
            OriginalUri(uri),
            headers,
        )
        .await
        .expect_err("stale fallback should be blocked")
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn onedrive_memory_only_policy_skips_non_memory_replication_jobs() {
        let state = test_state();
        {
            let mut control_plane = state
                .control_plane
                .lock()
                .expect("control plane should lock");
            control_plane.onedrive_policy = OnedrivePolicy {
                replication_enabled: true,
                fallback_enabled: true,
                scope_mode: OnedriveScopeMode::MemoryOnly,
                memory_buckets: vec!["agent-memory".to_string()],
                memory_prefixes: vec!["memory/".to_string()],
                updated_at_unix_ms: current_unix_ms(),
            };
        }

        let bucket = "placeholder".to_string();
        let key = "artifacts/output.txt".to_string();
        let body = Bytes::from_static(b"not memory");
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(
            &state.config,
            &Method::PUT,
            &uri,
            &body,
            &[("content-type", "text/plain")],
        );

        put_object(
            State(state.clone()),
            Path((bucket, key)),
            Method::PUT,
            OriginalUri(uri),
            headers,
            body.into(),
        )
        .await
        .expect("put should succeed");

        assert_eq!(state.replication.snapshot().pending_count, 0);
        assert_eq!(
            state
                .metadata_store
                .snapshot(16)
                .expect("snapshot should load")
                .pending_count,
            0
        );
    }

    #[tokio::test]
    async fn onedrive_memory_only_policy_blocks_non_memory_fallback_reads() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());
        {
            let mut control_plane = state
                .control_plane
                .lock()
                .expect("control plane should lock");
            control_plane.onedrive_policy = OnedrivePolicy {
                replication_enabled: true,
                fallback_enabled: true,
                scope_mode: OnedriveScopeMode::MemoryOnly,
                memory_buckets: vec!["agent-memory".to_string()],
                memory_prefixes: vec!["memory/".to_string()],
                updated_at_unix_ms: current_unix_ms(),
            };
        }

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "fallback/cached.txt".to_string(),
                body: Bytes::from_static(b"read from fallback").into(),
                size: Some(18),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("fallback object should be created");
        record_replication_state(
            &state,
            ProviderId::Onedrive,
            ReplicationOperation::Put,
            ReplicationStatus::Completed,
            "placeholder",
            "fallback/cached.txt",
            Some(18),
            Some("text/plain"),
        );

        let bucket = "placeholder".to_string();
        let key = "fallback/cached.txt".to_string();
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(&state.config, &Method::GET, &uri, &[], &[]);

        let response = get_object(
            State(state),
            Path((bucket, key)),
            Method::GET,
            OriginalUri(uri),
            headers,
        )
        .await
        .expect_err("fallback should be blocked by policy")
        .into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn topology_updates_can_disable_fallback_reads() {
        let state = test_state();

        let Json(saved) = update_topology(
            State(state.clone()),
            Json(TopologyUpdateInput {
                primary_provider: ProviderId::Stub,
                sync_targets: vec![ProviderId::Onedrive],
                fallback_read_order: Vec::new(),
            }),
        )
        .await
        .expect("topology update should succeed");

        assert_eq!(saved.primary_provider, "stub");
        assert_eq!(saved.sync_targets, vec!["onedrive"]);
        assert!(saved.fallback_read_order.is_empty());
        assert!(runtime_topology(&state).fallback_read_order.is_empty());
    }

    #[tokio::test]
    async fn topology_updates_apply_primary_provider_live() {
        let mut state = test_state();
        let telecom_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Telecom, telecom_backend.clone());

        let Json(saved) = update_topology(
            State(state.clone()),
            Json(TopologyUpdateInput {
                primary_provider: ProviderId::Telecom,
                sync_targets: vec![ProviderId::Onedrive],
                fallback_read_order: vec![ProviderId::Onedrive],
            }),
        )
        .await
        .expect("topology update should succeed");

        assert_eq!(saved.primary_provider, "telecom");
        assert!(!saved.restart_required);
        let desired = control_plane_snapshot(&state);
        assert_eq!(desired.topology.primary_provider, ProviderId::Telecom);
        assert_eq!(
            runtime_topology(&state).primary_provider,
            ProviderId::Telecom
        );

        let bucket = "placeholder".to_string();
        let key = "hot/live-switch.txt".to_string();
        let body = Bytes::from_static(b"write after switch");
        let uri: Uri = format!("/{bucket}/{key}")
            .parse()
            .expect("uri should parse");
        let headers = signed_headers(
            &state.config,
            &Method::PUT,
            &uri,
            &body,
            &[("content-type", "text/plain")],
        );

        put_object(
            State(state.clone()),
            Path((bucket.clone(), key.clone())),
            Method::PUT,
            OriginalUri(uri),
            headers,
            body.into(),
        )
        .await
        .expect("put after hot switch should succeed");

        let written = telecom_backend
            .get_object(&bucket, &key)
            .await
            .expect("new primary should receive writes immediately");
        assert_eq!(
            written
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"write after switch"
        );
        assert!(
            backend_for_test(&state, ProviderId::Stub)
                .get_object(&bucket, &key)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn replication_worker_can_copy_object_into_sync_target_backend() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "worker/copied.txt".to_string(),
                body: Bytes::from_static(b"copy through worker").into(),
                size: Some(19),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("source object should exist");

        let job = state
            .replication
            .enqueue_put(
                &runtime_topology(&state),
                Some(ProviderId::Stub.as_str().to_string()),
                "placeholder",
                "worker/copied.txt",
                None,
                19,
                Some("text/plain".to_string()),
            )
            .remove(0);

        process_replication_job(&state, &job)
            .await
            .expect("worker copy should succeed");

        let copied = target_backend
            .get_object("placeholder", "worker/copied.txt")
            .await
            .expect("target object should exist");
        assert_eq!(
            copied
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"copy through worker"
        );
    }

    #[tokio::test]
    async fn retryable_replication_failures_are_requeued_and_then_complete() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(FlakyWriteBackend::new(
            "flaky-onedrive",
            1,
            "temporary upstream outage",
        ));
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "worker/retry-once.txt".to_string(),
                body: Bytes::from_static(b"copy after retry").into(),
                size: Some(16),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("source object should exist");

        let jobs = state.replication.enqueue_put(
            &runtime_topology(&state),
            Some(ProviderId::Stub.as_str().to_string()),
            "placeholder",
            "worker/retry-once.txt",
            None,
            16,
            Some("text/plain".to_string()),
        );
        state
            .metadata_store
            .enqueue_jobs(&jobs)
            .expect("jobs should persist");

        assert!(process_next_replication_job(0, &state).await);

        let first_snapshot = state.replication.snapshot();
        assert_eq!(first_snapshot.pending_count, 1);
        assert_eq!(first_snapshot.retry_scheduled_count, 1);
        assert!(matches!(
            first_snapshot.pending_jobs[0].status,
            ReplicationStatus::RetryScheduled
        ));
        assert_eq!(first_snapshot.pending_jobs[0].attempts, 1);
        assert!(
            first_snapshot.pending_jobs[0]
                .next_attempt_at_unix_ms
                .is_some()
        );

        let persisted = state
            .metadata_store
            .snapshot(10)
            .expect("snapshot should persist retry state");
        assert_eq!(persisted.pending_count, 0);
        assert_eq!(persisted.retry_scheduled_count, 1);
        assert_eq!(persisted.failed_count, 0);

        assert!(process_next_replication_job(0, &state).await);

        let final_snapshot = state.replication.snapshot();
        assert_eq!(final_snapshot.pending_count, 0);
        assert_eq!(final_snapshot.recent_count, 1);
        assert!(matches!(
            final_snapshot.recent_jobs[0].status,
            ReplicationStatus::Completed
        ));
        assert_eq!(final_snapshot.recent_jobs[0].attempts, 2);

        let persisted = state
            .metadata_store
            .snapshot(10)
            .expect("snapshot should show completion");
        assert_eq!(persisted.pending_count, 0);
        assert_eq!(persisted.retry_scheduled_count, 0);
        assert_eq!(persisted.completed_count, 1);
        assert_eq!(persisted.failed_count, 0);

        let copied = target_backend
            .get_object("placeholder", "worker/retry-once.txt")
            .await
            .expect("target object should exist after retry");
        assert_eq!(
            copied
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"copy after retry"
        );
    }

    #[tokio::test]
    async fn replication_jobs_stop_retrying_after_max_attempts() {
        let mut state = test_state();
        let mut config = (*state.config).clone();
        config.replication_max_attempts = 2;
        state.config = Arc::new(config);

        let target_backend: DynBackend = Arc::new(FlakyWriteBackend::new(
            "flaky-onedrive",
            4,
            "target write keeps failing",
        ));
        replace_backend(&mut state, ProviderId::Onedrive, target_backend);

        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "worker/final-failure.txt".to_string(),
                body: Bytes::from_static(b"will not copy").into(),
                size: Some(13),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("source object should exist");

        let jobs = state.replication.enqueue_put(
            &runtime_topology(&state),
            Some(ProviderId::Stub.as_str().to_string()),
            "placeholder",
            "worker/final-failure.txt",
            None,
            13,
            Some("text/plain".to_string()),
        );
        state
            .metadata_store
            .enqueue_jobs(&jobs)
            .expect("jobs should persist");

        assert!(process_next_replication_job(0, &state).await);
        let retry_snapshot = state.replication.snapshot();
        assert_eq!(retry_snapshot.pending_count, 1);
        assert_eq!(retry_snapshot.retry_scheduled_count, 1);

        assert!(process_next_replication_job(0, &state).await);

        let final_snapshot = state.replication.snapshot();
        assert_eq!(final_snapshot.pending_count, 0);
        assert_eq!(final_snapshot.retry_scheduled_count, 0);
        assert_eq!(final_snapshot.recent_count, 1);
        assert!(matches!(
            final_snapshot.recent_jobs[0].status,
            ReplicationStatus::Failed
        ));
        assert_eq!(final_snapshot.recent_jobs[0].attempts, 2);

        let persisted = state
            .metadata_store
            .snapshot(10)
            .expect("snapshot should show final failure");
        assert_eq!(persisted.pending_count, 0);
        assert_eq!(persisted.retry_scheduled_count, 0);
        assert_eq!(persisted.failed_count, 1);
        assert_eq!(
            persisted.target_statuses[0]
                .latest_job
                .as_ref()
                .map(|job| job.attempts),
            Some(2)
        );
    }

    #[tokio::test]
    async fn queued_replication_jobs_keep_original_source_after_primary_hot_switch() {
        let mut state = test_state();
        let telecom_backend: DynBackend = Arc::new(StubBackend::new());
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Telecom, telecom_backend.clone());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        let bucket = "placeholder".to_string();
        let key = "worker/pre-switch.txt".to_string();
        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: bucket.clone(),
                key: key.clone(),
                body: Bytes::from_static(b"copy old primary").into(),
                size: Some(16),
                content_type: Some("text/plain".to_string()),
            })
            .await
            .expect("source object should exist on original primary");

        let job = state
            .replication
            .enqueue_put(
                &effective_topology_for_replication(
                    &state,
                    ReplicationOperation::Put,
                    &bucket,
                    &key,
                )
                .expect("effective topology should resolve"),
                Some(ProviderId::Stub.as_str().to_string()),
                bucket.clone(),
                key.clone(),
                None,
                16,
                Some("text/plain".to_string()),
            )
            .remove(0);

        let Json(saved) = update_topology(
            State(state.clone()),
            Json(TopologyUpdateInput {
                primary_provider: ProviderId::Telecom,
                sync_targets: vec![ProviderId::Onedrive],
                fallback_read_order: vec![ProviderId::Onedrive],
            }),
        )
        .await
        .expect("topology update should succeed");
        assert_eq!(saved.primary_provider, "telecom");
        assert_eq!(job.source_provider.as_deref(), Some("stub"));
        assert_eq!(
            runtime_topology(&state).primary_provider,
            ProviderId::Telecom
        );

        process_replication_job(&state, &job)
            .await
            .expect("queued job should still read from original primary");

        let copied = target_backend
            .get_object(&bucket, &key)
            .await
            .expect("target object should exist");
        assert_eq!(
            copied
                .body
                .collect()
                .await
                .expect("body should collect")
                .as_ref(),
            b"copy old primary"
        );
        assert!(telecom_backend.get_object(&bucket, &key).await.is_err());
    }
}
