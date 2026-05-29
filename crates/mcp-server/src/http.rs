// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use crate::client::ControlPlaneClient;
use crate::server::McpServer;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{ALLOW, AUTHORIZATION, CONTENT_TYPE, ORIGIN};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;
use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:61084";
const DEFAULT_ENDPOINT: &str = "/mcp";
const DEFAULT_ALLOWED_ORIGINS: &[&str] = &["http://localhost", "http://127.0.0.1"];
const ALLOW_POST: &str = "POST";
const MCP_PROTOCOL_VERSION_HEADER: &str = "MCP-Protocol-Version";
const SUPPORTED_PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpTransportConfig {
    pub enabled: bool,
    pub bind_addr: String,
    pub endpoint: String,
    pub bearer_token: Option<String>,
    pub allowed_origins: Vec<String>,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: DEFAULT_BIND_ADDR.to_string(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            bearer_token: None,
            allowed_origins: DEFAULT_ALLOWED_ORIGINS
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

impl HttpTransportConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_env_lookup(|key| std::env::var(key).ok())
    }

    fn from_env_lookup<F>(lookup: F) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut cfg = Self::default();
        cfg.enabled = parse_bool(lookup("MCP_SERVER_HTTP_ENABLED").as_deref()).unwrap_or(false);
        if let Some(bind_addr) = lookup("MCP_SERVER_HTTP_BIND") {
            cfg.bind_addr = bind_addr;
        }
        if let Some(endpoint) = lookup("MCP_SERVER_HTTP_PATH") {
            cfg.endpoint = normalize_endpoint_path(&endpoint)?;
        }
        if let Some(token) = lookup("MCP_SERVER_HTTP_BEARER_TOKEN") {
            cfg.bearer_token = Some(token);
        }
        if let Some(origins) = lookup("MCP_SERVER_HTTP_ALLOWED_ORIGINS") {
            cfg.allowed_origins = parse_origin_list(&origins)?;
        }
        if cfg.enabled && cfg.bearer_token.is_none() {
            return Err("MCP_SERVER_HTTP_BEARER_TOKEN is required when HTTP is enabled".into());
        }
        Ok(cfg)
    }
}

pub async fn serve_http<C: ControlPlaneClient + 'static>(
    server: Arc<McpServer<C>>,
    config: HttpTransportConfig,
) -> io::Result<()> {
    let addr: SocketAddr = config.bind_addr.parse().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid MCP_SERVER_HTTP_BIND: {err}"),
        )
    })?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let app = build_router(server, config);
    axum::serve(listener, app)
        .await
        .map_err(|err| io::Error::other(err.to_string()))
}

struct AppState {
    server: Arc<dyn JsonRpcHandler>,
    bearer_token: String,
    allowed_origins: HashSet<String>,
}

fn build_router<C: ControlPlaneClient + 'static>(
    server: Arc<McpServer<C>>,
    config: HttpTransportConfig,
) -> Router {
    let state = AppState {
        server,
        bearer_token: config
            .bearer_token
            .expect("bearer token must be validated by caller"),
        allowed_origins: config.allowed_origins.into_iter().collect(),
    };
    Router::new()
        .route(
            &config.endpoint,
            post(post_mcp)
                .get(get_not_supported)
                .fallback(method_not_allowed),
        )
        .with_state(state)
}

trait JsonRpcHandler: Send + Sync {
    fn handle_jsonrpc_str(
        &self,
        raw: &str,
    ) -> Result<Option<serde_json::Value>, crate::error::McpErrorPayload>;
}

impl<C: ControlPlaneClient> JsonRpcHandler for McpServer<C> {
    fn handle_jsonrpc_str(
        &self,
        raw: &str,
    ) -> Result<Option<serde_json::Value>, crate::error::McpErrorPayload> {
        self.handle_jsonrpc_str(raw)
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            server: Arc::clone(&self.server),
            bearer_token: self.bearer_token.clone(),
            allowed_origins: self.allowed_origins.clone(),
        }
    }
}

async fn post_mcp(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(resp) = validate_auth_and_origin(&headers, &state) {
        return resp;
    }
    if let Some(resp) = validate_protocol_version(&headers) {
        return resp;
    }
    let raw = match std::str::from_utf8(&body) {
        Ok(raw) => raw,
        Err(_) => return bad_request("request body must be utf-8"),
    };
    match state.server.handle_jsonrpc_str(raw) {
        Ok(Some(value)) => (
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            [(CONTENT_TYPE, "application/json")],
            json!({"jsonrpc":"2.0","id":serde_json::Value::Null,"error":err}).to_string(),
        )
            .into_response(),
    }
}

