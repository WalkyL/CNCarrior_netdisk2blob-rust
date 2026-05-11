use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use blob_core::{
    BlobError, BrowserFlowElement, BrowserFlowOperation, BrowserFlowPage, BrowserFlowRequest,
    BrowserFlowSession,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};

type CdpSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CdpConnectionConfig {
    pub endpoint_url: String,
    #[serde(default)]
    pub target_selector: Option<String>,
    #[serde(default)]
    pub target_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CdpTargetSelector {
    WebSocketDebuggerUrl(String),
    TargetId(String),
    UrlPattern(String),
    TitleContains(String),
}

impl CdpTargetSelector {
    pub fn parse(raw: &str) -> Result<Self, BlobError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(BlobError::Configuration(
                "cdp target selector must not be empty".to_string(),
            ));
        }
        if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
            return Ok(Self::WebSocketDebuggerUrl(trimmed.to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("ws:") {
            return Ok(Self::WebSocketDebuggerUrl(value.trim().to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("target:") {
            return Ok(Self::TargetId(value.trim().to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("url:") {
            return Ok(Self::UrlPattern(value.trim().to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("title:") {
            return Ok(Self::TitleContains(value.trim().to_string()));
        }

        Ok(Self::UrlPattern(trimmed.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CdpTargetDescriptor {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub web_socket_debugger_url: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonVersionDescriptor {
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonTargetDescriptor {
    id: Option<String>,
    title: Option<String>,
    url: Option<String>,
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: Option<String>,
    #[serde(rename = "type")]
    r#type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdpTargetInfo {
    #[serde(rename = "targetId")]
    target_id: String,
    title: String,
    url: String,
    #[serde(rename = "type")]
    r#type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CdpTargetGetTargetsResponse {
    #[serde(rename = "targetInfos")]
    target_infos: Vec<CdpTargetInfo>,
}

#[derive(Debug, Deserialize)]
struct CdpMessageEnvelope {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<CdpErrorObject>,
}

#[derive(Debug, Deserialize)]
struct CdpErrorObject {
    #[serde(default)]
    code: Option<i64>,
    message: String,
}

pub struct CdpBrowserFlowSession {
    connection: Arc<CdpConnection>,
}

struct CdpConnection {
    socket: Mutex<CdpSocket>,
    next_id: AtomicU64,
}

impl CdpConnection {
    async fn send_command(&self, method: &str, params: Value) -> Result<Option<Value>, BlobError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let payload = json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let mut socket = self.socket.lock().await;
        socket
            .send(Message::Text(payload.to_string().into()))
            .await
            .map_err(|error| {
                BlobError::Upstream(format!("failed to send CDP command {method}: {error}"))
            })?;

        loop {
            let message = socket.next().await.ok_or_else(|| {
                BlobError::Upstream(format!("cdp socket closed while waiting for {method}"))
            })?;
            let message = message.map_err(|error| {
                BlobError::Upstream(format!(
                    "failed to receive CDP response for {method}: {error}"
                ))
            })?;
            let Message::Text(text) = message else {
                continue;
            };
            let envelope: CdpMessageEnvelope = serde_json::from_str(&text).map_err(|error| {
                BlobError::Upstream(format!("invalid CDP payload for {method}: {error}"))
            })?;
            if envelope.id != Some(id) {
                continue;
            }
            if let Some(error) = envelope.error {
                let code = error.code.unwrap_or_default();
                return Err(BlobError::Upstream(format!(
                    "cdp command {method} failed ({code}): {}",
                    error.message
                )));
            }
            return Ok(envelope.result);
        }
    }
}

impl CdpBrowserFlowSession {
    pub async fn connect(config: &CdpConnectionConfig) -> Result<Self, BlobError> {
        let endpoint_url = config.endpoint_url.trim();
        if endpoint_url.is_empty() {
            return Err(BlobError::Configuration(
                "cdp endpoint_url must not be empty".to_string(),
            ));
        }

        let http = HttpClient::builder().build().map_err(|error| {
            BlobError::Upstream(format!("failed to build CDP HTTP client: {error}"))
        })?;
        let websocket_url =
            resolve_websocket_url(&http, endpoint_url, config.target_selector.as_deref()).await?;

        let (socket, _) = connect_async(&websocket_url).await.map_err(|error| {
            BlobError::Upstream(format!(
                "failed to connect CDP websocket {websocket_url}: {error}"
            ))
        })?;
        let connection = Arc::new(CdpConnection {
            socket: Mutex::new(socket),
            next_id: AtomicU64::new(1),
        });

        for method in [
            "Page.enable",
            "Runtime.enable",
            "DOM.enable",
            "Network.enable",
        ] {
            connection.send_command(method, json!({})).await?;
        }

        Ok(Self { connection })
    }

    pub async fn evaluate_value(&self, expression: &str) -> Result<Value, BlobError> {
        let result = self
            .connection
            .send_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        let result = result.ok_or_else(|| {
            BlobError::Upstream("missing Runtime.evaluate result payload".to_string())
        })?;
        extract_runtime_value(&result)
    }

    async fn resolve_node_id(&self, element: &BrowserFlowElement) -> Result<i64, BlobError> {
        let selector = element.selectors.first().ok_or_else(|| {
            BlobError::Configuration(format!(
                "browser flow element {} has no selectors",
                element.id
            ))
        })?;

        let expression = match selector.engine {
            blob_core::BrowserFlowSelectorEngine::Css => {
                format!("document.querySelector({:?})", selector.value)
            }
            blob_core::BrowserFlowSelectorEngine::Javascript => selector.value.clone(),
            blob_core::BrowserFlowSelectorEngine::Xpath => format!(
                "document.evaluate({:?}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue",
                selector.value
            ),
        };

        let remote_result = self
            .connection
            .send_command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": false,
                    "awaitPromise": true,
                }),
            )
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Runtime.evaluate node result payload".to_string())
            })?;
        let object_id = remote_result
            .get("result")
            .and_then(|value| value.get("objectId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                BlobError::NotFound(format!("cdp could not resolve DOM node for {}", element.id))
            })?;

        let node = self
            .connection
            .send_command("DOM.requestNode", json!({ "objectId": object_id }))
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing DOM.requestNode result payload".to_string())
            })?;
        node.get("nodeId")
            .and_then(Value::as_i64)
            .ok_or_else(|| BlobError::Upstream("missing DOM.requestNode nodeId".to_string()))
    }
}

#[async_trait]
impl BrowserFlowSession for CdpBrowserFlowSession {
    async fn navigate(&self, url: &str) -> Result<(), BlobError> {
        self.connection
            .send_command("Page.navigate", json!({ "url": url }))
            .await?;
        Ok(())
    }

    async fn click(&self, element: &BrowserFlowElement) -> Result<(), BlobError> {
        let expression = javascript_selector_expression(element)?;
        let script = format!(
            "(() => {{ const el = ({expression}); if (!el) {{ throw new Error('element not found'); }} el.click(); return true; }})()"
        );
        self.evaluate_value(&script).await?;
        Ok(())
    }

    async fn set_input(
        &self,
        element: &BrowserFlowElement,
        value: &str,
        dispatch_events: &[String],
    ) -> Result<(), BlobError> {
        let events_json = serde_json::to_string(dispatch_events).map_err(|error| {
            BlobError::Configuration(format!("failed to encode dispatch events: {error}"))
        })?;
        let expression = javascript_selector_expression(element)?;
        let script = format!(
            "(() => {{ const el = ({expression}); if (!el) {{ throw new Error('element not found'); }} el.value = {value:?}; for (const eventName of {events_json}) {{ el.dispatchEvent(new Event(eventName, {{ bubbles: true }})); }} return true; }})()"
        );
        self.evaluate_value(&script).await?;
        Ok(())
    }

    async fn invoke_operation(&self, operation: &BrowserFlowOperation) -> Result<(), BlobError> {
        self.evaluate_value(&operation.source).await?;
        Ok(())
    }

    async fn set_files(
        &self,
        element: &BrowserFlowElement,
        paths: &[String],
    ) -> Result<(), BlobError> {
        let node_id = self.resolve_node_id(element).await?;
        self.connection
            .send_command(
                "DOM.setFileInputFiles",
                json!({
                    "nodeId": node_id,
                    "files": paths,
                }),
            )
            .await?;
        Ok(())
    }

    async fn dispatch_events(
        &self,
        element: &BrowserFlowElement,
        events: &[String],
    ) -> Result<(), BlobError> {
        let events_json = serde_json::to_string(events).map_err(|error| {
            BlobError::Configuration(format!("failed to encode dispatch events: {error}"))
        })?;
        let expression = javascript_selector_expression(element)?;
        let script = format!(
            "(() => {{ const el = ({expression}); if (!el) {{ throw new Error('element not found'); }} for (const eventName of {events_json}) {{ el.dispatchEvent(new Event(eventName, {{ bubbles: true }})); }} return true; }})()"
        );
        self.evaluate_value(&script).await?;
        Ok(())
    }

    async fn wait_for_request(
        &self,
        request: &BrowserFlowRequest,
        _timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let mut filter = BTreeMap::new();
        filter.insert("method", Value::String(request.method.clone()));
        filter.insert("url_pattern", Value::String(request.url_pattern.clone()));
        let expression = format!(
            "window.__ccbgLastRequestFilter = {}; true",
            serde_json::to_string(&filter).map_err(|error| {
                BlobError::Configuration(format!("failed to encode request wait filter: {error}"))
            })?
        );
        self.evaluate_value(&expression).await?;
        Ok(())
    }

    async fn wait_for_page(
        &self,
        page: &BrowserFlowPage,
        _timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let patterns = serde_json::to_string(&page.url_patterns).map_err(|error| {
            BlobError::Configuration(format!("failed to encode page wait patterns: {error}"))
        })?;
        let expression = format!(
            "(() => {{ const href = location.href; return {patterns}.some(pattern => pattern.endsWith('*') ? href.startsWith(pattern.slice(0, -1)) : href === pattern); }})()"
        );
        match self.evaluate_value(&expression).await? {
            Value::Bool(true) => Ok(()),
            _ => Err(BlobError::Upstream(format!(
                "page {} is not active in current CDP target",
                page.id
            ))),
        }
    }

    async fn wait(&self, duration_ms: u64) -> Result<(), BlobError> {
        tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
        Ok(())
    }
}

pub async fn discover_targets(endpoint_url: &str) -> Result<Vec<CdpTargetDescriptor>, BlobError> {
    let http = HttpClient::builder().build().map_err(|error| {
        BlobError::Upstream(format!("failed to build CDP HTTP client: {error}"))
    })?;
    fetch_targets(&http, endpoint_url).await
}

async fn resolve_websocket_url(
    http: &HttpClient,
    endpoint_url: &str,
    raw_selector: Option<&str>,
) -> Result<String, BlobError> {
    let selector = raw_selector.map(CdpTargetSelector::parse).transpose()?;
    if let Some(CdpTargetSelector::WebSocketDebuggerUrl(url)) = selector.clone() {
        return Ok(url);
    }

    if selector.is_none() {
        if let Some(url) = fetch_browser_websocket_url(http, endpoint_url).await? {
            return Ok(url);
        }
    }

    let targets = fetch_targets(http, endpoint_url).await?;
    let target = choose_target(&targets, selector.as_ref())?;
    target.web_socket_debugger_url.clone().ok_or_else(|| {
        BlobError::NotFound("selected CDP target does not expose webSocketDebuggerUrl".to_string())
    })
}

async fn fetch_browser_websocket_url(
    http: &HttpClient,
    endpoint_url: &str,
) -> Result<Option<String>, BlobError> {
    let url = format!("{}/json/version", endpoint_url.trim_end_matches('/'));
    let descriptor: JsonVersionDescriptor = http
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!(
                "failed to query CDP version endpoint {url}: {error}"
            ))
        })?
        .error_for_status()
        .map_err(|error| {
            BlobError::Upstream(format!(
                "CDP version endpoint {url} returned error: {error}"
            ))
        })?
        .json()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!(
                "failed to decode CDP version payload {url}: {error}"
            ))
        })?;
    Ok(descriptor.web_socket_debugger_url)
}

async fn fetch_targets(
    http: &HttpClient,
    endpoint_url: &str,
) -> Result<Vec<CdpTargetDescriptor>, BlobError> {
    let target_infos = match http_get_json_targets(http, endpoint_url, "/json").await {
        Ok(targets) if !targets.is_empty() => targets,
        _ => {
            let result = http_get_protocol_targets(http, endpoint_url).await?;
            result
                .into_iter()
                .map(|target| CdpTargetDescriptor {
                    id: Some(target.target_id),
                    title: Some(target.title),
                    url: Some(target.url),
                    web_socket_debugger_url: None,
                    r#type: Some(target.r#type),
                })
                .collect()
        }
    };
    Ok(target_infos)
}

async fn http_get_json_targets(
    http: &HttpClient,
    endpoint_url: &str,
    suffix: &str,
) -> Result<Vec<CdpTargetDescriptor>, BlobError> {
    let url = format!("{}{}", endpoint_url.trim_end_matches('/'), suffix);
    let targets: Vec<JsonTargetDescriptor> = http
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!("failed to query CDP targets {url}: {error}"))
        })?
        .error_for_status()
        .map_err(|error| {
            BlobError::Upstream(format!(
                "CDP targets endpoint {url} returned error: {error}"
            ))
        })?
        .json()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!(
                "failed to decode CDP targets payload {url}: {error}"
            ))
        })?;
    Ok(targets
        .into_iter()
        .map(|target| CdpTargetDescriptor {
            id: target.id,
            title: target.title,
            url: target.url,
            web_socket_debugger_url: target.web_socket_debugger_url,
            r#type: target.r#type,
        })
        .collect())
}

