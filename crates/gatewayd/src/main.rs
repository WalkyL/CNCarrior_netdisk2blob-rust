use std::{
    collections::{BTreeMap, HashMap},
    env,
    net::SocketAddr,
    sync::Arc,
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, OriginalUri, Path, Query, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode, Uri,
        header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ETAG, HOST, LAST_MODIFIED},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use blob_core::{
    BlobBackend, BlobError, ListObjectsRequest, PutObjectRequest, StubBackend, TokenSource,
};
use hmac::{Hmac, Mac};
use metadata_store::{
    MetadataRetentionPolicy, MetadataSnapshot, MetadataStore, MetadataStoreOptions,
};
use policy_engine::{
    ProviderId, ReplicationMode, TopologyInput, TopologyPolicy, parse_provider_list,
};
use provider_mobile::{MobileBlobAdapter, MobileConfig};
use provider_onedrive::{OneDriveBlobAdapter, OneDriveConfig};
use provider_telecom::{TelecomBlobAdapter, TelecomConfig};
use provider_unicom::{UnicomBlobAdapter, UnicomConfig};
use replication_engine::{
    ReplicationEngine, ReplicationJob, ReplicationOperation, ReplicationSnapshot, ReplicationStatus,
};
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

#[derive(Clone)]
struct AppState {
    backend: DynBackend,
    config: Arc<AppConfig>,
    topology: Arc<TopologyPolicy>,
    replication: Arc<ReplicationEngine>,
    metadata_store: Arc<MetadataStore>,
    sync_backends: Vec<ConfiguredBackend>,
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
}

#[derive(Debug, Clone)]
struct AppConfig {
    bind_addr: SocketAddr,
    topology: TopologyPolicy,
    s3_access_key_id: String,
    s3_secret_access_key: String,
    s3_region: String,
    metadata_db_path: String,
    metadata_snapshot_recent_limit: usize,
    metadata_retention: MetadataRetentionPolicy,
    replication_workers: usize,
    replication_recent_limit: usize,
    max_in_memory_object_bytes: usize,
    onedrive: OneDriveConfig,
}