async fn get_not_supported(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(resp) = validate_auth_and_origin(&headers, &state) {
        return resp;
    }
    if let Some(resp) = validate_protocol_version(&headers) {
        return resp;
    }
    method_not_supported()
}

async fn method_not_allowed(
    State(state): State<AppState>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    let _ = method;
    if let Some(resp) = validate_auth_and_origin(&headers, &state) {
        return resp;
    }
    if let Some(resp) = validate_protocol_version(&headers) {
        return resp;
    }
    method_not_supported()
}

fn method_not_supported() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(ALLOW, HeaderValue::from_static(ALLOW_POST))],
    )
        .into_response()
}

fn validate_auth_and_origin(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    let auth = match headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        Some(auth) => auth,
        None => return Some(StatusCode::UNAUTHORIZED.into_response()),
    };
    let expected = format!("Bearer {}", state.bearer_token);
    if auth != expected {
        return Some(StatusCode::UNAUTHORIZED.into_response());
    }
    if let Some(origin) = headers.get(ORIGIN) {
        let origin = match origin.to_str() {
            Ok(origin) => origin,
            Err(_) => return Some(StatusCode::FORBIDDEN.into_response()),
        };
        if !state.allowed_origins.contains(origin) {
            return Some(StatusCode::FORBIDDEN.into_response());
        }
    }
    None
}

fn validate_protocol_version(headers: &HeaderMap) -> Option<Response> {
    let Some(raw) = headers.get(MCP_PROTOCOL_VERSION_HEADER) else {
        return None;
    };
    let Ok(protocol_version) = raw.to_str() else {
        return Some(bad_request("MCP-Protocol-Version must be valid ASCII"));
    };
    if protocol_version == SUPPORTED_PROTOCOL_VERSION {
        return None;
    }
    Some(bad_request("unsupported MCP-Protocol-Version"))
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(CONTENT_TYPE, "application/json")],
        json!({"error":message}).to_string(),
    )
        .into_response()
}

fn parse_bool(raw: Option<&str>) -> Option<bool> {
    let raw = raw?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn normalize_endpoint_path(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("MCP_SERVER_HTTP_PATH must not be empty".into());
    }
    if raw.starts_with('/') {
        Ok(raw.to_string())
    } else {
        Ok(format!("/{raw}"))
    }
}

