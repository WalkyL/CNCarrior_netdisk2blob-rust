use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
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
use tokio::sync::{Mutex, Notify, oneshot};
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
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct CdpErrorObject {
    #[serde(default)]
    code: Option<i64>,
    message: String,
}

#[derive(Debug, Clone, Default)]
struct CdpEventState {
    current_url: Option<String>,
    requests: Vec<CdpObservedRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdpObservedRequest {
    method: String,
    url: String,
}

#[derive(Clone)]
pub struct CdpBrowserFlowSession {
    connection: Arc<CdpConnection>,
}

struct CdpConnection {
    writer: Mutex<futures_util::stream::SplitSink<CdpSocket, Message>>,
    pending: Mutex<BTreeMap<u64, oneshot::Sender<Result<Option<Value>, BlobError>>>>,
    events: Mutex<CdpEventState>,
    event_notify: Notify,
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
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        if let Err(error) = self
            .writer
            .lock()
            .await
            .send(Message::Text(payload.to_string().into()))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(BlobError::Upstream(format!(
                "failed to send CDP command {method}: {error}"
            )));
        }

        rx.await.map_err(|error| {
            BlobError::Upstream(format!(
                "cdp response channel closed while waiting for {method}: {error}"
            ))
        })?
    }

    async fn wait_for_request_event(
        &self,
        request: &BrowserFlowRequest,
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(15_000));
        let future = async {
            loop {
                {
                    let events = self.events.lock().await;
                    if events.requests.iter().any(|item| {
                        item.method.eq_ignore_ascii_case(&request.method)
                            && wildcard_match(&request.url_pattern, &item.url)
                    }) {
                        return Ok(());
                    }
                }
                self.event_notify.notified().await;
            }
        };
        tokio::time::timeout(timeout, future).await.map_err(|_| {
            BlobError::Upstream(format!(
                "timed out waiting for CDP request {} {}",
                request.method, request.url_pattern
            ))
        })?
    }

    async fn wait_for_page_event(
        &self,
        page: &BrowserFlowPage,
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        let timeout = Duration::from_millis(timeout_ms.unwrap_or(15_000));
        let future = async {
            loop {
                {
                    let events = self.events.lock().await;
                    if events.current_url.as_deref().is_some_and(|url| {
                        page.url_patterns
                            .iter()
                            .any(|pattern| wildcard_match(pattern, url))
                    }) {
                        return Ok(());
                    }
                }
                self.event_notify.notified().await;
            }
        };
        tokio::time::timeout(timeout, future).await.map_err(|_| {
            BlobError::Upstream(format!("timed out waiting for CDP page {}", page.id))
        })?
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
        let websocket_url = resolve_websocket_url(
            &http,
            endpoint_url,
            config.target_selector.as_deref(),
            config.target_timeout_ms,
        )
        .await?;

        let (socket, _) = connect_async(&websocket_url).await.map_err(|error| {
            BlobError::Upstream(format!(
                "failed to connect CDP websocket {websocket_url}: {error}"
            ))
        })?;
        let (writer, reader) = socket.split();
        let connection = Arc::new(CdpConnection {
            writer: Mutex::new(writer),
            pending: Mutex::new(BTreeMap::new()),
            events: Mutex::new(CdpEventState::default()),
            event_notify: Notify::new(),
            next_id: AtomicU64::new(1),
        });
        spawn_reader_task(connection.clone(), reader);

        for method in [
            "Page.enable",
            "Runtime.enable",
            "DOM.enable",
            "Network.enable",
        ] {
            connection.send_command(method, json!({})).await?;
        }
        if let Ok(Value::String(href)) = (CdpBrowserFlowSession {
            connection: connection.clone(),
        })
        .evaluate_value("location.href")
        .await
        {
            let mut events = connection.events.lock().await;
            events.current_url = Some(href);
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

    pub async fn current_url(&self) -> Result<Option<String>, BlobError> {
        if let Some(url) = self.connection.events.lock().await.current_url.clone() {
            return Ok(Some(url));
        }

        match self.evaluate_value("location.href").await? {
            Value::String(href) => {
                let mut events = self.connection.events.lock().await;
                events.current_url = Some(href.clone());
                Ok(Some(href))
            }
            Value::Null => Ok(None),
            _ => Ok(None),
        }
    }

    async fn resolve_node_id(&self, element: &BrowserFlowElement) -> Result<i64, BlobError> {
        let expression = javascript_selector_expression(element)?;
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
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        self.connection
            .wait_for_request_event(request, timeout_ms)
            .await
    }

    async fn wait_for_page(
        &self,
        page: &BrowserFlowPage,
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError> {
        self.connection.wait_for_page_event(page, timeout_ms).await
    }

    async fn wait(&self, duration_ms: u64) -> Result<(), BlobError> {
        tokio::time::sleep(Duration::from_millis(duration_ms)).await;
        Ok(())
    }
}

pub async fn discover_targets(endpoint_url: &str) -> Result<Vec<CdpTargetDescriptor>, BlobError> {
    let http = HttpClient::builder().build().map_err(|error| {
        BlobError::Upstream(format!("failed to build CDP HTTP client: {error}"))
    })?;
    fetch_targets(&http, endpoint_url).await
}

fn spawn_reader_task(
    connection: Arc<CdpConnection>,
    mut reader: futures_util::stream::SplitStream<CdpSocket>,
) {
    tokio::spawn(async move {
        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let _ = handle_incoming_message(&connection, &text).await;
                }
                Ok(_) => {}
                Err(error) => {
                    fail_all_pending(
                        &connection,
                        BlobError::Upstream(format!("cdp reader error: {error}")),
                    )
                    .await;
                    return;
                }
            }
        }
        fail_all_pending(
            &connection,
            BlobError::Upstream("cdp websocket reader closed".to_string()),
        )
        .await;
    });
}