async fn http_get_protocol_targets(
    http: &HttpClient,
    endpoint_url: &str,
) -> Result<Vec<CdpTargetInfo>, BlobError> {
    let url = format!(
        "{}/json/protocol-targets",
        endpoint_url.trim_end_matches('/')
    );
    let result: CdpTargetGetTargetsResponse = http
        .get(&url)
        .send()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!("failed to query protocol targets {url}: {error}"))
        })?
        .error_for_status()
        .map_err(|error| {
            BlobError::Upstream(format!(
                "protocol targets endpoint {url} returned error: {error}"
            ))
        })?
        .json()
        .await
        .map_err(|error| {
            BlobError::Upstream(format!(
                "failed to decode protocol targets payload {url}: {error}"
            ))
        })?;
    Ok(result.target_infos)
}

fn choose_target(
    targets: &[CdpTargetDescriptor],
    selector: Option<&CdpTargetSelector>,
) -> Result<CdpTargetDescriptor, BlobError> {
    let matches = |target: &&CdpTargetDescriptor| match selector {
        None => target.r#type.as_deref().is_none_or(|value| value == "page"),
        Some(CdpTargetSelector::TargetId(id)) => target.id.as_deref() == Some(id.as_str()),
        Some(CdpTargetSelector::UrlPattern(pattern)) => target
            .url
            .as_deref()
            .is_some_and(|url| wildcard_match(pattern, url)),
        Some(CdpTargetSelector::TitleContains(fragment)) => target
            .title
            .as_deref()
            .is_some_and(|title| title.contains(fragment)),
        Some(CdpTargetSelector::WebSocketDebuggerUrl(url)) => {
            target.web_socket_debugger_url.as_deref() == Some(url.as_str())
        }
    };

    targets
        .iter()
        .filter(matches)
        .find(|target| target.web_socket_debugger_url.is_some())
        .or_else(|| targets.iter().filter(matches).next())
        .cloned()
        .ok_or_else(|| BlobError::NotFound("no matching CDP target found".to_string()))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        value == pattern
    }
}

