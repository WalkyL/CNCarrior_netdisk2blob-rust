// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
}

pub fn tool_registry() -> Vec<ToolSchema> {
    vec![
        provider_list(),
        provider_health(),
        replication_get_status(),
        replication_list_failed_jobs(),
        s3_list_buckets(),
        alerts_list_recent(),
    ]
}

fn provider_list() -> ToolSchema {
    ToolSchema {
        name: "provider_list",
        title: "Provider List",
        description: "List configured providers with status summary only.",
        input_schema: object_schema(vec![]),
        output_schema: object_schema(vec![("providers", array_schema(provider_summary_schema()))]),
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
    }
}

fn s3_list_buckets() -> ToolSchema {
    ToolSchema {
        name: "s3_list_buckets",
        title: "S3 Buckets",
        description: "List S3 buckets visible to current control-plane policy.",
        input_schema: object_schema(vec![]),
        output_schema: object_schema(vec![("buckets", array_schema(bucket_summary_schema()))]),
    }
}

fn alerts_list_recent() -> ToolSchema {
    ToolSchema {
        name: "alerts_list_recent",
        title: "Recent Alerts",
        description: "List recent admin alerts with sanitized payloads.",
        input_schema: object_schema_with_optional(vec![], vec![("limit", integer_schema())]),
        output_schema: object_schema(vec![("alerts", array_schema(alert_summary_schema()))]),
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
    for (k, v) in required_props {
        properties.insert(k.to_string(), v);
        required.push(k);
    }
    for (k, v) in optional_props {
        properties.insert(k.to_string(), v);
    }

    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": required,
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
    use super::tool_registry;
    use std::collections::HashSet;

    #[test]
    fn registry_contains_expected_tools() {
        let registry = tool_registry();
        let names: HashSet<_> = registry.iter().map(|tool| tool.name).collect();

        assert_eq!(registry.len(), 6);
        assert!(names.contains("provider_list"));
        assert!(names.contains("provider_health"));
        assert!(names.contains("replication_get_status"));
        assert!(names.contains("replication_list_failed_jobs"));
        assert!(names.contains("s3_list_buckets"));
        assert!(names.contains("alerts_list_recent"));
    }

    #[test]
    fn schemas_do_not_expose_secret_fields() {
        let banned = ["secret", "token", "password", "credential", "private_key"];
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
            .find(|t| t.name == "alerts_list_recent")
            .expect("alerts_list_recent exists");
        let required = tool
            .input_schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array");
        assert!(!required.iter().any(|v| v.as_str() == Some("limit")));
    }
}