async fn handle_incoming_message(
    connection: &Arc<CdpConnection>,
    raw: &str,
) -> Result<(), BlobError> {
    let envelope: CdpMessageEnvelope = serde_json::from_str(raw)
        .map_err(|error| BlobError::Upstream(format!("invalid CDP message: {error}")))?;

    if let Some(id) = envelope.id {
        if let Some(tx) = connection.pending.lock().await.remove(&id) {
            let result = if let Some(error) = envelope.error {
                let code = error.code.unwrap_or_default();
                Err(BlobError::Upstream(format!(
                    "cdp command response failed ({code}): {}",
                    error.message
                )))
            } else {
                Ok(envelope.result)
            };
            let _ = tx.send(result);
        }
        return Ok(());
    }

    let Some(method) = envelope.method.as_deref() else {
        return Ok(());
    };
    let params = envelope.params.unwrap_or(Value::Null);
    record_event_state(connection, method, &params).await;
    Ok(())
}

async fn fail_all_pending(connection: &Arc<CdpConnection>, error: BlobError) {
    let mut pending = connection.pending.lock().await;
    let drained = std::mem::take(&mut *pending);
    drop(pending);
    for (_, tx) in drained {
        let _ = tx.send(Err(BlobError::Upstream(error.to_string())));
    }
}

async fn record_event_state(connection: &Arc<CdpConnection>, method: &str, params: &Value) {
    let changed = {
        let mut state = connection.events.lock().await;
        apply_event_state(&mut state, method, params)
    };
    if changed {
        connection.event_notify.notify_waiters();
    }
}

fn parse_network_request(params: &Value) -> Option<CdpObservedRequest> {
    let request = params.get("request")?;
    let method = request.get("method")?.as_str()?.to_string();
    let url = request.get("url")?.as_str()?.to_string();
    Some(CdpObservedRequest { method, url })
}

