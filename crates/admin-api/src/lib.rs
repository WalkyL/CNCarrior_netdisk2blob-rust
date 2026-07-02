// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use blob_core::BrowserRequestProfile;
use policy_engine::ProviderId;

pub const ADMIN_API_VERSION: &str = "2026-05-26";

pub const ROUTE_STATUS: &str = "/api/status";
pub const ROUTE_ADMIN_LOGIN: &str = "/api/admin/login";
pub const ROUTE_ADMIN_LOGOUT: &str = "/api/admin/logout";
pub const ROUTE_ADMIN_CHANGE_PASSWORD: &str = "/api/admin/change-password";
pub const ROUTE_ADMIN_LOGS: &str = "/api/admin/logs";
pub const ROUTE_TOPOLOGY_UPDATE: &str = "/api/control-plane/topology";
pub const ROUTE_AUTH_CAPTURE_POLICY: &str = "/api/policy/auth-capture";
pub const ROUTE_PROVIDER_CREDENTIALS: &str = "/api/providers/{provider}/credentials";
pub const ROUTE_BROWSER_FLOW_SESSION_HANDOFF: &str =
    "/api/browser-flow/sessions/{session_id}/handoff";
pub const ROUTE_REPLICATION_RETRY_JOB: &str = "/api/replication/jobs/{job_id}/retry";
pub const ROUTE_REPLICATION_DLQ: &str = "/api/replication/dlq";
pub const ROUTE_REPLICATION_DLQ_REPLAY_JOB: &str = "/api/replication/dlq/jobs/{job_id}/replay";
pub const ROUTE_REPLICATION_DLQ_REPLAY_TARGET: &str =
    "/api/replication/dlq/targets/{target}/replay";
pub const ROUTE_OBJECT_RECONCILE_PREVIEW: &str = "/api/object-reconcile/preview";
pub const ROUTE_OBJECT_RECONCILE_EXECUTE: &str = "/api/object-reconcile/execute";
pub const ROUTE_ALERT_SUPPRESSIONS: &str = "/api/alerts/suppressions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminApiSurface {
    Operator,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminApiMethod {
    Get,
    Post,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminDtoKind {
    AdminStatusPayload,
    TopologyUpdateInput,
    DesiredTopologyPayload,
    AuthCapturePolicyInput,
    AuthCapturePolicyPayload,
    ProviderCredentialInput,
    ProviderCredentialPayload,
    BrowserFlowSessionHandoffInput,
    BrowserFlowSessionHandoffPayload,
    ReplicationRetryPayload,
    ReplicationDlqEntryPayload,
    ReplicationDlqListPayload,
    ReplicationDlqReplayPayload,
    ReplicationDlqTargetReplayPayload,
    ObjectReconcileExecuteInput,
    ObjectReconcileExecutePayload,
    ObjectReconcilePreviewPayload,
    AdminAlertSuppressionInput,
    SuppressedAdminAlertRecord,
    SuppressedAdminAlertRecordList,
    AdminLoginInput,
    AdminLoginPayload,
    AdminChangePasswordInput,
    AdminChangePasswordPayload,
    AdminLogQueryResult,
    NoContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AdminRouteContract {
    pub id: &'static str,
    pub method: AdminApiMethod,
    pub path: &'static str,
    pub surface: AdminApiSurface,
    pub request: Option<AdminDtoKind>,
    pub response: AdminDtoKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectPlacementMode {
    PreferPrimary,
    PreferHighSpeed,
    BalanceHighSpeed,
}

impl ObjectPlacementMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreferPrimary => "prefer_primary",
            Self::PreferHighSpeed => "prefer_high_speed",
            Self::BalanceHighSpeed => "balance_high_speed",
        }
    }
}

