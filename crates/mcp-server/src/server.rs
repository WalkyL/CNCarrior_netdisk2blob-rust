// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::client::ControlPlaneClient;
use crate::error::{ErrorCode, McpErrorPayload, ServerError};
use crate::prompts::{get_prompt, is_public_prompt, prompt_registry};
use crate::resources::{
    feature_access_summary, is_public_resource, read_resource, resource_registry,
    storage_access_model_summary,
};
use crate::schema::{
    TOOL_MCP_FEATURE_ACCESS_SUMMARY, TOOL_MCP_STORAGE_ACCESS_MODEL_SUMMARY, is_public_tool,
    tool_registry,
};
use crate::{MCP_PROTOCOL_VERSION, MCP_SERVER_NAME};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};

pub struct McpServer<C> {
    client: C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestAccess {
    PublicDiscovery,
    Operator,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RequestInspection {
    pub id: Option<Value>,
    pub access: RequestAccess,
}

impl<C: ControlPlaneClient> McpServer<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    pub fn serve_stdio<R: Read, W: Write>(&self, reader: R, writer: &mut W) -> std::io::Result<()> {
        let mut output = BufWriter::new(writer);
        for line in BufReader::new(reader).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let response = match self.handle_jsonrpc_str(&line) {
                Ok(response) => response,
                Err(err) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": err,
                })),
            };
            if let Some(response) = response {
                write_line(&mut output, &response)?;
            }
        }
        output.flush()
    }

    pub fn handle_jsonrpc_str(&self, raw: &str) -> Result<Option<Value>, McpErrorPayload> {
        let req = parse_jsonrpc_request(raw)?;
        Ok(self.handle_request(req))
    }

    fn handle_request(&self, req: JsonRpcRequest) -> Option<Value> {
        if req.id.is_none() {
            if req.method == "notifications/initialized" {
                return None;
            }
            return None;
        }
        let id = req.id.unwrap_or(Value::Null);
        match self.dispatch(&req.method, req.params.as_ref()) {
            Ok(result) => Some(json!({"jsonrpc":"2.0","id": id, "result": result})),
            Err(error) => Some(json!({"jsonrpc":"2.0","id": id, "error": error.to_payload()})),
        }
    }

    fn dispatch(&self, method: &str, params: Option<&Value>) -> Result<Value, ServerError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "serverInfo": {
                    "name": MCP_SERVER_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {
                    "tools": {},
                    "resources": {},
                    "prompts": {}
                },
            })),
            "tools/list" => Ok(json!({ "tools": tool_registry() })),
            "tools/call" => self.call_tool(params),
            "resources/list" => Ok(json!({ "resources": resource_registry() })),
            "resources/read" => self.read_resource(params),
            "prompts/list" => Ok(json!({ "prompts": prompt_registry() })),
            "prompts/get" => self.get_prompt(params),
            _ => Err(ServerError::NotFound(format!("unknown method: {method}"))),
        }
    }

    fn call_tool(&self, params: Option<&Value>) -> Result<Value, ServerError> {
        let params = params.ok_or_else(|| ServerError::BadRequest("missing params".into()))?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ServerError::BadRequest("missing tool name".into()))?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = match name {
            TOOL_MCP_FEATURE_ACCESS_SUMMARY => feature_access_summary(),
            TOOL_MCP_STORAGE_ACCESS_MODEL_SUMMARY => storage_access_model_summary(),
            "provider_list" => to_value(self.client.provider_list()?),
            "provider_health" => to_value(
                self.client
                    .provider_health(required_str(&args, "provider_id")?)?,
            ),
            "replication_get_status" => to_value(self.client.replication_get_status()?),
            "replication_list_failed_jobs" => to_value(
                self.client
                    .replication_list_failed_jobs(optional_limit(&args)?)?,
            ),
            "s3_list_buckets" => to_value(self.client.s3_list_buckets()?),
            "alerts_list_recent" => {
                to_value(self.client.alerts_list_recent(optional_limit(&args)?)?)
            }
            "admin_status_get" => self.client.admin_status_get(),
            "applications_get" => self.client.applications_get(),
            "applications_update" => self
                .client
                .applications_update(required_object_value(&args, "payload")?),
            "content_policies_get" => self.client.content_policies_get(),
            "content_policies_update" => self
                .client
                .content_policies_update(required_object_value(&args, "payload")?),
            "topology_update" => self
                .client
                .topology_update(required_object_value(&args, "payload")?),
            "provider_credentials_get" => self
                .client
                .provider_credentials_get(required_str(&args, "provider_id")?),
            "provider_credentials_update" => self.client.provider_credentials_update(
                required_str(&args, "provider_id")?,
                required_object_value(&args, "payload")?,
            ),
            "auth_capture_policy_get" => self.client.auth_capture_policy_get(),
            "auth_capture_policy_update" => self
                .client
                .auth_capture_policy_update(required_object_value(&args, "payload")?),
            "replication_dlq_list" => to_value(self.client.replication_dlq_list()?),
            "replication_retry_job" => to_value(
                self.client
                    .replication_retry_job(required_u64(&args, "job_id")?)?,
            ),
            "replication_dlq_replay_job" => to_value(
                self.client
                    .replication_dlq_replay_job(required_u64(&args, "job_id")?)?,
            ),
            "replication_dlq_replay_target" => to_value(
                self.client
                    .replication_dlq_replay_target(required_str(&args, "target")?)?,
            ),
            _ => Err(ServerError::NotFound(format!("unknown tool: {name}"))),
        }?;

        Ok(tool_result(result)?)
    }

    fn read_resource(&self, params: Option<&Value>) -> Result<Value, ServerError> {
        let params = params.ok_or_else(|| ServerError::BadRequest("missing params".into()))?;
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| ServerError::BadRequest("missing resource uri".into()))?;
        read_resource(&self.client, uri)
    }

    fn get_prompt(&self, params: Option<&Value>) -> Result<Value, ServerError> {
        let params = params.ok_or_else(|| ServerError::BadRequest("missing params".into()))?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ServerError::BadRequest("missing prompt name".into()))?;
        let arguments = params.get("arguments");
        get_prompt(name, arguments)
    }
}