fn javascript_selector_expression(element: &BrowserFlowElement) -> Result<String, BlobError> {
    let selector = element.selectors.first().ok_or_else(|| {
        BlobError::Configuration(format!(
            "browser flow element {} has no selectors",
            element.id
        ))
    })?;
    let expression = match selector.engine {
        blob_core::BrowserFlowSelectorEngine::Css => {
            format!("document.querySelector({:?})", selector.value)
        }
        blob_core::BrowserFlowSelectorEngine::Javascript => selector.value.clone(),
        blob_core::BrowserFlowSelectorEngine::Xpath => format!(
            "document.evaluate({:?}, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue",
            selector.value
        ),
    };
    Ok(expression)
}

fn extract_runtime_value(payload: &Value) -> Result<Value, BlobError> {
    if payload
        .get("exceptionDetails")
        .is_some_and(|details| !details.is_null())
    {
        return Err(BlobError::Upstream(
            "CDP Runtime.evaluate returned exceptionDetails".to_string(),
        ));
    }

    let result = payload
        .get("result")
        .ok_or_else(|| BlobError::Upstream("missing Runtime.evaluate result".to_string()))?;
    if let Some(value) = result.get("value") {
        return Ok(value.clone());
    }
    if result
        .get("subtype")
        .and_then(Value::as_str)
        .is_some_and(|subtype| subtype == "null")
    {
        return Ok(Value::Null);
    }

    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::{
        CdpTargetDescriptor, CdpTargetSelector, choose_target, extract_runtime_value,
        wildcard_match,
    };
    use serde_json::json;

    #[test]
    fn cdp_target_selector_parses_supported_forms() {
        assert_eq!(
            CdpTargetSelector::parse("http://127.0.0.1:9222/devtools/page/1").unwrap(),
            CdpTargetSelector::UrlPattern("http://127.0.0.1:9222/devtools/page/1".to_string())
        );
        assert_eq!(
            CdpTargetSelector::parse("ws://127.0.0.1:9222/devtools/page/1").unwrap(),
            CdpTargetSelector::WebSocketDebuggerUrl(
                "ws://127.0.0.1:9222/devtools/page/1".to_string()
            )
        );
        assert_eq!(
            CdpTargetSelector::parse("title:pan.wo.cn").unwrap(),
            CdpTargetSelector::TitleContains("pan.wo.cn".to_string())
        );
        assert_eq!(
            CdpTargetSelector::parse("url:https://pan.wo.cn/*").unwrap(),
            CdpTargetSelector::UrlPattern("https://pan.wo.cn/*".to_string())
        );
        assert_eq!(
            CdpTargetSelector::parse("target:page-123").unwrap(),
            CdpTargetSelector::TargetId("page-123".to_string())
        );
    }

    #[test]
    fn choose_target_prefers_page_with_websocket_url() {
        let selected = choose_target(
            &[
                CdpTargetDescriptor {
                    id: Some("a".to_string()),
                    title: Some("background".to_string()),
                    url: Some("https://example.com".to_string()),
                    web_socket_debugger_url: None,
                    r#type: Some("service_worker".to_string()),
                },
                CdpTargetDescriptor {
                    id: Some("b".to_string()),
                    title: Some("pan.wo.cn".to_string()),
                    url: Some("https://pan.wo.cn/pan/file_list/all".to_string()),
                    web_socket_debugger_url: Some(
                        "ws://127.0.0.1:9222/devtools/page/b".to_string(),
                    ),
                    r#type: Some("page".to_string()),
                },
            ],
            Some(&CdpTargetSelector::UrlPattern(
                "https://pan.wo.cn/*".to_string(),
            )),
        )
        .expect("matching target should be found");
        assert_eq!(selected.id.as_deref(), Some("b"));
    }

    #[test]
    fn wildcard_match_supports_suffix_star() {
        assert!(wildcard_match(
            "https://pan.wo.cn/*",
            "https://pan.wo.cn/pan/file_list/all"
        ));
        assert!(!wildcard_match(
            "https://pan.wo.cn/*",
            "https://example.com"
        ));
    }

    #[test]
    fn extract_runtime_value_reads_value_payload() {
        let payload = json!({
            "result": {
                "type": "string",
                "value": "ok"
            }
        });
        assert_eq!(extract_runtime_value(&payload).unwrap(), json!("ok"));
    }
}