fn apply_event_state(state: &mut CdpEventState, method: &str, params: &Value) -> bool {
    match method {
        "Network.requestWillBeSent" => {
            if let Some(request) = parse_network_request(params) {
                state.requests.push(request);
                if state.requests.len() > 128 {
                    let drain = state.requests.len().saturating_sub(128);
                    state.requests.drain(0..drain);
                }
                true
            } else {
                false
            }
        }
        "Page.frameNavigated" => {
            if let Some(url) = params
                .get("frame")
                .and_then(|value| value.get("url"))
                .and_then(Value::as_str)
            {
                state.current_url = Some(url.to_string());
                true
            } else {
                false
            }
        }
        "Page.navigatedWithinDocument" => {
            if let Some(url) = params.get("url").and_then(Value::as_str) {
                state.current_url = Some(url.to_string());
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

async fn resolve_websocket_url(
    http: &HttpClient,
    endpoint_url: &str,
    raw_selector: Option<&str>,
    target_timeout_ms: Option<u64>,
) -> Result<String, BlobError> {
    let selector = raw_selector.map(CdpTargetSelector::parse).transpose()?;
    if let Some(CdpTargetSelector::WebSocketDebuggerUrl(url)) = selector.clone() {
        return validate_page_websocket_url(&url).map(ToString::to_string);
    }

    let timeout = target_timeout_ms
        .filter(|value| *value > 0)
        .map(Duration::from_millis);
    let deadline = timeout.map(|duration| tokio::time::Instant::now() + duration);
    let mut last_error;

    loop {
        match resolve_websocket_url_once(http, endpoint_url, selector.as_ref()).await {
            Ok(url) => return Ok(url),
            Err(error) => last_error = error,
        }

        let Some(deadline) = deadline else {
            return Err(last_error);
        };
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    Err(last_error)
}

async fn resolve_websocket_url_once(
    http: &HttpClient,
    endpoint_url: &str,
    selector: Option<&CdpTargetSelector>,
) -> Result<String, BlobError> {
    let targets = fetch_targets(http, endpoint_url).await?;
    let target = choose_target(&targets, selector)?;
    let url = target.web_socket_debugger_url.clone().ok_or_else(|| {
        BlobError::NotFound("selected CDP target does not expose webSocketDebuggerUrl".to_string())
    })?;
    validate_page_websocket_url(&url).map(ToString::to_string)
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

fn validate_page_websocket_url(url: &str) -> Result<&str, BlobError> {
    if url.contains("/devtools/browser/") {
        return Err(BlobError::Configuration(
            "browser-level CDP websocket is not supported; choose a page target selector instead"
                .to_string(),
        ));
    }
    Ok(url)
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
        CdpEventState, CdpObservedRequest, CdpTargetDescriptor, CdpTargetSelector,
        apply_event_state, choose_target, extract_runtime_value, parse_network_request,
        validate_page_websocket_url, wildcard_match,
    };
    use blob_core::BlobError;
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
    fn browser_level_websocket_url_is_rejected() {
        let error = validate_page_websocket_url("ws://127.0.0.1:9222/devtools/browser/abc")
            .expect_err("browser-level websocket should be rejected");
        assert!(matches!(error, BlobError::Configuration(_)));
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

    #[test]
    fn parse_network_request_extracts_method_and_url() {
        let params = json!({
            "request": {
                "method": "POST",
                "url": "https://panservice.mail.wo.cn/wohome/dispatcher"
            }
        });
        assert_eq!(
            parse_network_request(&params),
            Some(CdpObservedRequest {
                method: "POST".to_string(),
                url: "https://panservice.mail.wo.cn/wohome/dispatcher".to_string(),
            })
        );
    }

    #[test]
    fn apply_event_state_tracks_request_and_page_url() {
        let mut state = CdpEventState::default();

        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSent",
            &json!({
                "request": {
                    "method": "POST",
                    "url": "https://panservice.mail.wo.cn/wohome/dispatcher"
                }
            }),
        ));
        assert!(apply_event_state(
            &mut state,
            "Page.frameNavigated",
            &json!({
                "frame": {
                    "url": "https://pan.wo.cn/pan/file_list/all"
                }
            }),
        ));
        assert_eq!(state.requests.len(), 1);
        assert_eq!(
            state.current_url.as_deref(),
            Some("https://pan.wo.cn/pan/file_list/all")
        );
    }
}