pub(crate) fn inspect_jsonrpc_request(raw: &str) -> Result<RequestInspection, McpErrorPayload> {
    let req = parse_jsonrpc_request(raw)?;
    let access = classify_request_access(&req)?;
    Ok(RequestInspection { id: req.id, access })
}

fn parse_jsonrpc_request(raw: &str) -> Result<JsonRpcRequest, McpErrorPayload> {
    serde_json::from_str::<JsonRpcRequest>(raw).map_err(|err| {
        McpErrorPayload::new(
            ErrorCode::BadRequest,
            format!("invalid request json: {err}"),
        )
    })
}

fn classify_request_access(req: &JsonRpcRequest) -> Result<RequestAccess, McpErrorPayload> {
    match req.method.as_str() {
        "initialize"
        | "notifications/initialized"
        | "tools/list"
        | "resources/list"
        | "prompts/list" => Ok(RequestAccess::PublicDiscovery),
        "tools/call" => {
            let params = req
                .params
                .as_ref()
                .ok_or_else(|| bad_request_payload("missing params"))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request_payload("missing tool name"))?;
            if is_public_tool(name) || !tool_exists(name) {
                Ok(RequestAccess::PublicDiscovery)
            } else {
                Ok(RequestAccess::Operator)
            }
        }
        "resources/read" => {
            let params = req
                .params
                .as_ref()
                .ok_or_else(|| bad_request_payload("missing params"))?;
            let uri = params
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request_payload("missing resource uri"))?;
            if is_public_resource(uri) || !resource_exists(uri) {
                Ok(RequestAccess::PublicDiscovery)
            } else {
                Ok(RequestAccess::Operator)
            }
        }
        "prompts/get" => {
            let params = req
                .params
                .as_ref()
                .ok_or_else(|| bad_request_payload("missing params"))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| bad_request_payload("missing prompt name"))?;
            if is_public_prompt(name) || !prompt_exists(name) {
                Ok(RequestAccess::PublicDiscovery)
            } else {
                Ok(RequestAccess::Operator)
            }
        }
        _ => Ok(RequestAccess::PublicDiscovery),
    }
}

