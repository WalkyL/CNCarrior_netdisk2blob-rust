// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const TOOL_MCP_FEATURE_ACCESS_SUMMARY: &str = "mcp_feature_access_summary";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAccess {
    PublicDiscovery,
    Operator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
    #[serde(rename = "authRequired")]
    pub auth_required: bool,
    pub access: McpAccess,
    pub mutating: bool,
}

pub fn tool_registry() -> Vec<ToolSchema> {
    vec![
        mcp_feature_access_summary(),
        provider_list(),
        provider_health(),
        replication_get_status(),
        replication_list_failed_jobs(),
        s3_list_buckets(),
        alerts_list_recent(),
        admin_status_get(),
        applications_get(),
        applications_update(),
        content_policies_get(),
        content_policies_update(),
        topology_update(),
        provider_credentials_get(),
        provider_credentials_update(),
        auth_capture_policy_get(),
        auth_capture_policy_update(),
        replication_dlq_list(),
        replication_retry_job(),
        replication_dlq_replay_job(),
        replication_dlq_replay_target(),
    ]
}

pub fn is_public_tool(name: &str) -> bool {
    tool_registry()
        .into_iter()
        .find(|tool| tool.name == name)
        .is_some_and(|tool| !tool.auth_required)
}

fn mcp_feature_access_summary() -> ToolSchema {
    ToolSchema {
        name: TOOL_MCP_FEATURE_ACCESS_SUMMARY,
        title: "Feature Access Summary",
        description: "Return MCP discovery and access requirements.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: false,
        access: McpAccess::PublicDiscovery,
        mutating: false,
    }
}

