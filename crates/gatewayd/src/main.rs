use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    net::SocketAddr,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
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
use blob_core::{
    BlobBackend, BlobError, ListObjectsRequest, OutboundIpFamily, PutObjectRequest, StubBackend,
    TokenSource,
};
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
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Clone)]
struct AppState {
    config: Arc<AppConfig>,
    backends: Arc<Mutex<Vec<ConfiguredBackend>>>,
    replication: Arc<ReplicationEngine>,
    metadata_store: Arc<MetadataStore>,
    auth: Arc<AuthBrokerState>,
    control_plane: Arc<Mutex<ControlPlaneState>>,
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
    runtime_topology: RuntimeTopologyPayload,
    desired_topology: DesiredTopologyPayload,
    replication: ReplicationQueueSummary,
    replication_state: ReplicationStatePayload,
    provider_health: Vec<BackendPayload>,
    alerts: Vec<AdminAlertPayload>,
    onedrive_auth: OneDriveAuthStatusPayload,
    onedrive_policy: OnedrivePolicy,
    auth_capture_policy: AuthCapturePolicyPayload,
}

#[derive(Debug, Serialize)]
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
    control_plane_file: String,
    credentials_dir: String,
    topology: TopologyPolicy,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_region: String,
    metadata_db_path: String,
    metadata_snapshot_recent_limit: usize,
    metadata_retention: MetadataRetentionPolicy,
    replication_workers: usize,
    replication_recent_limit: usize,
    replication_max_attempts: u32,
    replication_base_retry_delay_ms: u64,
    replication_max_retry_delay_ms: u64,
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
            control_plane_file: env_or("CCBG_CONTROL_PLANE_FILE", "./data/control-plane.json"),
            credentials_dir: env_or("CCBG_CREDENTIALS_DIR", "./data/provider-credentials"),
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
            replication_recent_limit: env_usize("CCBG_REPLICATION_RECENT_LIMIT", 64),
            replication_max_attempts: env_u64("CCBG_REPLICATION_MAX_ATTEMPTS", 3) as u32,
            replication_base_retry_delay_ms: env_u64("CCBG_REPLICATION_BASE_RETRY_DELAY_MS", 1_000),
            replication_max_retry_delay_ms: env_u64("CCBG_REPLICATION_MAX_RETRY_DELAY_MS", 30_000),
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
        };

        (status, Json(json!({ "error": error.to_string() }))).into_response()
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
        },
        config.onedrive.enabled,
    )?;
    config.topology = control_plane.topology.clone();
    let config = Arc::new(config);
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

    let state = AppState {
        config: config.clone(),
        backends: Arc::new(Mutex::new(backends)),
        replication,
        metadata_store,
        auth: Arc::new(AuthBrokerState::new()),
        control_plane: Arc::new(Mutex::new(control_plane)),
    };
    spawn_replication_workers(state.clone(), config.replication_workers);
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
        control_plane_file = %config.control_plane_file,
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
        .route("/api/control-plane/topology", post(update_topology))
        .route(
            "/api/providers/{provider}/credentials",
            get(get_provider_credentials).post(update_provider_credentials),
        )
        .route("/api/providers/{provider}/test", post(test_provider))
        .route("/api/object-status", get(inspect_object_status))
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