fn tool_exists(name: &str) -> bool {
    tool_registry().into_iter().any(|tool| tool.name == name)
}

fn resource_exists(uri: &str) -> bool {
    resource_registry()
        .into_iter()
        .any(|resource| resource.uri == uri)
}

fn prompt_exists(name: &str) -> bool {
    prompt_registry()
        .into_iter()
        .any(|prompt| prompt.name == name)
}

fn bad_request_payload(message: &str) -> McpErrorPayload {
    McpErrorPayload::new(ErrorCode::BadRequest, message)
}

fn tool_result(result: Value) -> Result<Value, ServerError> {
    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": serde_json::to_string(&result).map_err(|err| ServerError::Internal(err.to_string()))?,
            }
        ],
        "structuredContent": result,
        "isError": false
    }))
}

fn to_value<T: Serialize>(result: T) -> Result<Value, ServerError> {
    serde_json::to_value(result).map_err(|err| ServerError::Internal(err.to_string()))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ServerError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::BadRequest(format!("missing required string field: {key}")))
}

fn required_u64(args: &Value, key: &str) -> Result<u64, ServerError> {
    args.get(key).and_then(Value::as_u64).ok_or_else(|| {
        ServerError::BadRequest(format!("missing required unsigned integer field: {key}"))
    })
}

fn required_object_value(args: &Value, key: &str) -> Result<Value, ServerError> {
    let value = args
        .get(key)
        .ok_or_else(|| ServerError::BadRequest(format!("missing required object field: {key}")))?;
    if !value.is_object() {
        return Err(ServerError::BadRequest(format!(
            "field {key} must be a json object"
        )));
    }
    Ok(value.clone())
}

fn optional_limit(args: &Value) -> Result<usize, ServerError> {
    match args.get("limit") {
        None => Ok(50),
        Some(limit) => {
            let parsed = limit.as_u64().ok_or_else(|| {
                ServerError::BadRequest("limit must be an unsigned integer".into())
            })?;
            usize::try_from(parsed)
                .map_err(|_| ServerError::BadRequest("limit out of range".into()))
        }
    }
}