fn parse_origin_list(raw: &str) -> Result<Vec<String>, String> {
    let origins: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect();
    if origins.is_empty() {
        return Err("MCP_SERVER_HTTP_ALLOWED_ORIGINS must contain at least one origin".into());
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::{HttpTransportConfig, MCP_PROTOCOL_VERSION_HEADER, build_router};
    use crate::client::StubControlPlaneClient;
    use crate::server::McpServer;
    use reqwest::blocking::Client;
    use reqwest::header::{ACCEPT, AUTHORIZATION, ORIGIN};
    use serde_json::Value;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;

    #[test]
    fn default_config_keeps_stdio_mode_disabled() {
        let cfg = HttpTransportConfig::from_env_lookup(|_| None).expect("config");
        assert!(!cfg.enabled);
        assert_eq!(cfg.bind_addr, "127.0.0.1:61084");
        assert_eq!(cfg.endpoint, "/mcp");
    }

    #[test]
    fn enabled_http_requires_bearer_token() {
        let err = HttpTransportConfig::from_env_lookup(|key| match key {
            "MCP_SERVER_HTTP_ENABLED" => Some("true".to_string()),
            _ => None,
        })
        .expect_err("should fail");
        assert!(err.contains("MCP_SERVER_HTTP_BEARER_TOKEN"));
    }

    #[test]
    fn http_transport_auth_origin_and_methods() {
        let test_server = start_test_server();
        let addr = test_server.addr;

        let join = thread::spawn(move || {
            let client = Client::builder().build().expect("client");
            let base = format!("http://{}", addr);

            let ok = client
                .post(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(ACCEPT, "application/json, text/event-stream")
                .header(ORIGIN, "http://localhost")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(ok.status().as_u16(), 200);
            let v: Value = ok.json().expect("json");
            assert_eq!(v["result"]["protocolVersion"], "2025-03-26");

            let supported_version = client
                .post(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(MCP_PROTOCOL_VERSION_HEADER, "2025-03-26")
                .body(r#"{"jsonrpc":"2.0","id":2,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(supported_version.status().as_u16(), 200);

            let unsupported_version = client
                .post(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(MCP_PROTOCOL_VERSION_HEADER, "2025-06-18")
                .body(r#"{"jsonrpc":"2.0","id":3,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(unsupported_version.status().as_u16(), 400);

            let missing_auth = client
                .post(format!("{base}/mcp"))
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(missing_auth.status().as_u16(), 401);

            let invalid_auth = client
                .post(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer wrong")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(invalid_auth.status().as_u16(), 401);

            let invalid_origin = client
                .post(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .header(ORIGIN, "https://evil.example")
                .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
                .send()
                .expect("request");
            assert_eq!(invalid_origin.status().as_u16(), 403);

            let get_resp = client
                .get(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .expect("request");
            assert_eq!(get_resp.status().as_u16(), 405);
            assert_eq!(
                get_resp
                    .headers()
                    .get("allow")
                    .expect("allow header")
                    .to_str()
                    .expect("allow value"),
                "POST"
            );

            let delete_missing_auth = client
                .delete(format!("{base}/mcp"))
                .send()
                .expect("request");
            assert_eq!(delete_missing_auth.status().as_u16(), 401);

            let delete_with_auth = client
                .delete(format!("{base}/mcp"))
                .header(AUTHORIZATION, "Bearer secret")
                .send()
                .expect("request");
            assert_eq!(delete_with_auth.status().as_u16(), 405);
        });

        join.join().expect("test thread");
        test_server.shutdown();
    }

    #[test]
    fn notification_post_returns_accepted_with_no_body() {
        let test_server = start_test_server();
        let addr = test_server.addr;

        let client = Client::builder().build().expect("client");
        let resp = client
            .post(format!("http://{addr}/mcp"))
            .header(AUTHORIZATION, "Bearer secret")
            .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#)
            .send()
            .expect("request");
        assert_eq!(resp.status().as_u16(), 202);
        test_server.shutdown();
    }

    #[test]
    fn concurrent_http_requests_share_the_same_dispatch_path() {
        let test_server = start_test_server();
        let addr = test_server.addr;
        let mut handles = Vec::new();

        for request_id in 0_u64..8 {
            handles.push(thread::spawn(move || {
                let client = Client::builder().build().expect("client");
                let response = client
                    .post(format!("http://{addr}/mcp"))
                    .header(AUTHORIZATION, "Bearer secret")
                    .body(format!(
                        r#"{{"jsonrpc":"2.0","id":{request_id},"method":"tools/list"}}"#
                    ))
                    .send()
                    .expect("request");
                assert_eq!(response.status().as_u16(), 200);
                let body: Value = response.json().expect("json");
                assert_eq!(body["id"].as_u64(), Some(request_id));
                assert!(
                    body["result"]["tools"]
                        .as_array()
                        .is_some_and(|v| !v.is_empty())
                );
            }));
        }

        for handle in handles {
            handle.join().expect("request thread");
        }
        test_server.shutdown();
    }

    struct RunningServer {
        addr: SocketAddr,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl RunningServer {
        fn shutdown(mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
            if let Some(handle) = self.handle.take() {
                handle.join().expect("server thread");
            }
        }
    }

    fn start_test_server() -> RunningServer {
        let (addr_tx, addr_rx) = mpsc::channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let handle = thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            runtime.block_on(async move {
                let server = Arc::new(McpServer::new(StubControlPlaneClient));
                let cfg = HttpTransportConfig {
                    enabled: true,
                    bind_addr: "127.0.0.1:0".to_string(),
                    endpoint: "/mcp".to_string(),
                    bearer_token: Some("secret".to_string()),
                    allowed_origins: vec!["http://localhost".to_string()],
                };
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind");
                let addr = listener.local_addr().expect("local addr");
                addr_tx.send(addr).expect("send addr");
                let app = build_router(server, cfg);
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("serve");
            });
        });
        let addr = addr_rx.recv().expect("recv addr");
        RunningServer {
            addr,
            shutdown_tx: Some(shutdown_tx),
            handle: Some(handle),
        }
    }
}