impl AppConfig {
    fn from_env() -> Result<Self> {
        let bind_addr: SocketAddr = env::var("CCBG_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:61080".to_string())
            .parse()
            .context("invalid CCBG_BIND_ADDR")?;
        validate_port_range(bind_addr.port())?;

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
struct ReplicationStatePayload {
    in_memory: ReplicationSnapshot,
    persisted: MetadataSnapshot,
}

#[derive(Debug, Serialize)]
struct BackendPayload {
    role: &'static str,
    provider: &'static str,
    health: blob_core::ServiceHealth,
}

#[derive(Debug, Deserialize)]
struct ObjectsQuery {
    container: Option<String>,
    prefix: Option<String>,
    limit: Option<usize>,
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

    let config = Arc::new(AppConfig::from_env()?);
    let topology = Arc::new(config.topology.clone());
    let backend = build_backend(&config, topology.primary_provider);
    let backend_name = backend.name();
    let sync_backends = build_sync_backends(&config, &topology);
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
    let restored_jobs = metadata_store
        .load_pending_jobs(None)
        .context("failed to restore pending replication jobs")?;
    if !restored_jobs.is_empty() {
        info!(
            restored_jobs = restored_jobs.len(),
            "restored replication jobs from sqlite"
        );
        replication.restore_pending(restored_jobs);
    }

    let state = AppState {
        backend,
        config: config.clone(),
        topology,
        replication,
        metadata_store,
        sync_backends,
    };
    spawn_replication_workers(state.clone(), config.replication_workers);

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
        backend = backend_name,
        metadata_db_path = %config.metadata_db_path,
        metadata_snapshot_recent_limit = config.metadata_snapshot_recent_limit,
        metadata_completed_history_limit = config.metadata_retention.completed_history_limit,
        metadata_failed_history_limit = config.metadata_retention.failed_history_limit,
        replication_workers = config.replication_workers,
        replication_recent_limit = config.replication_recent_limit,
        max_in_memory_object_bytes = config.max_in_memory_object_bytes,
        "gateway ready"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server exited with error")
}

fn build_backend(config: &AppConfig, provider: ProviderId) -> DynBackend {
    match provider {
        ProviderId::Stub => Arc::new(StubBackend::new()),
        ProviderId::Unicom => Arc::new(UnicomBlobAdapter::new(UnicomConfig {
            base_url: env_or("CCBG_UNICOM_BASE_URL", "https://panservice.mail.wo.cn"),
            token_source: resolve_token_source("CCBG_UNICOM"),
            cookie_header: env::var("CCBG_UNICOM_COOKIE_HEADER").ok(),
            user_agent: env_or(
                "CCBG_UNICOM_USER_AGENT",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
            ),
            request_timeout_secs: env_u64("CCBG_UNICOM_TIMEOUT_SECS", 30),
        })),
        ProviderId::Telecom => Arc::new(TelecomBlobAdapter::new(TelecomConfig {
            base_url: env_or("CCBG_TELECOM_BASE_URL", "https://cloud.189.cn"),
            token_source: resolve_token_source("CCBG_TELECOM"),
            cookie_header: env::var("CCBG_TELECOM_COOKIE_HEADER").ok(),
            user_agent: env_or(
                "CCBG_TELECOM_USER_AGENT",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
            ),
            request_timeout_secs: env_u64("CCBG_TELECOM_TIMEOUT_SECS", 30),
        })),
        ProviderId::Mobile => Arc::new(MobileBlobAdapter::new(MobileConfig {
            base_url: env_or("CCBG_MOBILE_BASE_URL", "https://yun.139.com"),
            token_source: resolve_token_source("CCBG_MOBILE"),
            cookie_header: env::var("CCBG_MOBILE_COOKIE_HEADER").ok(),
            user_agent: env_or(
                "CCBG_MOBILE_USER_AGENT",
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
            ),
            request_timeout_secs: env_u64("CCBG_MOBILE_TIMEOUT_SECS", 30),
        })),
        ProviderId::Onedrive => Arc::new(OneDriveBlobAdapter::new(config.onedrive.clone())),
    }
}

fn build_sync_backends(config: &AppConfig, topology: &TopologyPolicy) -> Vec<ConfiguredBackend> {
    topology
        .sync_targets
        .iter()
        .copied()
        .map(|provider| ConfiguredBackend {
            provider,
            backend: build_backend(config, provider),
        })
        .collect()
}

fn ordered_fallback_backends(state: &AppState) -> Vec<ConfiguredBackend> {
    state
        .topology
        .fallback_read_order
        .iter()
        .filter_map(|provider| {
            state
                .sync_backends
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
    let primary_provider = state.topology.primary_provider;
    let mut first_non_not_found = None;

    match state.backend.list_containers().await {
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

fn load_fallback_gate_for_object(
    state: &AppState,
    provider: ProviderId,
    bucket: &str,
    key: &str,
) -> Result<FallbackObjectGate, BlobError> {
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
    let primary_provider = state.topology.primary_provider;
    let mut first_non_not_found = None;

    match state.backend.head_container(bucket).await {
        Ok(_) => {
            return Ok(ResolvedReadBackend {
                source: ReadSource {
                    provider: primary_provider,
                    fallback_from: None,
                },
                backend: state.backend.clone(),
            });
        }
        Err(error) => remember_read_error(&mut first_non_not_found, error),
    }

    for backend in ordered_fallback_backends(state) {
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
    let primary_provider = state.topology.primary_provider;
    let mut first_non_not_found = None;
    let mut deleted_on_fallback = false;

    match state.backend.head_object(bucket, key).await {
        Ok(object) => {
            return Ok(ResolvedObjectRead {
                source: ReadSource {
                    provider: primary_provider,
                    fallback_from: None,
                },
                backend: state.backend.clone(),
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
        let Some(job) = state.replication.pop_next() else {
            sleep(Duration::from_millis(250)).await;
            continue;
        };

        let attempts = job.attempts.saturating_add(1);
        let result = process_replication_job(&state, &job).await;

        match result {
            Ok(()) => {
                let mut completed_job = job.clone();
                completed_job.attempts = attempts;
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
                    "replication job completed"
                );
            }
            Err(error) => {
                let mut failed_job = job.clone();
                failed_job.attempts = attempts;
                state.replication.record_failed(failed_job, error.clone());
                match state.metadata_store.mark_job_status(
                    job.job_id,
                    replication_engine::ReplicationStatus::Failed,
                    attempts,
                    Some(&error),
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
                    error = %error,
                    "replication job failed"
                );
            }
        }
    }
}

async fn process_replication_job(
    state: &AppState,
    job: &replication_engine::ReplicationJob,
) -> Result<(), String> {
    let backend = state
        .sync_backends
        .iter()
        .find(|backend| backend.provider.as_str() == job.target)
        .map(|backend| backend.backend.clone())
        .ok_or_else(|| format!("sync target backend is not configured: {}", job.target))?;

    match job.operation {
        replication_engine::ReplicationOperation::Put => {
            if !backend.capabilities().write {
                return Err(format!("target {} does not support write", job.target));
            }
            ensure_replication_object_within_in_memory_limit(&state.config, job)?;

            let source_object = state
                .backend
                .get_object(&job.object.bucket, &job.object.key)
                .await
                .map_err(|error| format!("failed to read source object: {error}"))?;

            backend
                .put_object(PutObjectRequest {
                    container: job.object.bucket.clone(),
                    key: job.object.key.clone(),
                    body: source_object.body,
                    content_type: source_object.info.content_type,
                })
                .await
                .map_err(|error| format!("failed to write target object: {error}"))?;
        }
        replication_engine::ReplicationOperation::Delete => {
            if !backend.capabilities().delete {
                return Err(format!("target {} does not support delete", job.target));
            }

            backend
                .delete_object(&job.object.bucket, &job.object.key)
                .await
                .map_err(|error| format!("failed to delete target object: {error}"))?;
        }
    }

    Ok(())
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

    Json(IndexPayload {
        service: "carrier-cloud-blob-gateway",
        backend: state.backend.name(),
        primary_provider: state.topology.primary_provider_name(),
        sync_targets: state.topology.sync_target_names(),
        fallback_read_order: state.topology.fallback_read_order_names(),
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
    let mut providers = Vec::with_capacity(1 + state.sync_backends.len());
    providers.push(BackendPayload {
        role: "primary",
        provider: state.topology.primary_provider_name(),
        health: state.backend.health().await?,
    });

    for backend in &state.sync_backends {
        providers.push(BackendPayload {
            role: "sync_target",
            provider: backend.provider.as_str(),
            health: backend.backend.health().await?,
        });
    }

    Ok(Json(providers))
}

async fn replication_snapshot(
    State(state): State<AppState>,
) -> Result<Json<ReplicationStatePayload>, ApiError> {
    let persisted = state
        .metadata_store
        .snapshot(state.config.metadata_snapshot_recent_limit)
        .map_err(|error| BlobError::Upstream(error.to_string()))?;

    Ok(Json(ReplicationStatePayload {
        in_memory: state.replication.snapshot(),
        persisted,
    }))
}

async fn healthz(
    State(state): State<AppState>,
) -> Result<Json<blob_core::ServiceHealth>, ApiError> {
    Ok(Json(state.backend.health().await?))
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
    let request = ListObjectsRequest {
        container: query.container,
        prefix: query.prefix,
        limit: query.limit,
    };

    Ok(Json(state.backend.list_objects(request).await?))
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

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);

    ensure_object_within_in_memory_limit(&state.config, body.len() as u64)?;

    let result = state
        .backend
        .put_object(PutObjectRequest {
            container: bucket.clone(),
            key: key.clone(),
            body: body.to_vec(),
            content_type: content_type.clone(),
        })
        .await
        .map_err(map_backend_error_to_s3)?;

    let jobs = state.replication.enqueue_put(
        &state.topology,
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

    state
        .backend
        .delete_object(&bucket, &key)
        .await
        .map_err(|error| map_object_error(error, &bucket, &key))?;

    let jobs = state
        .replication
        .enqueue_delete(&state.topology, bucket.clone(), key.clone());
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

    fn test_config() -> Arc<AppConfig> {
        let topology = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Stub,
            sync_targets: vec![ProviderId::Onedrive],
            fallback_read_order: Vec::new(),
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect("test topology should validate");

        Arc::new(AppConfig {
            bind_addr: "127.0.0.1:61080".parse().expect("test addr should parse"),
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
            max_in_memory_object_bytes: 8 * 1024 * 1024,
            onedrive: OneDriveConfig {
                enabled: true,
                tenant: "common".to_string(),
                client_id: Some("unit-test-client".to_string()),
                use_device_code: true,
                redirect_url: Some("http://127.0.0.1:61082/auth/onedrive/callback".to_string()),
                drive_id: Some("drive-test".to_string()),
                graph_base_url: "https://graph.microsoft.com/v1.0".to_string(),
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
        let topology = Arc::new(config.topology.clone());
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
            backend: Arc::new(StubBackend::new()),
            config: config.clone(),
            topology: topology.clone(),
            replication: Arc::new(ReplicationEngine::with_recent_limit(
                config.replication_recent_limit,
            )),
            metadata_store,
            sync_backends: build_sync_backends(&config, &topology),
        }
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
        assert_eq!(snapshot.persisted.pending_count, 1);
        assert_eq!(snapshot.persisted.recent_jobs.len(), 1);
    }

    #[tokio::test]
    async fn object_lifecycle_round_trip_works_for_signed_requests() {
        let mut state = test_state();
        state.sync_backends.clear();
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
        state.sync_backends.clear();

        state
            .backend
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
        state.sync_backends = vec![ConfiguredBackend {
            provider: ProviderId::Onedrive,
            backend: target_backend.clone(),
        }];

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
        state.sync_backends = vec![ConfiguredBackend {
            provider: ProviderId::Onedrive,
            backend: target_backend.clone(),
        }];

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
        state.backend = Arc::new(FailingBackend::new(
            "stub",
            "carrier primary temporarily blocked",
        ));
        state.sync_backends = vec![ConfiguredBackend {
            provider: ProviderId::Onedrive,
            backend: target_backend.clone(),
        }];

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
        state.sync_backends = vec![ConfiguredBackend {
            provider: ProviderId::Onedrive,
            backend: target_backend.clone(),
        }];

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
    async fn replication_worker_can_copy_object_into_sync_target_backend() {
        let mut state = test_state();
        let target_backend: DynBackend = Arc::new(StubBackend::new());
        state.sync_backends = vec![ConfiguredBackend {
            provider: ProviderId::Onedrive,
            backend: target_backend.clone(),
        }];

        state
            .backend
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
                &state.topology,
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
}
