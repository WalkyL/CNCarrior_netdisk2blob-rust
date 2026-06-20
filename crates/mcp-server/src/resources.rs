// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::client::ControlPlaneClient;
use crate::error::ServerError;
use crate::prompts::prompt_registry;
use crate::schema::{McpAccess, tool_registry};
use crate::{MCP_PROTOCOL_VERSION, MCP_SERVER_NAME};
use admin_api::{
    ROUTE_AUTH_CAPTURE_POLICY, ROUTE_PROVIDER_CREDENTIALS, ROUTE_REPLICATION_DLQ,
    ROUTE_REPLICATION_DLQ_REPLAY_JOB, ROUTE_REPLICATION_DLQ_REPLAY_TARGET,
    ROUTE_REPLICATION_RETRY_JOB, ROUTE_STATUS, ROUTE_TOPOLOGY_UPDATE, operator_route_contracts,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const URI_PUBLIC_FEATURE_ACCESS_SUMMARY: &str = "ccbg://public/feature-access-summary";
pub const URI_PROVIDER_STATUS_SUMMARY: &str = "ccbg://status/provider-summary";
pub const URI_LATEST_FALLBACK_FAILURE_SUMMARY: &str =
    "ccbg://status/latest-fallback-failure-summary";
pub const URI_REPLICATION_FAILED_QUEUE_SUMMARY: &str =
    "ccbg://status/replication-failed-queue-summary";
pub const URI_PORT_DEPLOYMENT_CONFIGURATION_SUMMARY: &str = "ccbg://config/port-deployment-summary";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceSchema {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "mimeType")]
    pub mime_type: &'static str,
    #[serde(rename = "authRequired")]
    pub auth_required: bool,
    pub access: McpAccess,
}

pub fn resource_registry() -> Vec<ResourceSchema> {
    vec![
        ResourceSchema {
            uri: URI_PUBLIC_FEATURE_ACCESS_SUMMARY,
            name: "feature_access_summary",
            description: "Public MCP discovery summary including auth requirements.",
            mime_type: "application/json",
            auth_required: false,
            access: McpAccess::PublicDiscovery,
        },
        ResourceSchema {
            uri: URI_PROVIDER_STATUS_SUMMARY,
            name: "provider_status_summary",
            description: "Provider status summary from control-plane health.",
            mime_type: "application/json",
            auth_required: true,
            access: McpAccess::Operator,
        },
        ResourceSchema {
            uri: URI_LATEST_FALLBACK_FAILURE_SUMMARY,
            name: "latest_fallback_replication_failure_summary",
            description: "Fallback-risk summary derived from replication status and latest failed job.",
            mime_type: "application/json",
            auth_required: true,
            access: McpAccess::Operator,
        },
        ResourceSchema {
            uri: URI_REPLICATION_FAILED_QUEUE_SUMMARY,
            name: "replication_failed_queue_summary",
            description: "Replication failed queue summary with recent failed jobs.",
            mime_type: "application/json",
            auth_required: true,
            access: McpAccess::Operator,
        },
        ResourceSchema {
            uri: URI_PORT_DEPLOYMENT_CONFIGURATION_SUMMARY,
            name: "port_deployment_configuration_summary",
            description: "Port and deployment configuration summary for MCP transports.",
            mime_type: "application/json",
            auth_required: true,
            access: McpAccess::Operator,
        },
    ]
}

pub fn is_public_resource(uri: &str) -> bool {
    resource_registry()
        .into_iter()
        .find(|resource| resource.uri == uri)
        .is_some_and(|resource| !resource.auth_required)
}

pub fn feature_access_summary() -> Result<Value, ServerError> {
    let admin_routes = operator_route_contracts();
    let extra_gatewayd_routes = vec![
        json!({
            "id": "applications_get",
            "method": "get",
            "path": "/api/applications",
            "surface": "operator",
            "request": Value::Null,
            "response": "json"
        }),
        json!({
            "id": "applications_update",
            "method": "post",
            "path": "/api/applications",
            "surface": "operator",
            "request": "json",
            "response": "json"
        }),
        json!({
            "id": "content_policies_get",
            "method": "get",
            "path": "/api/content-policies",
            "surface": "operator",
            "request": Value::Null,
            "response": "json"
        }),
        json!({
            "id": "content_policies_update",
            "method": "post",
            "path": "/api/content-policies",
            "surface": "operator",
            "request": "json",
            "response": "json"
        }),
    ];

    Ok(json!({
        "server": {
            "name": MCP_SERVER_NAME,
            "protocolVersion": MCP_PROTOCOL_VERSION,
        },
        "authentication": {
            "publicDiscoveryAvailableWithoutAuth": true,
            "operatorCallsRequireHttpBearerToken": true,
            "publicJsonRpcMethods": [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "resources/list",
                "resources/read for public resources",
                "prompts/list",
                "prompts/get for public prompts",
                "tools/call for public tools"
            ],
            "operatorJsonRpcMethods": [
                "tools/call for operator tools",
                "resources/read for operator resources",
                "prompts/get for operator prompts"
            ],
            "httpUnauthorizedHint": format!(
                "Call tools/list or read {} first, then authenticate for operator actions.",
                URI_PUBLIC_FEATURE_ACCESS_SUMMARY
            )
        },
        "tools": tool_registry(),
        "resources": resource_registry(),
        "prompts": prompt_registry(),
        "controlPlaneRoutes": {
            "sharedAdminApiOperatorContracts": admin_routes,
            "gatewaydOperatorJsonRoutes": extra_gatewayd_routes,
            "highlightedPaths": [
                ROUTE_STATUS,
                ROUTE_TOPOLOGY_UPDATE,
                ROUTE_AUTH_CAPTURE_POLICY,
                ROUTE_PROVIDER_CREDENTIALS,
                ROUTE_REPLICATION_RETRY_JOB,
                ROUTE_REPLICATION_DLQ,
                ROUTE_REPLICATION_DLQ_REPLAY_JOB,
                ROUTE_REPLICATION_DLQ_REPLAY_TARGET,
                "/api/applications",
                "/api/content-policies"
            ]
        }
    }))
}

