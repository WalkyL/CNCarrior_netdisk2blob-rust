// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::client::ControlPlaneClient;
use crate::error::ServerError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
}

pub fn resource_registry() -> Vec<ResourceSchema> {
    vec![
        ResourceSchema {
            uri: URI_PROVIDER_STATUS_SUMMARY,
            name: "provider_status_summary",
            description: "Provider status summary from control-plane health.",
            mime_type: "application/json",
        },
        ResourceSchema {
            uri: URI_LATEST_FALLBACK_FAILURE_SUMMARY,
            name: "latest_fallback_replication_failure_summary",
            description: "Fallback-risk summary derived from replication status and latest failed job.",
            mime_type: "application/json",
        },
        ResourceSchema {
            uri: URI_REPLICATION_FAILED_QUEUE_SUMMARY,
            name: "replication_failed_queue_summary",
            description: "Replication failed queue summary with recent failed jobs.",
            mime_type: "application/json",
        },
        ResourceSchema {
            uri: URI_PORT_DEPLOYMENT_CONFIGURATION_SUMMARY,
            name: "port_deployment_configuration_summary",
            description: "Port and deployment configuration summary for MCP stdio slice.",
            mime_type: "application/json",
        },
    ]
}

pub fn read_resource<C: ControlPlaneClient>(client: &C, uri: &str) -> Result<Value, ServerError> {
    let text = match uri {
        URI_PROVIDER_STATUS_SUMMARY => serde_json::to_string(&client.provider_list()?)
            .map_err(|e| ServerError::Internal(e.to_string()))?,
        URI_LATEST_FALLBACK_FAILURE_SUMMARY => {
            let status = client.replication_get_status()?;
            let failed_jobs = client.replication_list_failed_jobs(1)?;
            let value = json!({
                "fallback_specific_events_available": false,
                "replication_status": status,
                "latest_replication_failed_job_sample": failed_jobs.jobs.into_iter().next(),
            });
            serde_json::to_string(&value).map_err(|e| ServerError::Internal(e.to_string()))?
        }
        URI_REPLICATION_FAILED_QUEUE_SUMMARY => {
            let status = client.replication_get_status()?;
            let jobs = client.replication_list_failed_jobs(10)?;
            let value = json!({
                "status": status,
                "failed_jobs_sample": jobs.jobs,
            });
            serde_json::to_string(&value).map_err(|e| ServerError::Internal(e.to_string()))?
        }
        URI_PORT_DEPLOYMENT_CONFIGURATION_SUMMARY => {
            serde_json::to_string(&client.deployment_config_summary()?)
                .map_err(|e| ServerError::Internal(e.to_string()))?
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
    use super::resource_registry;

    #[test]
    fn schemas_do_not_expose_secret_fields() {
        let banned = ["secret", "token", "password", "credential", "private_key"];
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
}