fn write_line<W: Write>(writer: &mut W, value: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

#[cfg(test)]
mod tests {
    use super::{McpServer, RequestAccess, inspect_jsonrpc_request};
    use crate::client::{
        AlertListResult, BucketListResult, ControlPlaneClient, DeploymentConfigSummary,
        FailedJobSummary, FailedJobsResult, ProviderHealthResult, ProviderListResult,
        ProviderSummary, ReplicationStatusResult, StubControlPlaneClient,
    };
    use crate::error::ServerError;
    use crate::prompts::{
        PROMPT_DESIGN_STORAGE_ACCESS_MAPPING, PROMPT_DISCOVER_FEATURE_ACCESS_MODEL,
    };
    use crate::resources::{URI_PUBLIC_FEATURE_ACCESS_SUMMARY, URI_PUBLIC_STORAGE_ACCESS_MODEL};
    use crate::schema::{TOOL_MCP_FEATURE_ACCESS_SUMMARY, TOOL_MCP_STORAGE_ACCESS_MODEL_SUMMARY};
    use serde_json::{Value, json};

    struct TestControlPlaneClient;

    impl ControlPlaneClient for TestControlPlaneClient {
        fn provider_list(&self) -> Result<ProviderListResult, ServerError> {
            Ok(ProviderListResult {
                providers: vec![ProviderSummary {
                    provider_id: "mobile".to_string(),
                    display_name: "CMCC".to_string(),
                    healthy: true,
                }],
            })
        }

        fn provider_health(&self, provider_id: &str) -> Result<ProviderHealthResult, ServerError> {
            Ok(ProviderHealthResult {
                provider_id: provider_id.to_string(),
                healthy: true,
                status: "healthy".to_string(),
            })
        }

        fn replication_get_status(&self) -> Result<ReplicationStatusResult, ServerError> {
            Ok(ReplicationStatusResult {
                healthy: false,
                pending_jobs: 2,
                failed_jobs: 1,
            })
        }

        fn replication_list_failed_jobs(
            &self,
            _limit: usize,
        ) -> Result<FailedJobsResult, ServerError> {
            Ok(FailedJobsResult {
                jobs: vec![FailedJobSummary {
                    job_id: "job-1".to_string(),
                    object_key: "b/k".to_string(),
                    failure_code: "network".to_string(),
                    last_attempt_unix_ms: 100,
                }],
            })
        }

        fn deployment_config_summary(&self) -> Result<DeploymentConfigSummary, ServerError> {
            Ok(DeploymentConfigSummary {
                base_url: "http://custom-control.example:9000".to_string(),
                status_path: "/statusz".to_string(),
                timeout_ms: 1234,
                max_retries: 9,
                api_key_present: true,
            })
        }

        fn s3_list_buckets(&self) -> Result<BucketListResult, ServerError> {
            Ok(BucketListResult { buckets: vec![] })
        }

        fn alerts_list_recent(&self, _limit: usize) -> Result<AlertListResult, ServerError> {
            Ok(AlertListResult { alerts: vec![] })
        }

        fn admin_status_get(&self) -> Result<Value, ServerError> {
            Ok(json!({"ok": true}))
        }

        fn applications_get(&self) -> Result<Value, ServerError> {
            Ok(json!({"applications": []}))
        }

        fn applications_update(&self, payload: Value) -> Result<Value, ServerError> {
            Ok(payload)
        }

        fn content_policies_get(&self) -> Result<Value, ServerError> {
            Ok(json!({"policies": []}))
        }

        fn content_policies_update(&self, payload: Value) -> Result<Value, ServerError> {
            Ok(payload)
        }

        fn topology_update(&self, payload: Value) -> Result<Value, ServerError> {
            Ok(payload)
        }

        fn provider_credentials_get(&self, provider_id: &str) -> Result<Value, ServerError> {
            Ok(json!({"provider": provider_id, "token_present": true}))
        }

        fn provider_credentials_update(
            &self,
            provider_id: &str,
            payload: Value,
        ) -> Result<Value, ServerError> {
            Ok(json!({"provider": provider_id, "payload": payload}))
        }

        fn auth_capture_policy_get(&self) -> Result<Value, ServerError> {
            Ok(json!({"enabled": true}))
        }

        fn auth_capture_policy_update(&self, payload: Value) -> Result<Value, ServerError> {
            Ok(payload)
        }

        fn replication_dlq_list(
            &self,
        ) -> Result<admin_api::ReplicationDlqListPayload, ServerError> {
            Ok(admin_api::ReplicationDlqListPayload {
                entries: vec![],
                open_count: 0,
                returned_count: 0,
            })
        }

        fn replication_retry_job(
            &self,
            job_id: u64,
        ) -> Result<admin_api::ReplicationRetryPayload, ServerError> {
            Ok(admin_api::ReplicationRetryPayload {
                job_id,
                status: "retried".to_string(),
                target: "mobile".to_string(),
                bucket: "bucket".to_string(),
                key: "key".to_string(),
            })
        }

        fn replication_dlq_replay_job(
            &self,
            job_id: u64,
        ) -> Result<admin_api::ReplicationDlqReplayPayload, ServerError> {
            Ok(admin_api::ReplicationDlqReplayPayload {
                original_job_id: job_id,
                replayed_job_id: job_id + 1,
                status: "queued".to_string(),
                target: "mobile".to_string(),
                bucket: "bucket".to_string(),
                key: "key".to_string(),
            })
        }

        fn replication_dlq_replay_target(
            &self,
            target: &str,
        ) -> Result<admin_api::ReplicationDlqTargetReplayPayload, ServerError> {
            Ok(admin_api::ReplicationDlqTargetReplayPayload {
                target: target.to_string(),
                replayed_jobs: 0,
                jobs: vec![],
            })
        }
    }

    #[test]
    fn initialize_includes_tools_resources_and_prompts_capabilities() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let value: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(
            value["result"]["protocolVersion"],
            crate::MCP_PROTOCOL_VERSION
        );
        assert_eq!(
            value["result"]["serverInfo"]["name"],
            crate::MCP_SERVER_NAME
        );
        assert!(value["result"]["capabilities"]["tools"].is_object());
        assert!(value["result"]["capabilities"]["resources"].is_object());
        assert!(value["result"]["capabilities"]["prompts"].is_object());
    }

    #[test]
    fn initialized_notification_has_no_response() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        assert!(out.is_empty());
    }

    #[test]
    fn tools_list_uses_mcp_schema_field_names_and_access_metadata() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let first_tool = &response["result"]["tools"][0];
        assert!(first_tool.get("inputSchema").is_some());
        assert!(first_tool.get("outputSchema").is_some());
        assert!(first_tool.get("authRequired").is_some());
        assert!(first_tool.get("access").is_some());
        assert!(first_tool.get("mutating").is_some());
        assert!(first_tool.get("input_schema").is_none());
        assert!(first_tool.get("output_schema").is_none());
    }

    #[test]
    fn public_tool_call_returns_access_summary() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{}","arguments":{{}}}}}}"#,
            TOOL_MCP_FEATURE_ACCESS_SUMMARY
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(
            response["result"]["structuredContent"]["authentication"]["publicDiscoveryAvailableWithoutAuth"],
            true
        );
    }

    #[test]
    fn public_tool_call_returns_storage_access_model_summary() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{{"name":"{}","arguments":{{}}}}}}"#,
            TOOL_MCP_STORAGE_ACCESS_MODEL_SUMMARY
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(
            response["result"]["structuredContent"]["s3Compatibility"]["recommendedRegion"],
            "us-east-1"
        );
    }

    #[test]
    fn tools_call_returns_content_and_structured_content() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"provider_list","arguments":{}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert!(response["result"]["content"].is_array());
        assert!(response["result"]["structuredContent"].is_object());
        assert!(response["result"]["structuredContent"]["providers"].is_array());
        assert_eq!(
            response["result"]["structuredContent"]["providers"]
                .as_array()
                .expect("providers array")
                .len(),
            0
        );
    }

    #[test]
    fn operator_tool_update_requires_payload_object() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"applications_update","arguments":{"payload":"bad"}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(response["error"]["code"], "bad_request");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("json object"))
        );
    }

    #[test]
    fn resources_list_uses_mcp_schema_field_names() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":11,"method":"resources/list"}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let resources = response["result"]["resources"]
            .as_array()
            .expect("resources array");
        assert!(resources.len() >= 5);
        let first = &resources[0];
        assert!(first.get("uri").is_some());
        assert!(first.get("name").is_some());
        assert!(first.get("description").is_some());
        assert!(first.get("mimeType").is_some());
        assert!(first.get("authRequired").is_some());
    }

    #[test]
    fn public_resource_read_returns_feature_access_summary() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":12,"method":"resources/read","params":{{"uri":"{}"}}}}"#,
            URI_PUBLIC_FEATURE_ACCESS_SUMMARY
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let first = &response["result"]["contents"][0];
        assert_eq!(first["uri"], URI_PUBLIC_FEATURE_ACCESS_SUMMARY);
        let text = first["text"].as_str().expect("text");
        assert!(text.contains("publicDiscoveryAvailableWithoutAuth"));
    }

    #[test]
    fn public_resource_read_returns_storage_access_model_summary() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":18,"method":"resources/read","params":{{"uri":"{}"}}}}"#,
            URI_PUBLIC_STORAGE_ACCESS_MODEL
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let text = response["result"]["contents"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("bucket plus key prefix"));
    }

    #[test]
    fn resources_read_unknown_uri_returns_not_found() {
        let server = McpServer::new(StubControlPlaneClient);
        let input =
            br#"{"jsonrpc":"2.0","id":13,"method":"resources/read","params":{"uri":"ccbg://missing"}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(response["error"]["code"], "not_found");
        assert!(response["error"]["message"].is_string());
    }

    #[test]
    fn prompts_list_returns_expected_shape() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":14,"method":"prompts/list"}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let prompts = response["result"]["prompts"].as_array().expect("prompts");
        assert!(prompts.len() >= 5);
        let first = &prompts[0];
        assert!(first.get("name").is_some());
        assert!(first.get("description").is_some());
        assert!(first.get("authRequired").is_some());
    }

    #[test]
    fn public_prompt_get_returns_messages_shape() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":15,"method":"prompts/get","params":{{"name":"{}","arguments":{{}}}}}}"#,
            PROMPT_DISCOVER_FEATURE_ACCESS_MODEL
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let first_message = &response["result"]["messages"][0];
        assert_eq!(first_message["role"], "user");
        assert_eq!(first_message["content"]["type"], "text");
        assert!(first_message["content"]["text"].is_string());
    }

    #[test]
    fn public_prompt_get_returns_storage_mapping_guidance() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = format!(
            r#"{{"jsonrpc":"2.0","id":19,"method":"prompts/get","params":{{"name":"{}","arguments":{{}}}}}}"#,
            PROMPT_DESIGN_STORAGE_ACCESS_MAPPING
        );
        let mut out = Vec::new();
        server
            .serve_stdio(input.as_bytes(), &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let text = response["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("mcp_storage_access_model_summary"));
        assert!(text.contains("do not encode carrier choice in region"));
    }

    #[test]
    fn safe_object_read_prompt_only_mentions_available_flow() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":21,"method":"prompts/get","params":{"name":"safe_object_read","arguments":{"object_key":"a/b"}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let text = response["result"]["messages"][0]["content"]["text"]
            .as_str()
            .expect("text");
        assert!(!text.contains("serving provider"));
        assert!(text.contains("provider_list"));
        assert!(text.contains("replication_get_status"));
        assert!(text.contains("cannot infer serving-provider selection"));
    }

    #[test]
    fn fallback_failure_summary_is_replication_based() {
        let server = McpServer::new(TestControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":22,"method":"resources/read","params":{"uri":"ccbg://status/latest-fallback-failure-summary"}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let text = response["result"]["contents"][0]["text"]
            .as_str()
            .expect("text");
        let parsed: Value = serde_json::from_str(text).expect("json text payload");
        assert_eq!(parsed["fallback_specific_events_available"], false);
        assert_eq!(parsed["replication_status"]["failed_jobs"], 1);
        assert_eq!(
            parsed["latest_replication_failed_job_sample"]["job_id"],
            "job-1"
        );
    }

    #[test]
    fn deployment_config_resource_reflects_runtime_config_and_is_sanitized() {
        let server = McpServer::new(TestControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":23,"method":"resources/read","params":{"uri":"ccbg://config/port-deployment-summary"}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let text = response["result"]["contents"][0]["text"]
            .as_str()
            .expect("text");
        let parsed: Value = serde_json::from_str(text).expect("json text payload");
        assert_eq!(parsed["base_url"], "http://custom-control.example:9000");
        assert_eq!(parsed["status_path"], "/statusz");
        assert_eq!(parsed["timeout_ms"], 1234);
        assert_eq!(parsed["max_retries"], 9);
        assert_eq!(parsed["api_key_present"], true);
        assert!(parsed.get("api_key").is_none());
    }

    #[test]
    fn prompts_get_unknown_name_returns_not_found() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":16,"method":"prompts/get","params":{"name":"missing","arguments":{}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(response["error"]["code"], "not_found");
        assert!(response["error"]["message"].is_string());
    }

    #[test]
    fn request_inspection_marks_public_and_operator_calls() {
        let public = inspect_jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mcp_feature_access_summary","arguments":{}}}"#,
        )
        .expect("inspect public");
        assert_eq!(public.access, RequestAccess::PublicDiscovery);

        let operator = inspect_jsonrpc_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"provider_list","arguments":{}}}"#,
        )
        .expect("inspect operator");
        assert_eq!(operator.access, RequestAccess::Operator);
    }
}