pub fn read_resource<C: ControlPlaneClient>(client: &C, uri: &str) -> Result<Value, ServerError> {
    let text = match uri {
        URI_PUBLIC_FEATURE_ACCESS_SUMMARY => {
            serde_json::to_string_pretty(&feature_access_summary()?)
                .map_err(|err| ServerError::Internal(err.to_string()))?
        }
        URI_PROVIDER_STATUS_SUMMARY => serde_json::to_string(&client.provider_list()?)
            .map_err(|err| ServerError::Internal(err.to_string()))?,
        URI_LATEST_FALLBACK_FAILURE_SUMMARY => {
            let status = client.replication_get_status()?;
            let failed_jobs = client.replication_list_failed_jobs(1)?;
            let value = json!({
                "fallback_specific_events_available": false,
                "replication_status": status,
                "latest_replication_failed_job_sample": failed_jobs.jobs.into_iter().next(),
            });
            serde_json::to_string(&value).map_err(|err| ServerError::Internal(err.to_string()))?
        }
        URI_REPLICATION_FAILED_QUEUE_SUMMARY => {
            let status = client.replication_get_status()?;
            let jobs = client.replication_list_failed_jobs(10)?;
            let value = json!({
                "status": status,
                "failed_jobs_sample": jobs.jobs,
            });
            serde_json::to_string(&value).map_err(|err| ServerError::Internal(err.to_string()))?
        }
        URI_PORT_DEPLOYMENT_CONFIGURATION_SUMMARY => {
            serde_json::to_string(&client.deployment_config_summary()?)
                .map_err(|err| ServerError::Internal(err.to_string()))?
        }
        _ => {
            return Err(ServerError::NotFound(format!(
                "unknown resource uri: {uri}"
            )));
        }
    };

    Ok(json!({
        "contents": [
            {
                "uri": uri,
                "mimeType": "application/json",
                "text": text,
            }
        ]
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        URI_PUBLIC_FEATURE_ACCESS_SUMMARY, feature_access_summary, is_public_resource,
        resource_registry,
    };
    use crate::schema::McpAccess;
    use admin_api::{AdminApiMethod, AdminApiSurface, ROUTE_TOPOLOGY_UPDATE};
    use serde_json::json;

    #[test]
    fn public_summary_resource_is_marked_public() {
        let resources = resource_registry();
        let public = resources
            .iter()
            .find(|resource| resource.uri == URI_PUBLIC_FEATURE_ACCESS_SUMMARY)
            .expect("public resource");
        assert_eq!(public.access, McpAccess::PublicDiscovery);
        assert!(!public.auth_required);
    }

    #[test]
    fn public_resource_lookup_matches_registry_metadata() {
        assert!(is_public_resource(URI_PUBLIC_FEATURE_ACCESS_SUMMARY));
        assert!(!is_public_resource("ccbg://status/provider-summary"));
        assert!(!is_public_resource("ccbg://missing"));
    }

    #[test]
    fn feature_access_summary_contains_public_discovery_contract() {
        let summary = feature_access_summary().expect("summary");
        assert_eq!(summary["server"]["name"], crate::MCP_SERVER_NAME);
        assert_eq!(
            summary["server"]["protocolVersion"],
            crate::MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            summary["authentication"]["publicDiscoveryAvailableWithoutAuth"],
            true
        );
        assert!(
            summary["tools"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }

    #[test]
    fn resources_do_not_expose_high_risk_secret_field_names() {
        let banned = [
            "password",
            "private_key",
            "secret_access_key",
            "cookie_header",
        ];
        for resource in resource_registry() {
            let encoded = serde_json::to_string(&resource).expect("schema serializes");
            let encoded = encoded.to_ascii_lowercase();
            for term in banned {
                assert!(
                    !encoded.contains(term),
                    "resource {} unexpectedly contains banned term {}",
                    resource.name,
                    term
                );
            }
        }
    }

    #[test]
    fn registry_does_not_expose_onedrive() {
        let encoded = serde_json::to_string(&resource_registry()).expect("registry serializes");
        assert!(!encoded.to_ascii_lowercase().contains("onedrive"));
    }

    #[test]
    fn summary_references_operator_routes() {
        let summary = feature_access_summary().expect("summary");
        let routes = summary["controlPlaneRoutes"]["sharedAdminApiOperatorContracts"]
            .as_array()
            .expect("operator routes");
        assert!(routes.iter().any(|route| {
            route["path"] == json!(ROUTE_TOPOLOGY_UPDATE)
                && route["method"] == json!(AdminApiMethod::Post)
                && route["surface"] == json!(AdminApiSurface::Operator)
        }));
    }
}
