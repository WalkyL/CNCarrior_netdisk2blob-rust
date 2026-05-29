// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::client::ControlPlaneClient;
use crate::error::{McpErrorPayload, ServerError};
use crate::prompts::{get_prompt, prompt_registry};
use crate::resources::{read_resource, resource_registry};
use crate::schema::tool_registry;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};

pub struct McpServer<C> {
    client: C,
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
        let req = serde_json::from_str::<JsonRpcRequest>(raw).map_err(|err| {
            McpErrorPayload::new(
                crate::error::ErrorCode::BadRequest,
                format!("invalid request json: {err}"),
            )
        })?;
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
                "protocolVersion": "2025-03-26",
                "serverInfo": {
                    "name": "carrier-cloud-blob-gateway-mcp",
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
            _ => Err(ServerError::NotFound(format!("unknown tool: {name}"))),
        }?;

        Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string(&result).map_err(|e| ServerError::Internal(e.to_string()))?,
                }
            ],
            "structuredContent": result,
            "isError": false
        }))
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

fn to_value<T: Serialize>(result: T) -> Result<Value, ServerError> {
    serde_json::to_value(result).map_err(|e| ServerError::Internal(e.to_string()))
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ServerError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::BadRequest(format!("missing required string field: {key}")))
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
    use super::McpServer;
    use crate::client::{
        AlertListResult, BucketListResult, ControlPlaneClient, DeploymentConfigSummary,
        FailedJobSummary, FailedJobsResult, ProviderHealthResult, ProviderListResult,
        ProviderSummary, ReplicationStatusResult, StubControlPlaneClient,
    };
    use crate::error::ServerError;
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
        let v: Value = serde_json::from_slice(&out).expect("json response");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert!(v["result"]["capabilities"]["resources"].is_object());
        assert!(v["result"]["capabilities"]["prompts"].is_object());
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
    fn tools_list_uses_mcp_schema_field_names() {
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
        assert!(first_tool.get("input_schema").is_none());
        assert!(first_tool.get("output_schema").is_none());
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
    fn tools_call_bad_parameters_return_machine_readable_error() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"provider_health","arguments":{}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        assert_eq!(response["error"]["code"], "bad_request");
        assert!(response["error"]["message"].is_string());
        assert!(response["error"]["retryable"].is_boolean());
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
        assert!(resources.len() >= 4);
        let first = &resources[0];
        assert!(first.get("uri").is_some());
        assert!(first.get("name").is_some());
        assert!(first.get("description").is_some());
        assert!(first.get("mimeType").is_some());
    }

    #[test]
    fn resources_read_returns_contents_with_client_data() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":12,"method":"resources/read","params":{"uri":"ccbg://status/provider-summary"}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let first = &response["result"]["contents"][0];
        assert_eq!(first["uri"], "ccbg://status/provider-summary");
        assert_eq!(first["mimeType"], "application/json");
        let text = first["text"].as_str().expect("text");
        assert!(text.contains("providers"));
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
        assert!(prompts.len() >= 4);
        let first = &prompts[0];
        assert!(first.get("name").is_some());
        assert!(first.get("description").is_some());
    }

    #[test]
    fn prompts_get_returns_messages_shape() {
        let server = McpServer::new(StubControlPlaneClient);
        let input = br#"{"jsonrpc":"2.0","id":15,"method":"prompts/get","params":{"name":"safe_object_read","arguments":{"object_key":"a/b"}}}
"#;
        let mut out = Vec::new();
        server
            .serve_stdio(&input[..], &mut out)
            .expect("stdio works");
        let response: Value = serde_json::from_slice(&out).expect("json response");
        let first_message = &response["result"]["messages"][0];
        assert_eq!(first_message["role"], "user");
        assert_eq!(first_message["content"]["type"], "text");
        assert!(first_message["content"]["text"].is_string());
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
        assert_eq!(
            parsed,
            json!({
                "base_url": "http://custom-control.example:9000",
                "status_path": "/statusz",
                "timeout_ms": 1234,
                "max_retries": 9,
                "api_key_present": true
            })
        );
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
}