fn provider_list() -> ToolSchema {
    ToolSchema {
        name: "provider_list",
        title: "Provider List",
        description: "List configured providers with status summary only.",
        input_schema: object_schema(vec![]),
        output_schema: object_schema(vec![("providers", array_schema(provider_summary_schema()))]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn provider_health() -> ToolSchema {
    ToolSchema {
        name: "provider_health",
        title: "Provider Health",
        description: "Get provider health by provider id.",
        input_schema: object_schema(vec![("provider_id", string_schema())]),
        output_schema: object_schema(vec![
            ("provider_id", string_schema()),
            ("healthy", bool_schema()),
            ("status", string_schema()),
        ]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn replication_get_status() -> ToolSchema {
    ToolSchema {
        name: "replication_get_status",
        title: "Replication Status",
        description: "Read replication status counters and lag summary.",
        input_schema: object_schema(vec![]),
        output_schema: object_schema(vec![
            ("healthy", bool_schema()),
            ("pending_jobs", integer_schema()),
            ("failed_jobs", integer_schema()),
        ]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn replication_list_failed_jobs() -> ToolSchema {
    ToolSchema {
        name: "replication_list_failed_jobs",
        title: "Replication Failed Jobs",
        description: "List recent failed replication jobs using sanitized fields.",
        input_schema: object_schema_with_optional(vec![], vec![("limit", integer_schema())]),
        output_schema: object_schema(vec![(
            "jobs",
            array_schema(replication_failed_job_schema()),
        )]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn s3_list_buckets() -> ToolSchema {
    ToolSchema {
        name: "s3_list_buckets",
        title: "S3 Buckets",
        description: "List S3 buckets visible to current control-plane policy.",
        input_schema: object_schema(vec![]),
        output_schema: object_schema(vec![("buckets", array_schema(bucket_summary_schema()))]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn alerts_list_recent() -> ToolSchema {
    ToolSchema {
        name: "alerts_list_recent",
        title: "Recent Alerts",
        description: "List recent admin alerts with sanitized payloads.",
        input_schema: object_schema_with_optional(vec![], vec![("limit", integer_schema())]),
        output_schema: object_schema(vec![("alerts", array_schema(alert_summary_schema()))]),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn admin_status_get() -> ToolSchema {
    ToolSchema {
        name: "admin_status_get",
        title: "Admin Status",
        description: "Fetch the full operator status document from the control plane.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn applications_get() -> ToolSchema {
    ToolSchema {
        name: "applications_get",
        title: "Applications Get",
        description: "Fetch the current data-plane applications document.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn applications_update() -> ToolSchema {
    ToolSchema {
        name: "applications_update",
        title: "Applications Update",
        description: "Replace the current data-plane applications document with the provided payload.",
        input_schema: payload_wrapper_schema(),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn content_policies_get() -> ToolSchema {
    ToolSchema {
        name: "content_policies_get",
        title: "Content Policies Get",
        description: "Fetch current content policies.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn content_policies_update() -> ToolSchema {
    ToolSchema {
        name: "content_policies_update",
        title: "Content Policies Update",
        description: "Replace current content policies with the provided payload.",
        input_schema: payload_wrapper_schema(),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn topology_update() -> ToolSchema {
    ToolSchema {
        name: "topology_update",
        title: "Topology Update",
        description: "Update desired control-plane topology using the provided payload.",
        input_schema: payload_wrapper_schema(),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn provider_credentials_get() -> ToolSchema {
    ToolSchema {
        name: "provider_credentials_get",
        title: "Provider Credentials Get",
        description: "Fetch the provider credential record for one provider.",
        input_schema: object_schema(vec![("provider_id", string_schema())]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn provider_credentials_update() -> ToolSchema {
    ToolSchema {
        name: "provider_credentials_update",
        title: "Provider Credentials Update",
        description: "Update the provider credential record for one provider using the provided payload.",
        input_schema: object_schema(vec![
            ("provider_id", string_schema()),
            ("payload", any_object_schema()),
        ]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn auth_capture_policy_get() -> ToolSchema {
    ToolSchema {
        name: "auth_capture_policy_get",
        title: "Auth Capture Policy Get",
        description: "Fetch current auth-capture policy.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn auth_capture_policy_update() -> ToolSchema {
    ToolSchema {
        name: "auth_capture_policy_update",
        title: "Auth Capture Policy Update",
        description: "Update auth-capture policy using the provided payload.",
        input_schema: payload_wrapper_schema(),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn replication_dlq_list() -> ToolSchema {
    ToolSchema {
        name: "replication_dlq_list",
        title: "Replication DLQ List",
        description: "List replication dead-letter queue entries.",
        input_schema: object_schema(vec![]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: false,
    }
}

fn replication_retry_job() -> ToolSchema {
    ToolSchema {
        name: "replication_retry_job",
        title: "Replication Retry Job",
        description: "Retry one replication job by numeric job id.",
        input_schema: object_schema(vec![("job_id", integer_schema())]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn replication_dlq_replay_job() -> ToolSchema {
    ToolSchema {
        name: "replication_dlq_replay_job",
        title: "Replication DLQ Replay Job",
        description: "Replay one replication dead-letter entry by numeric job id.",
        input_schema: object_schema(vec![("job_id", integer_schema())]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn replication_dlq_replay_target() -> ToolSchema {
    ToolSchema {
        name: "replication_dlq_replay_target",
        title: "Replication DLQ Replay Target",
        description: "Replay all replication dead-letter entries for one target.",
        input_schema: object_schema(vec![("target", string_schema())]),
        output_schema: any_object_schema(),
        auth_required: true,
        access: McpAccess::Operator,
        mutating: true,
    }
}

fn provider_summary_schema() -> Value {
    object_schema(vec![
        ("provider_id", string_schema()),
        ("display_name", string_schema()),
        ("healthy", bool_schema()),
    ])
}

fn replication_failed_job_schema() -> Value {
    object_schema(vec![
        ("job_id", string_schema()),
        ("object_key", string_schema()),
        ("failure_code", string_schema()),
        ("last_attempt_unix_ms", integer_schema()),
    ])
}

fn bucket_summary_schema() -> Value {
    object_schema(vec![
        ("bucket", string_schema()),
        ("region", string_schema()),
    ])
}

fn alert_summary_schema() -> Value {
    object_schema(vec![
        ("alert_id", string_schema()),
        ("severity", string_schema()),
        ("summary", string_schema()),
        ("created_at_unix_ms", integer_schema()),
    ])
}

fn object_schema(props: Vec<(&str, Value)>) -> Value {
    object_schema_with_optional(props, vec![])
}

fn object_schema_with_optional(
    required_props: Vec<(&str, Value)>,
    optional_props: Vec<(&str, Value)>,
) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (key, value) in required_props {
        properties.insert(key.to_string(), value);
        required.push(key);
    }
    for (key, value) in optional_props {
        properties.insert(key.to_string(), value);
    }

    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
    })
}

fn payload_wrapper_schema() -> Value {
    object_schema(vec![("payload", any_object_schema())])
}

fn any_object_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
    })
}

fn array_schema(items: Value) -> Value {
    json!({
        "type": "array",
        "items": items,
    })
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn bool_schema() -> Value {
    json!({ "type": "boolean" })
}

fn integer_schema() -> Value {
    json!({ "type": "integer", "minimum": 0 })
}

#[cfg(test)]
mod tests {
    use super::{McpAccess, TOOL_MCP_FEATURE_ACCESS_SUMMARY, is_public_tool, tool_registry};
    use std::collections::HashSet;

    #[test]
    fn registry_contains_expected_tools() {
        let registry = tool_registry();
        let names: HashSet<_> = registry.iter().map(|tool| tool.name).collect();

        assert_eq!(registry.len(), 21);
        assert!(names.contains(TOOL_MCP_FEATURE_ACCESS_SUMMARY));
        assert!(names.contains("provider_list"));
        assert!(names.contains("applications_update"));
        assert!(names.contains("replication_dlq_replay_target"));
    }

    #[test]
    fn registry_marks_public_and_operator_tools() {
        let registry = tool_registry();
        let public = registry
            .iter()
            .find(|tool| tool.name == TOOL_MCP_FEATURE_ACCESS_SUMMARY)
            .expect("public tool");
        assert_eq!(public.access, McpAccess::PublicDiscovery);
        assert!(!public.auth_required);
        assert!(!public.mutating);

        let operator = registry
            .iter()
            .find(|tool| tool.name == "topology_update")
            .expect("operator tool");
        assert_eq!(operator.access, McpAccess::Operator);
        assert!(operator.auth_required);
        assert!(operator.mutating);
    }

    #[test]
    fn public_tool_lookup_matches_registry_metadata() {
        assert!(is_public_tool(TOOL_MCP_FEATURE_ACCESS_SUMMARY));
        assert!(!is_public_tool("provider_list"));
        assert!(!is_public_tool("missing"));
    }

    #[test]
    fn registry_does_not_expose_high_risk_secret_field_names() {
        let banned = [
            "password",
            "private_key",
            "secret_access_key",
            "cookie_header",
        ];
        for tool in tool_registry() {
            let encoded = serde_json::to_string(&tool).expect("schema serializes");
            let encoded = encoded.to_ascii_lowercase();
            for term in banned {
                assert!(
                    !encoded.contains(term),
                    "tool {} unexpectedly contains banned term {}",
                    tool.name,
                    term
                );
            }
        }
    }

    #[test]
    fn registry_does_not_expose_onedrive() {
        let encoded = serde_json::to_string(&tool_registry()).expect("registry serializes");
        assert!(!encoded.to_ascii_lowercase().contains("onedrive"));
    }

    #[test]
    fn optional_limit_is_not_required() {
        let registry = tool_registry();
        let tool = registry
            .iter()
            .find(|item| item.name == "alerts_list_recent")
            .expect("alerts_list_recent exists");
        let required = tool
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("required array");
        assert!(!required.iter().any(|value| value.as_str() == Some("limit")));
    }
}