impl Default for ObjectPlacementMode {
    fn default() -> Self {
        Self::PreferPrimary
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyUpdateInput {
    pub primary_provider: ProviderId,
    #[serde(default)]
    pub sync_targets: Vec<ProviderId>,
    #[serde(default)]
    pub fallback_read_order: Vec<ProviderId>,
    #[serde(default)]
    pub high_speed_providers: Vec<ProviderId>,
    #[serde(default)]
    pub write_targets: Vec<ProviderId>,
    #[serde(default)]
    pub object_placement_mode: ObjectPlacementMode,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DesiredTopologyPayload {
    pub primary_provider: &'static str,
    pub write_targets: Vec<&'static str>,
    pub sync_targets: Vec<&'static str>,
    pub fallback_read_order: Vec<&'static str>,
    pub high_speed_providers: Vec<&'static str>,
    pub object_placement_mode: &'static str,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthCaptureBrowserEndpoint {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub endpoint_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub target_selector: Option<String>,
    #[serde(default)]
    pub target_timeout_ms: Option<u64>,
    #[serde(default)]
    pub detected_browser: Option<String>,
    #[serde(default)]
    pub detected_protocol_version: Option<String>,
    #[serde(default)]
    pub detected_user_agent: Option<String>,
    #[serde(default)]
    pub detected_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCaptureBrowserEndpointInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub endpoint_url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub target_selector: Option<String>,
    #[serde(default)]
    pub target_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthCaptureLlmEndpoint {
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub endpoint_url: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_llm_reasoning_effort")]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCaptureLlmEndpointInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub endpoint_url: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthCaptureLlmEndpointPayload {
    pub id: String,
    pub label: Option<String>,
    pub endpoint_url: String,
    pub model_id: String,
    pub enabled: bool,
    pub reasoning_effort: Option<String>,
    pub api_key_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCapturePolicyPayload {
    pub enabled: bool,
    pub broker_url: Option<String>,
    pub browser_endpoints: Vec<AuthCaptureBrowserEndpoint>,
    pub cdp_endpoint_url: Option<String>,
    pub cdp_target_selector: Option<String>,
    pub cdp_target_timeout_ms: Option<u64>,
    pub llm_analysis_enabled: bool,
    pub llm_endpoints: Vec<AuthCaptureLlmEndpointPayload>,
    pub llm_endpoint: Option<String>,
    pub llm_model_id: Option<String>,
    pub llm_api_key_present: bool,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCapturePolicyInput {
    pub enabled: bool,
    pub broker_url: Option<String>,
    #[serde(default)]
    pub browser_endpoints: Option<Vec<AuthCaptureBrowserEndpointInput>>,
    pub cdp_endpoint_url: Option<String>,
    pub cdp_target_selector: Option<String>,
    pub cdp_target_timeout_ms: Option<u64>,
    pub llm_analysis_enabled: bool,
    #[serde(default)]
    pub llm_endpoints: Option<Vec<AuthCaptureLlmEndpointInput>>,
    pub llm_endpoint: Option<String>,
    pub llm_model_id: Option<String>,
    #[serde(default)]
    pub llm_api_key: Option<String>,
    #[serde(default)]
    pub clear_llm_api_key: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserFlowSessionHandoffInput {
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowSessionHandoffPayload {
    pub session_id: String,
    pub provider: String,
    pub status: String,
    pub handoff_at_unix_ms: u64,
    pub credential_keys: Vec<String>,
    pub runtime_keys: Vec<String>,
    pub audit_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationRetryPayload {
    pub job_id: u64,
    pub status: String,
    pub target: String,
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationDlqEntryPayload {
    pub original_job_id: u64,
    pub status: String,
    pub target: String,
    pub bucket: String,
    pub key: String,
    pub operation: String,
    pub attempts: u32,
    pub dead_lettered_at_unix_ms: u64,
    pub reason: String,
    pub replay_count: u32,
    pub last_replayed_job_id: Option<u64>,
    pub last_replayed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationDlqListPayload {
    pub entries: Vec<ReplicationDlqEntryPayload>,
    pub open_count: usize,
    pub returned_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationDlqReplayPayload {
    pub original_job_id: u64,
    pub replayed_job_id: u64,
    pub status: String,
    pub target: String,
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationDlqTargetReplayPayload {
    pub target: String,
    pub replayed_jobs: usize,
    pub jobs: Vec<ReplicationDlqReplayPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcilePreviewSummaryPayload {
    pub total_rows: usize,
    pub no_change_count: usize,
    pub needs_changes_count: usize,
    pub blocked_count: usize,
    pub skipped_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcilePreviewRowPayload {
    pub bucket: String,
    pub key: String,
    pub home_provider: String,
    pub home_label: String,
    pub has_home_placement: bool,
    pub desired_home_provider: String,
    pub desired_home_label: String,
    pub application_id: Option<String>,
    pub logical_content_type: Option<String>,
    pub encrypted: bool,
    pub desired_encrypted: bool,
    pub current_encryption_profile_id: Option<String>,
    pub desired_encryption_profile_id: Option<String>,
    pub current_sync_targets: Vec<String>,
    pub desired_sync_targets: Vec<String>,
    pub add_sync_targets: Vec<String>,
    pub remove_sync_targets: Vec<String>,
    pub capacity_required_bytes: Option<u64>,
    pub provider_peak_required_bytes: Option<u64>,
    pub local_spool_required_bytes: Option<u64>,
    pub status: String,
    pub status_label: String,
    pub note: String,
    pub next_step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcilePreviewPayload {
    pub provider_filter: Option<String>,
    pub bucket_filter: Option<String>,
    pub prefix_filter: Option<String>,
    pub limit: usize,
    pub rows: Vec<ObjectReconcilePreviewRowPayload>,
    pub summary: ObjectReconcilePreviewSummaryPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcileExecuteInput {
    pub rows: Vec<ObjectReconcileExecuteRowInput>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcileExecuteRowInput {
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcileExecutePayload {
    pub requested: usize,
    pub executed_count: usize,
    pub failed_count: usize,
    pub entries: Vec<ObjectReconcileExecuteEntryPayload>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectReconcileExecuteEntryPayload {
    pub bucket: String,
    pub key: String,
    pub outcome: String,
    pub action: String,
    pub message: String,
    pub preview_status: String,
    pub has_home_placement: bool,
    pub add_sync_targets: Vec<String>,
    pub remove_sync_targets: Vec<String>,
    pub old_home_provider: String,
    pub new_home_provider: String,
    pub old_encryption_profile_id: Option<String>,
    pub new_encryption_profile_id: Option<String>,
    pub capacity_required_bytes: Option<u64>,
    pub provider_peak_required_bytes: Option<u64>,
    pub local_spool_required_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAlertSuppressionInput {
    pub alert_id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub suppressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuppressedAdminAlertRecord {
    pub id: String,
    pub title: String,
    pub closed_at_unix_ms: u64,
    pub delete_after_unix_ms: u64,
}

pub type SuppressedAdminAlertRecordList = Vec<SuppressedAdminAlertRecord>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderCredentialInput {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub browser_id: Option<String>,
    #[serde(default)]
    pub cookie_header: Option<String>,
    #[serde(default)]
    pub family_id: Option<String>,
    #[serde(default)]
    pub root_folder_id: Option<String>,
    #[serde(default)]
    pub user_domain_id: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub tenant: Option<String>,
    #[serde(default)]
    pub drive_id: Option<String>,
    #[serde(default)]
    pub redirect_url: Option<String>,
    #[serde(default)]
    pub quota_min_free: Option<String>,
    #[serde(default)]
    pub quota_max_used: Option<String>,
    #[serde(default)]
    pub personal_quota_min_free: Option<String>,
    #[serde(default)]
    pub personal_quota_max_used: Option<String>,
    #[serde(default)]
    pub family_quota_min_free: Option<String>,
    #[serde(default)]
    pub family_quota_max_used: Option<String>,
    #[serde(default)]
    pub browser_profile: Option<BrowserRequestProfile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderCredentialPayload {
    pub provider: &'static str,
    pub label: &'static str,
    pub storage_path: String,
    pub token: Option<String>,
    pub token_present: bool,
    pub browser_id: Option<String>,
    pub browser_id_present: bool,
    pub cookie_header: Option<String>,
    pub cookie_header_present: bool,
    pub family_id: Option<String>,
    pub root_folder_id: Option<String>,
    pub user_domain_id: Option<String>,
    pub client_id: Option<String>,
    pub tenant: Option<String>,
    pub drive_id: Option<String>,
    pub redirect_url: Option<String>,
    pub quota_min_free: Option<String>,
    pub quota_max_used: Option<String>,
    pub personal_quota_min_free: Option<String>,
    pub personal_quota_max_used: Option<String>,
    pub family_quota_min_free: Option<String>,
    pub family_quota_max_used: Option<String>,
    pub browser_profile: Option<BrowserRequestProfile>,
    pub session_file: Option<String>,
    pub lease: Option<ProviderCredentialLeasePayload>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProviderCredentialLeasePayload {
    pub provider: &'static str,
    pub label: &'static str,
    pub status: String,
    pub summary: String,
    pub requires_reauth: bool,
    pub captured_at_unix_ms: Option<u64>,
    pub last_verified_at_unix_ms: Option<u64>,
    pub last_success_at_unix_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminStatusPayload<
    Runtime,
    AdminAuth,
    GatewayBackup,
    GatewayWriteAheadLog,
    DesktopMountSeed,
    TopologyProviderCatalog,
    ProviderLimitProbes,
    ProviderLimitProfiles,
    Monitoring,
    OperationsOverview,
    Notify,
    RuntimeTopology,
    DesiredTopology,
    ReplicationState,
    ObjectActionHistory,
    ProviderHealth,
    AdminAlerts,
    SuppressedAlerts,
    OnedriveAuth,
> {
    pub runtime: Runtime,
    pub admin_client_ip: Option<String>,
    pub admin_auth: AdminAuth,
    pub gateway_backup: GatewayBackup,
    pub gateway_write_ahead_log: GatewayWriteAheadLog,
    pub desktop_mount_seed: DesktopMountSeed,
    pub topology_provider_catalog: TopologyProviderCatalog,
    pub provider_bridge_catalog_seed: BTreeMap<String, serde_json::Value>,
    pub provider_limit_probes: ProviderLimitProbes,
    pub provider_limit_profiles: ProviderLimitProfiles,
    pub monitoring: Monitoring,
    pub operations_overview: OperationsOverview,
    pub notify: Notify,
    pub runtime_topology: RuntimeTopology,
    pub desired_topology: DesiredTopology,
    pub replication_state: ReplicationState,
    pub object_action_history: ObjectActionHistory,
    pub object_action_history_limit: usize,
    pub provider_health: ProviderHealth,
    pub alerts: AdminAlerts,
    pub suppressed_alerts: SuppressedAlerts,
    pub onedrive_auth: OnedriveAuth,
}

fn default_true() -> bool {
    true
}

fn default_llm_reasoning_effort() -> Option<String> {
    Some("none".to_string())
}

pub fn route_contracts() -> Vec<AdminRouteContract> {
    vec![
        AdminRouteContract {
            id: "status",
            method: AdminApiMethod::Get,
            path: ROUTE_STATUS,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::AdminStatusPayload,
        },
        AdminRouteContract {
            id: "topology_update",
            method: AdminApiMethod::Post,
            path: ROUTE_TOPOLOGY_UPDATE,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::TopologyUpdateInput),
            response: AdminDtoKind::DesiredTopologyPayload,
        },
        AdminRouteContract {
            id: "auth_capture_policy_get",
            method: AdminApiMethod::Get,
            path: ROUTE_AUTH_CAPTURE_POLICY,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::AuthCapturePolicyPayload,
        },
        AdminRouteContract {
            id: "auth_capture_policy_update",
            method: AdminApiMethod::Post,
            path: ROUTE_AUTH_CAPTURE_POLICY,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::AuthCapturePolicyInput),
            response: AdminDtoKind::AuthCapturePolicyPayload,
        },
        AdminRouteContract {
            id: "provider_credentials_get",
            method: AdminApiMethod::Get,
            path: ROUTE_PROVIDER_CREDENTIALS,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ProviderCredentialPayload,
        },
        AdminRouteContract {
            id: "provider_credentials_update",
            method: AdminApiMethod::Post,
            path: ROUTE_PROVIDER_CREDENTIALS,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::ProviderCredentialInput),
            response: AdminDtoKind::ProviderCredentialPayload,
        },
        AdminRouteContract {
            id: "browser_flow_session_handoff",
            method: AdminApiMethod::Post,
            path: ROUTE_BROWSER_FLOW_SESSION_HANDOFF,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::BrowserFlowSessionHandoffInput),
            response: AdminDtoKind::BrowserFlowSessionHandoffPayload,
        },
        AdminRouteContract {
            id: "replication_retry_job",
            method: AdminApiMethod::Post,
            path: ROUTE_REPLICATION_RETRY_JOB,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ReplicationRetryPayload,
        },
        AdminRouteContract {
            id: "replication_dlq_list",
            method: AdminApiMethod::Get,
            path: ROUTE_REPLICATION_DLQ,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ReplicationDlqListPayload,
        },
        AdminRouteContract {
            id: "replication_dlq_replay_job",
            method: AdminApiMethod::Post,
            path: ROUTE_REPLICATION_DLQ_REPLAY_JOB,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ReplicationDlqReplayPayload,
        },
        AdminRouteContract {
            id: "replication_dlq_replay_target",
            method: AdminApiMethod::Post,
            path: ROUTE_REPLICATION_DLQ_REPLAY_TARGET,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ReplicationDlqTargetReplayPayload,
        },
        AdminRouteContract {
            id: "object_reconcile_preview",
            method: AdminApiMethod::Get,
            path: ROUTE_OBJECT_RECONCILE_PREVIEW,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::ObjectReconcilePreviewPayload,
        },
        AdminRouteContract {
            id: "object_reconcile_execute",
            method: AdminApiMethod::Post,
            path: ROUTE_OBJECT_RECONCILE_EXECUTE,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::ObjectReconcileExecuteInput),
            response: AdminDtoKind::ObjectReconcileExecutePayload,
        },
        AdminRouteContract {
            id: "admin_alert_suppressions_update",
            method: AdminApiMethod::Post,
            path: ROUTE_ALERT_SUPPRESSIONS,
            surface: AdminApiSurface::Operator,
            request: Some(AdminDtoKind::AdminAlertSuppressionInput),
            response: AdminDtoKind::SuppressedAdminAlertRecordList,
        },
        AdminRouteContract {
            id: "admin_login",
            method: AdminApiMethod::Post,
            path: ROUTE_ADMIN_LOGIN,
            surface: AdminApiSurface::Internal,
            request: Some(AdminDtoKind::AdminLoginInput),
            response: AdminDtoKind::AdminLoginPayload,
        },
        AdminRouteContract {
            id: "admin_logout",
            method: AdminApiMethod::Post,
            path: ROUTE_ADMIN_LOGOUT,
            surface: AdminApiSurface::Internal,
            request: None,
            response: AdminDtoKind::NoContent,
        },
        AdminRouteContract {
            id: "admin_change_password",
            method: AdminApiMethod::Post,
            path: ROUTE_ADMIN_CHANGE_PASSWORD,
            surface: AdminApiSurface::Internal,
            request: Some(AdminDtoKind::AdminChangePasswordInput),
            response: AdminDtoKind::AdminChangePasswordPayload,
        },
        AdminRouteContract {
            id: "admin_logs_list",
            method: AdminApiMethod::Get,
            path: ROUTE_ADMIN_LOGS,
            surface: AdminApiSurface::Operator,
            request: None,
            response: AdminDtoKind::AdminLogQueryResult,
        },
    ]
}

pub fn operator_route_contracts() -> Vec<AdminRouteContract> {
    route_contracts()
        .into_iter()
        .filter(|route| route.surface == AdminApiSurface::Operator)
        .collect()
}

pub fn internal_route_contracts() -> Vec<AdminRouteContract> {
    route_contracts()
        .into_iter()
        .filter(|route| route.surface == AdminApiSurface::Internal)
        .collect()
}

pub const OPERATOR_PROVIDER_CONTRACT: &[&str] = &["unicom", "telecom", "mobile"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminApiErrorCode {
    Unauthorized,
    Forbidden,
    BadRequest,
    BadGateway,
    NotImplemented,
    NotFound,
    ServiceUnavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminApiErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<AdminApiErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
}

impl AdminApiErrorResponse {
    pub fn with_code(error: impl Into<String>, code: AdminApiErrorCode) -> Self {
        Self {
            error: error.into(),
            code: Some(code),
            api_version: Some(ADMIN_API_VERSION.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLoginPayload {
    pub ok: bool,
    pub redirect_to: String,
    pub username: String,
    pub expires_at_unix_ms: u64,
    pub must_change_password: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminChangePasswordInput {
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminChangePasswordPayload {
    pub ok: bool,
    pub must_change_password: bool,
    pub password_changed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogEntryPayload {
    pub seq: u64,
    pub ts_unix_ms: u64,
    pub level: String,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogQueryResult {
    pub entries: Vec<AdminLogEntryPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub limit: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn contract_routes_distinguish_operator_and_internal_surfaces() {
        let routes = route_contracts();
        let operator_routes = operator_route_contracts();
        let internal_routes = internal_route_contracts();

        assert!(
            operator_routes
                .iter()
                .all(|route| route.surface == AdminApiSurface::Operator)
        );
        assert!(
            internal_routes
                .iter()
                .all(|route| route.surface == AdminApiSurface::Internal)
        );
        assert!(routes.iter().any(|route| route.path == ROUTE_STATUS));
        assert!(
            internal_routes
                .iter()
                .any(|route| route.path == ROUTE_ADMIN_LOGIN)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_PROVIDER_CREDENTIALS)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_REPLICATION_RETRY_JOB)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_REPLICATION_DLQ)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_REPLICATION_DLQ_REPLAY_JOB)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_REPLICATION_DLQ_REPLAY_TARGET)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_OBJECT_RECONCILE_PREVIEW)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_OBJECT_RECONCILE_EXECUTE)
        );
        assert!(
            routes
                .iter()
                .any(|route| route.path == ROUTE_ALERT_SUPPRESSIONS)
        );
    }

    #[test]
    fn contract_routes_cover_read_and_write_methods_for_mutable_resources() {
        let routes = route_contracts();
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_AUTH_CAPTURE_POLICY && route.method == AdminApiMethod::Get
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_AUTH_CAPTURE_POLICY && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_PROVIDER_CREDENTIALS && route.method == AdminApiMethod::Get
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_PROVIDER_CREDENTIALS && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_REPLICATION_RETRY_JOB && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_REPLICATION_DLQ && route.method == AdminApiMethod::Get
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_REPLICATION_DLQ_REPLAY_JOB && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_REPLICATION_DLQ_REPLAY_TARGET
                && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_OBJECT_RECONCILE_PREVIEW && route.method == AdminApiMethod::Get
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_OBJECT_RECONCILE_EXECUTE && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_ALERT_SUPPRESSIONS && route.method == AdminApiMethod::Post
        }));
        assert!(routes.iter().any(|route| {
            route.path == ROUTE_ADMIN_LOGS && route.method == AdminApiMethod::Get
        }));
    }

    #[test]
    fn route_contracts_reference_exported_dto_kinds() {
        let routes = route_contracts();
        let topology = routes
            .iter()
            .find(|route| route.id == "topology_update")
            .expect("topology route should be registered");
        assert_eq!(topology.request, Some(AdminDtoKind::TopologyUpdateInput));
        assert_eq!(topology.response, AdminDtoKind::DesiredTopologyPayload);

        let handoff = routes
            .iter()
            .find(|route| route.id == "browser_flow_session_handoff")
            .expect("handoff route should be registered");
        assert_eq!(
            handoff.request,
            Some(AdminDtoKind::BrowserFlowSessionHandoffInput)
        );
        assert_eq!(
            handoff.response,
            AdminDtoKind::BrowserFlowSessionHandoffPayload
        );

        let retry = routes
            .iter()
            .find(|route| route.id == "replication_retry_job")
            .expect("retry route should be registered");
        assert_eq!(retry.request, None);
        assert_eq!(retry.response, AdminDtoKind::ReplicationRetryPayload);

        let dlq = routes
            .iter()
            .find(|route| route.id == "replication_dlq_list")
            .expect("dlq list route should be registered");
        assert_eq!(dlq.request, None);
        assert_eq!(dlq.response, AdminDtoKind::ReplicationDlqListPayload);

        let reconcile = routes
            .iter()
            .find(|route| route.id == "object_reconcile_execute")
            .expect("object reconcile execute route should be registered");
        assert_eq!(
            reconcile.request,
            Some(AdminDtoKind::ObjectReconcileExecuteInput)
        );
        assert_eq!(
            reconcile.response,
            AdminDtoKind::ObjectReconcileExecutePayload
        );

        let suppress = routes
            .iter()
            .find(|route| route.id == "admin_alert_suppressions_update")
            .expect("alert suppression route should be registered");
        assert_eq!(
            suppress.request,
            Some(AdminDtoKind::AdminAlertSuppressionInput)
        );
        assert_eq!(
            suppress.response,
            AdminDtoKind::SuppressedAdminAlertRecordList
        );

        let logs = routes
            .iter()
            .find(|route| route.id == "admin_logs_list")
            .expect("admin logs route should be registered");
        assert_eq!(logs.request, None);
        assert_eq!(logs.response, AdminDtoKind::AdminLogQueryResult);
    }

    #[test]
    fn exported_operator_dtos_have_stable_json_shapes() {
        let topology = TopologyUpdateInput {
            primary_provider: ProviderId::Unicom,
            sync_targets: vec![ProviderId::Telecom],
            fallback_read_order: vec![ProviderId::Mobile],
            high_speed_providers: vec![ProviderId::Unicom],
            write_targets: vec![ProviderId::Unicom],
            object_placement_mode: ObjectPlacementMode::PreferPrimary,
        };
        let topology_json = serde_json::to_value(&topology).expect("topology should serialize");
        assert_eq!(topology_json["primary_provider"], json!("unicom"));
        assert_eq!(
            topology_json["object_placement_mode"],
            json!("prefer_primary")
        );

        let credentials = ProviderCredentialPayload {
            provider: "unicom",
            label: "Unicom",
            storage_path: "/tmp/credentials/unicom.json".to_string(),
            token: None,
            token_present: true,
            browser_id: None,
            browser_id_present: false,
            cookie_header: None,
            cookie_header_present: true,
            family_id: Some("family-1".to_string()),
            root_folder_id: None,
            user_domain_id: None,
            client_id: None,
            tenant: None,
            drive_id: None,
            redirect_url: None,
            quota_min_free: None,
            quota_max_used: None,
            personal_quota_min_free: None,
            personal_quota_max_used: None,
            family_quota_min_free: None,
            family_quota_max_used: None,
            browser_profile: None,
            session_file: None,
            lease: None,
        };
        let credentials_json =
            serde_json::to_value(&credentials).expect("credentials should serialize");
        assert!(credentials_json["token"].is_null());
        assert_eq!(credentials_json["token_present"], json!(true));
        assert!(credentials_json["cookie_header"].is_null());
        assert_eq!(credentials_json["cookie_header_present"], json!(true));

        let handoff = BrowserFlowSessionHandoffPayload {
            session_id: "session-1".to_string(),
            provider: "unicom".to_string(),
            status: "completed".to_string(),
            handoff_at_unix_ms: 1_778_947_975_011,
            credential_keys: vec!["token".to_string()],
            runtime_keys: vec!["access_token".to_string()],
            audit_path: "/tmp/handoff/session-1.json".to_string(),
        };
        let handoff_json = serde_json::to_value(&handoff).expect("handoff should serialize");
        assert_eq!(handoff_json["credential_keys"], json!(["token"]));
        assert_eq!(handoff_json["runtime_keys"], json!(["access_token"]));

        let retry = ReplicationRetryPayload {
            job_id: 7,
            status: "pending".to_string(),
            target: "telecom".to_string(),
            bucket: "root".to_string(),
            key: "ops/f.txt".to_string(),
        };
        let retry_json = serde_json::to_value(&retry).expect("retry should serialize");
        assert_eq!(retry_json["job_id"], json!(7));
        assert_eq!(retry_json["status"], json!("pending"));

        let dlq_replay = ReplicationDlqTargetReplayPayload {
            target: "telecom".to_string(),
            replayed_jobs: 1,
            jobs: vec![ReplicationDlqReplayPayload {
                original_job_id: 10,
                replayed_job_id: 99,
                status: "pending".to_string(),
                target: "telecom".to_string(),
                bucket: "root".to_string(),
                key: "ops/dlq.txt".to_string(),
            }],
        };
        let dlq_json = serde_json::to_value(&dlq_replay).expect("dlq replay should serialize");
        assert_eq!(dlq_json["target"], json!("telecom"));
        assert_eq!(dlq_json["replayed_jobs"], json!(1));
        assert_eq!(dlq_json["jobs"][0]["original_job_id"], json!(10));

        let reconcile_input = ObjectReconcileExecuteInput {
            rows: vec![ObjectReconcileExecuteRowInput {
                bucket: "root".to_string(),
                key: "legacy/a.txt".to_string(),
            }],
            dry_run: true,
            operator: Some("ops".to_string()),
            ticket: Some("OPS-002".to_string()),
            notes: None,
        };
        let reconcile_json =
            serde_json::to_value(&reconcile_input).expect("reconcile input should serialize");
        assert_eq!(reconcile_json["dry_run"], json!(true));
        assert_eq!(reconcile_json["rows"][0]["bucket"], json!("root"));

        let suppress = AdminAlertSuppressionInput {
            alert_id: "x".to_string(),
            title: Some("y".to_string()),
            suppressed: true,
        };
        let suppress_json =
            serde_json::to_value(&suppress).expect("suppression input should serialize");
        assert_eq!(suppress_json["alert_id"], json!("x"));
    }

    #[test]
    fn error_response_keeps_legacy_error_field() {
        let payload = AdminApiErrorResponse::with_code("bad input", AdminApiErrorCode::BadRequest);
        let value = serde_json::to_value(payload).expect("serialize");
        assert_eq!(value["error"], json!("bad input"));
        assert_eq!(value["code"], json!("bad_request"));
        assert_eq!(value["api_version"], json!(ADMIN_API_VERSION));
    }

    #[test]
    fn error_response_can_be_deserialized_by_owned_clients() {
        let payload: AdminApiErrorResponse = serde_json::from_value(json!({
            "error": "login required",
            "code": "unauthorized",
            "api_version": ADMIN_API_VERSION
        }))
        .expect("owned client should deserialize error response");

        assert_eq!(payload.error, "login required");
        assert_eq!(payload.code, Some(AdminApiErrorCode::Unauthorized));
        assert_eq!(payload.api_version.as_deref(), Some(ADMIN_API_VERSION));
    }

    #[test]
    fn operator_provider_contract_excludes_onedrive() {
        assert!(!OPERATOR_PROVIDER_CONTRACT.contains(&"onedrive"));
    }
}
