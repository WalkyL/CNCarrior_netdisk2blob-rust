// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::error::ServerError;
use crate::schema::McpAccess;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PROMPT_DISCOVER_FEATURE_ACCESS_MODEL: &str = "discover_feature_access_model";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptArgumentSchema {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSchema {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgumentSchema>>,
    #[serde(rename = "authRequired")]
    pub auth_required: bool,
    pub access: McpAccess,
}

pub fn prompt_registry() -> Vec<PromptSchema> {
    vec![
        PromptSchema {
            name: PROMPT_DISCOVER_FEATURE_ACCESS_MODEL,
            description: "Explain which MCP features are public and which require authentication.",
            arguments: None,
            auth_required: false,
            access: McpAccess::PublicDiscovery,
        },
        PromptSchema {
            name: "safe_object_read",
            description: "Safely read one object with minimal blast radius.",
            arguments: Some(vec![PromptArgumentSchema {
                name: "object_key",
                description: "Object key to read.",
                required: true,
            }]),
            auth_required: true,
            access: McpAccess::Operator,
        },
        PromptSchema {
            name: "check_replication_before_fallback",
            description: "Check replication state before relying on fallback.",
            arguments: Some(vec![PromptArgumentSchema {
                name: "object_key",
                description: "Object key to validate.",
                required: true,
            }]),
            auth_required: true,
            access: McpAccess::Operator,
        },
        PromptSchema {
            name: "diagnose_provider_connection_failure",
            description: "Diagnose one provider connectivity failure.",
            arguments: Some(vec![PromptArgumentSchema {
                name: "provider_id",
                description: "Provider identifier to inspect.",
                required: true,
            }]),
            auth_required: true,
            access: McpAccess::Operator,
        },
        PromptSchema {
            name: "retry_replication_for_one_object",
            description: "Retry replication for one failed object.",
            arguments: Some(vec![PromptArgumentSchema {
                name: "object_key",
                description: "Object key to retry.",
                required: true,
            }]),
            auth_required: true,
            access: McpAccess::Operator,
        },
    ]
}

pub fn is_public_prompt(name: &str) -> bool {
    prompt_registry()
        .into_iter()
        .find(|prompt| prompt.name == name)
        .is_some_and(|prompt| !prompt.auth_required)
}

pub fn get_prompt(name: &str, arguments: Option<&Value>) -> Result<Value, ServerError> {
    let args = arguments.cloned().unwrap_or_else(|| json!({}));
    let text = match name {
        PROMPT_DISCOVER_FEATURE_ACCESS_MODEL => {
            "Discover CCBG MCP capabilities in two passes. First call tools/list, resources/list, and prompts/list. Then read ccbg://public/feature-access-summary or call mcp_feature_access_summary. Use the authRequired and access fields to separate public discovery from operator-only workflows.".to_string()
        }
        "safe_object_read" => {
            let object_key = required_str(&args, "object_key")?;
            format!(
                "Read object `{object_key}` safely. First call provider_list and replication_get_status. Inspect provider_list for healthy providers. If no healthy providers or replication is unhealthy, stop and report a sanitized reason. This MCP slice cannot infer serving-provider selection; proceed conservatively."
            )
        }
        "check_replication_before_fallback" => {
            let object_key = required_str(&args, "object_key")?;
            format!(
                "Before using fallback for `{object_key}`, call replication_get_status and replication_list_failed_jobs(limit=10). Confirm failed_jobs and pending_jobs are acceptable. If not healthy, return a short risk warning and do not assume fallback consistency."
            )
        }
        "diagnose_provider_connection_failure" => {
            let provider_id = required_str(&args, "provider_id")?;
            format!(
                "Diagnose provider `{provider_id}` connection failure. Call provider_health(provider_id). If unhealthy, summarize status and next safe checks. Keep output concise and do not include sensitive values."
            )
        }
        "retry_replication_for_one_object" => {
            let object_key = required_str(&args, "object_key")?;
            format!(
                "Retry replication for `{object_key}`. First inspect replication_list_failed_jobs(limit=50) to locate matching job_id. Confirm replication_get_status. Prepare a minimal retry plan for that single object and include rollback and safety checks."
            )
        }
        _ => {
            return Err(ServerError::NotFound(format!(
                "unknown prompt name: {name}"
            )));
        }
    };

    Ok(json!({
        "description": "Operational prompt template",
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text,
                }
            }
        ]
    }))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ServerError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::BadRequest(format!("missing required string field: {key}")))
}

#[cfg(test)]
mod tests {
    use super::{PROMPT_DISCOVER_FEATURE_ACCESS_MODEL, is_public_prompt, prompt_registry};
    use crate::schema::McpAccess;

    #[test]
    fn registry_marks_public_and_operator_prompts() {
        let prompts = prompt_registry();
        let public = prompts
            .iter()
            .find(|prompt| prompt.name == PROMPT_DISCOVER_FEATURE_ACCESS_MODEL)
            .expect("public prompt");
        assert_eq!(public.access, McpAccess::PublicDiscovery);
        assert!(!public.auth_required);

        let operator = prompts
            .iter()
            .find(|prompt| prompt.name == "safe_object_read")
            .expect("operator prompt");
        assert_eq!(operator.access, McpAccess::Operator);
        assert!(operator.auth_required);
    }

    #[test]
    fn public_prompt_lookup_matches_registry_metadata() {
        assert!(is_public_prompt(PROMPT_DISCOVER_FEATURE_ACCESS_MODEL));
        assert!(!is_public_prompt("safe_object_read"));
        assert!(!is_public_prompt("missing"));
    }

    #[test]
    fn registry_does_not_expose_high_risk_secret_field_names() {
        let banned = [
            "password",
            "private_key",
            "secret_access_key",
            "cookie_header",
        ];
        for prompt in prompt_registry() {
            let encoded = serde_json::to_string(&prompt).expect("schema serializes");
            let encoded = encoded.to_ascii_lowercase();
            for term in banned {
                assert!(
                    !encoded.contains(term),
                    "prompt {} unexpectedly contains banned term {}",
                    prompt.name,
                    term
                );
            }
        }
    }

    #[test]
    fn registry_does_not_expose_onedrive() {
        let encoded = serde_json::to_string(&prompt_registry()).expect("registry serializes");
        assert!(!encoded.to_ascii_lowercase().contains("onedrive"));
    }
}