fn current_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

    Ok(ReplicationStatePayload {
        in_memory: state.replication.snapshot(),
        persisted,
        target_statuses,
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

    if replication_state.persisted.failed_count > 0 {
        alerts.push(AdminAlertPayload {
            severity: "error",
            title: format!(
                "{} replication jobs failed",
                replication_state.persisted.failed_count
            ),
            detail: "Check the recent replication jobs table for the latest error details."
                .to_string(),
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
    <div class="grid">
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
      <div id="replication-metrics" class="metric-grid"></div>
      <div style="margin-top: 16px;">
        <h3>Target Status</h3>
        <div id="replication-targets" class="table-wrap"></div>
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
          {{ key: 'family_id', label: 'Family ID (Optional)', placeholder: 'Used to probe family cloud when available' }},
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
    let authPromptState = new Map();

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
    function setProviderCredentialsFeedback(message, tone) {{
      const node = document.getElementById('provider-credentials-feedback');
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
      document.getElementById('auth-capture-llm-enabled').checked = !!payload.llm_analysis_enabled;
      document.getElementById('auth-capture-llm-endpoint').value = payload.llm_endpoint || '';
      document.getElementById('auth-capture-llm-model-id').value = payload.llm_model_id || '';
      document.getElementById('auth-capture-llm-api-key').value = '';
      document.getElementById('auth-capture-clear-llm-api-key').checked = false;
      document.getElementById('auth-capture-policy-json').textContent = JSON.stringify(payload, null, 2);
      const summary = [
        `Capture sidecar: ${{payload.enabled ? 'enabled' : 'disabled'}}`,
        `Broker URL: ${{payload.broker_url || 'not set'}}`,
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
                </tr>
              `).join('')
            }}
          </tbody>
        </table>
      `;
    }}
    function renderReplication(replicationState) {{
      if (!replicationState) {{
        return;
      }}
      document.getElementById('replication-metrics').innerHTML = `
        <div class="metric-card">
          <div>In-memory Pending</div>
          <strong>${{replicationState.in_memory.pending_count || 0}}</strong>
        </div>
        <div class="metric-card">
          <div>Persisted Pending</div>
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
    async function refreshStatus() {{
      try {{
        const status = await fetchJson('/api/status');
        document.getElementById('gateway-status').textContent = JSON.stringify(status, null, 2);
        document.getElementById('auth-status').textContent = JSON.stringify(status.onedrive_auth, null, 2);
        document.getElementById('runtime-topology').textContent = JSON.stringify(status.runtime_topology, null, 2);
        renderAuthSummary(status.onedrive_auth || {{}});
        renderRuntimeTopologySummary(status.runtime_topology || {{}});
        renderAlerts(status.alerts || []);
        renderProviderHealth(status.provider_health || []);
        renderReplication(status.replication_state);
        loadDesiredTopology(status.desired_topology);
        loadOnedrivePolicy(status.onedrive_policy);
        renderAuthCapturePolicy(status.auth_capture_policy || {{}});
        await refreshProviderCredentials();
        await refreshAuthCapturePrompts();
      }} catch (error) {{
        document.getElementById('gateway-status').textContent = error.message;
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
      const payload = {{
        enabled: document.getElementById('auth-capture-enabled').checked,
        broker_url: document.getElementById('auth-capture-broker-url').value.trim(),
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
    document.getElementById('reload-status').addEventListener('click', refreshStatus);
    document.getElementById('device-start').addEventListener('click', startDeviceFlow);
    document.getElementById('save-topology').addEventListener('click', saveTopology);
    document.getElementById('save-onedrive-policy').addEventListener('click', saveOnedrivePolicy);
    document.getElementById('save-auth-capture-policy').addEventListener('click', saveAuthCapturePolicy);
    document.getElementById('inspect-object-status').addEventListener('click', inspectObjectStatus);
    renderTopologyEditor();
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
    Ok(Json(AdminStatusPayload {
        runtime_topology: runtime_topology_payload(&runtime_topology(&state)),
        desired_topology: desired_topology_payload(&state),
        replication: ReplicationQueueSummary {
            pending_jobs: replication_state.in_memory.pending_count,
            recent_jobs: replication_state.in_memory.recent_count,
        },
        replication_state,
        provider_health,
        alerts,
        onedrive_auth,
        onedrive_policy: current_onedrive_policy(&state),
        auth_capture_policy: current_auth_capture_policy_payload(&state),
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

    let topology = runtime_topology(&state);
    let mut providers = Vec::with_capacity(1 + topology.sync_targets.len());
    providers.push(topology.primary_provider);
    for provider in &topology.sync_targets {
        if !providers.contains(provider) {
            providers.push(*provider);
        }
    }

    let gateway_resolution = resolve_object_read(&state, &bucket, &key).await;
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
        let backend = backend_for_provider(&state, provider)?;
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
                .latest_job_for_object(provider.as_str(), &bucket, &key)
                .map_err(|error| BlobError::Upstream(error.to_string()))?
        };
        let accepts_replication_put = if provider == topology.primary_provider {
            None
        } else {
            Some(provider_allowed_for_replication(
                &state,
                provider,
                ReplicationOperation::Put,
                &bucket,
                &key,
            )?)
        };
        let fallback_gate = if provider == topology.primary_provider {
            None
        } else {
            Some(load_fallback_gate_for_object(
                &state, provider, &bucket, &key,
            )?)
        };

        let (exists, object_info, access_error) = match backend.head_object(&bucket, &key).await {
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

    Ok(Json(ObjectStatusPayload {
        bucket,
        key,
        primary_provider: topology.primary_provider.as_str(),
        gateway_read_source,
        gateway_fallback_from,
        gateway_error,
        provider_states,
    }))
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
    Ok(Json(prompt.sanitized()))
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
        BlobError::Upstream(message) | BlobError::NotFound(message) => {
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
        BlobError::Upstream(message) => {
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

async fn list_containers(
    State(state): State<AppState>,
) -> Result<Json<Vec<blob_core::ContainerInfo>>, ApiError> {
    Ok(Json(list_containers_with_fallback(&state).await?.value))
}

async fn list_objects(
    State(state): State<AppState>,
    Query(query): Query<ObjectsQuery>,
) -> Result<Json<Vec<blob_core::ObjectInfo>>, ApiError> {
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

    Ok((StatusCode::OK, response_headers, object.body).into_response())
}

async fn put_object(
    State(state): State<AppState>,
    Path((bucket, key)): Path<(String, String)>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, S3Error> {
    authorize_s3(&state.config, &method, &uri, &headers, Some(&body))?;
    let (primary_provider, primary_backend) =
        current_primary_backend(&state).map_err(map_backend_error_to_s3)?;

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);

    ensure_object_within_in_memory_limit(&state.config, body.len() as u64)?;

    let result = primary_backend
        .put_object(PutObjectRequest {
            container: bucket.clone(),
            key: key.clone(),
            body: body.to_vec(),
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
        body.len() as u64,
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use axum::body::to_bytes;
    use blob_core::{BackendCapabilities, ContainerInfo, HealthStatus, ObjectInfo, ServiceHealth};

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

        Arc::new(AppConfig {
            bind_addr: "127.0.0.1:61080".parse().expect("test addr should parse"),
            admin_mode: AdminMode::Web,
            admin_bind_addr: "127.0.0.1:61081".parse().expect("admin addr should parse"),
            auth_callback_bind_addr: "127.0.0.1:61082"
                .parse()
                .expect("callback addr should parse"),
            control_plane_file: temp_db_path().replace(".db", "-control-plane.json"),
            credentials_dir: temp_db_path().replace(".db", "-provider-credentials"),
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
            replication_recent_limit: 64,
            replication_max_attempts: 3,
            replication_base_retry_delay_ms: 0,
            replication_max_retry_delay_ms: 0,
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
            })),
        }
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
    async fn object_status_reports_fallback_readability() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "fallback/inspect.txt".to_string(),
                body: b"inspect fallback".to_vec(),
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

        let Json(status) = admin_status(State(state))
            .await
            .expect("admin status should succeed");

        assert_eq!(status.runtime_topology.primary_provider, "stub");
        assert_eq!(status.provider_health.len(), 2);
        assert_eq!(status.replication_state.persisted.pending_count, 0);
        assert!(
            status
                .alerts
                .iter()
                .any(|alert| alert.title.contains("Replication workers"))
        );
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
            body,
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
            body.clone(),
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
            2
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
    async fn oversized_object_reads_are_rejected_by_in_memory_limit() {
        let mut state = test_state();
        let mut config = (*state.config).clone();
        config.max_in_memory_object_bytes = 8;
        state.config = Arc::new(config);

        backend_for_test(&state, ProviderId::Stub)
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "notes/large.txt".to_string(),
                body: b"this object is larger than eight bytes".to_vec(),
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
                body: b"read from fallback".to_vec(),
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
                body: b"snapshot".to_vec(),
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
                body: b"snapshot".to_vec(),
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
    async fn stale_fallback_object_is_blocked_when_delete_is_pending() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        replace_backend(&mut state, ProviderId::Onedrive, target_backend.clone());

        target_backend
            .put_object(PutObjectRequest {
                container: "placeholder".to_string(),
                key: "deleted/stale.txt".to_string(),
                body: b"stale backup".to_vec(),
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
            body,
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
                body: b"read from fallback".to_vec(),
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
            body,
        )
        .await
        .expect("put after hot switch should succeed");

        let written = telecom_backend
            .get_object(&bucket, &key)
            .await
            .expect("new primary should receive writes immediately");
        assert_eq!(written.body, b"write after switch".to_vec());
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
                body: b"copy through worker".to_vec(),
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
        assert_eq!(copied.body, b"copy through worker".to_vec());
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
                body: b"copy after retry".to_vec(),
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
        assert_eq!(persisted.pending_count, 1);
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
        assert_eq!(copied.body, b"copy after retry".to_vec());
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
                body: b"will not copy".to_vec(),
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
                body: b"copy old primary".to_vec(),
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
        assert_eq!(copied.body, b"copy old primary".to_vec());
        assert!(telecom_backend.get_object(&bucket, &key).await.is_err());
    }
}
