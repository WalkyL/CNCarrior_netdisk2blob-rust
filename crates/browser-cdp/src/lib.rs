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
    BrowserFlowSelector, BrowserFlowSelectorEngine, BrowserFlowSession,
    BrowserFlowVisualCaptchaRequest, BrowserFlowVisualLayoutValidationRequest,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Client as HttpClient, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, oneshot};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::protocol::Message,
};

type CdpSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const DEFAULT_ELEMENT_WAIT_TIMEOUT_MS: u64 = 10_000;
const ELEMENT_WAIT_POLL_MS: u64 = 200;

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

#[derive(Debug, Clone, Deserialize)]
struct CdpFrameTreeResponse {
    #[serde(rename = "frameTree")]
    frame_tree: CdpFrameTreeNode,
}

#[derive(Debug, Clone, Deserialize)]
struct CdpFrameTreeNode {
    frame: CdpFrameDescriptor,
    #[serde(default, rename = "childFrames")]
    child_frames: Vec<CdpFrameTreeNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdpFrameDescriptor {
    #[serde(rename = "id")]
    frame_id: String,
    #[serde(default)]
    name: String,
    url: String,
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
    pending_request_extra_headers: BTreeMap<String, BTreeMap<String, String>>,
    request_selections: BTreeMap<String, u64>,
    next_request_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CdpObservedRequest {
    sequence: u64,
    request_id: Option<String>,
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    post_data: Option<String>,
    response_status: Option<u16>,
    response_status_text: Option<String>,
    loading_failed_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CdpObservedRequestState {
    NotSeen,
    Pending,
    Succeeded(u64),
    Failed(String),
}

#[derive(Clone)]
pub struct CdpBrowserFlowSession {
    connection: Arc<CdpConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CdpElementRectSnapshot {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CdpElementSnapshot {
    pub tag_name: Option<String>,
    pub text: Option<String>,
    pub context_text: Option<String>,
    pub placeholder: Option<String>,
    pub class_name: Option<String>,
    pub input_type: Option<String>,
    pub src: Option<String>,
    pub visible: bool,
    pub rect: CdpElementRectSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CdpFrameSelector {
    FrameId(String),
    Name(String),
    UrlPattern(String),
}

impl CdpFrameSelector {
    fn parse(raw: &str) -> Result<Self, BlobError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(BlobError::Configuration(
                "cdp frame selector must not be empty".to_string(),
            ));
        }
        if let Some(value) = trimmed.strip_prefix("id:") {
            return Ok(Self::FrameId(value.trim().to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            return Ok(Self::Name(value.trim().to_string()));
        }
        if let Some(value) = trimmed.strip_prefix("url:") {
            return Ok(Self::UrlPattern(value.trim().to_string()));
        }
        Ok(Self::Name(trimmed.to_string()))
    }
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
        let start_sequence = {
            let events = self.events.lock().await;
            events.latest_request_sequence()
        };
        let future = async {
            loop {
                let state = {
                    let mut events = self.events.lock().await;
                    match events.observed_request_state_since(request, start_sequence) {
                        CdpObservedRequestState::Succeeded(sequence) => {
                            events
                                .request_selections
                                .insert(request.id.clone(), sequence);
                            CdpObservedRequestState::Succeeded(sequence)
                        }
                        other => other,
                    }
                };
                match state {
                    CdpObservedRequestState::Succeeded(_) => return Ok(()),
                    CdpObservedRequestState::Failed(message) => {
                        return Err(BlobError::Upstream(message));
                    }
                    CdpObservedRequestState::NotSeen | CdpObservedRequestState::Pending => {}
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

    async fn read_request_header(
        &self,
        request: &BrowserFlowRequest,
        header_name: &str,
    ) -> Result<Option<String>, BlobError> {
        let header_key = normalize_header_name(header_name);
        let events = self.events.lock().await;
        Ok(events
            .request_for_output(request)
            .and_then(|item| item.headers.get(&header_key))
            .cloned())
    }

    async fn read_request_field(
        &self,
        request: &BrowserFlowRequest,
        field_name: &str,
    ) -> Result<Option<String>, BlobError> {
        let field_name = field_name.trim();
        if field_name.is_empty() {
            return Ok(None);
        }
        let events = self.events.lock().await;
        Ok(events
            .request_for_output(request)
            .and_then(|item| item.field_value(field_name)))
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
                if let Some(url) = self.refresh_current_url().await? {
                    if page
                        .url_patterns
                        .iter()
                        .any(|pattern| wildcard_match(pattern, &url))
                    {
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

    async fn refresh_current_url(&self) -> Result<Option<String>, BlobError> {
        let result = self
            .send_command(
                "Runtime.evaluate",
                runtime_evaluate_params("location.href", true, None),
            )
            .await?;
        let value = extract_runtime_value(
            &result.ok_or_else(|| {
                BlobError::Upstream("missing Runtime.evaluate result payload".to_string())
            })?,
            "Runtime.evaluate",
        )?;
        match value {
            Value::String(href) => {
                let mut events = self.events.lock().await;
                events.current_url = Some(href.clone());
                Ok(Some(href))
            }
            Value::Null => Ok(None),
            _ => Ok(None),
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
        self.evaluate_value_in_frame(expression, None).await
    }

    pub async fn evaluate_value_in_frame(
        &self,
        expression: &str,
        frame_selector: Option<&str>,
    ) -> Result<Value, BlobError> {
        let result = self
            .evaluate_with_context(expression, frame_selector, true)
            .await?;
        extract_runtime_value(&result, "Runtime.evaluate")
    }

    async fn evaluate_with_context(
        &self,
        expression: &str,
        frame_selector: Option<&str>,
        return_by_value: bool,
    ) -> Result<Value, BlobError> {
        let context_id = match frame_selector {
            Some(selector) => Some(self.resolve_execution_context_id(selector).await?),
            None => None,
        };
        let result = self
            .connection
            .send_command(
                "Runtime.evaluate",
                runtime_evaluate_params(expression, return_by_value, context_id),
            )
            .await?;
        result.ok_or_else(|| {
            BlobError::Upstream("missing Runtime.evaluate result payload".to_string())
        })
    }

    pub async fn current_url(&self) -> Result<Option<String>, BlobError> {
        self.current_url_in_frame(None).await
    }

    pub async fn current_url_in_frame(
        &self,
        frame_selector: Option<&str>,
    ) -> Result<Option<String>, BlobError> {
        if frame_selector.is_none() {
            if let Some(url) = self.connection.events.lock().await.current_url.clone() {
                return Ok(Some(url));
            }
        }

        match self
            .evaluate_value_in_frame("location.href", frame_selector)
            .await?
        {
            Value::String(href) => {
                if frame_selector.is_none() {
                    let mut events = self.connection.events.lock().await;
                    events.current_url = Some(href.clone());
                }
                Ok(Some(href))
            }
            Value::Null => Ok(None),
            _ => Ok(None),
        }
    }

    async fn resolve_execution_context_id(&self, frame_selector: &str) -> Result<i64, BlobError> {
        let frame_id = self.resolve_frame_id(frame_selector).await?;
        let payload = self
            .connection
            .send_command(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": frame_id,
                    "worldName": "ccbg_browser_flow",
                    "grantUniveralAccess": false,
                }),
            )
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Page.createIsolatedWorld result payload".to_string())
            })?;
        payload
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "Page.createIsolatedWorld did not return executionContextId".to_string(),
                )
            })
    }

    async fn resolve_frame_id(&self, frame_selector: &str) -> Result<String, BlobError> {
        let selector = CdpFrameSelector::parse(frame_selector)?;
        let payload = self
            .connection
            .send_command("Page.getFrameTree", json!({}))
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Page.getFrameTree result payload".to_string())
            })?;
        let frame_tree: CdpFrameTreeResponse =
            serde_json::from_value(payload).map_err(|error| {
                BlobError::Upstream(format!("failed to decode CDP frame tree: {error}"))
            })?;
        choose_frame(&frame_tree.frame_tree, &selector)
            .map(|frame| frame.frame_id.clone())
            .ok_or_else(|| {
                BlobError::NotFound(format!("no matching CDP frame found: {frame_selector}"))
            })
    }

    pub async fn element_exists(&self, element: &BrowserFlowElement) -> Result<bool, BlobError> {
        let object_id = self.resolve_object_id_once(element).await?;
        if let Some(object_id) = object_id {
            self.release_object(&object_id).await;
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn capture_element_screenshot_png_base64(
        &self,
        element: &BrowserFlowElement,
    ) -> Result<String, BlobError> {
        let object_id = self.wait_for_object_id(element).await?;
        let clip = self
            .call_function_on_object(
                &object_id,
                "function() { if (typeof this.scrollIntoView === 'function') { this.scrollIntoView({ block: 'center', inline: 'center' }); } const rect = this.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; }",
                &[],
            )
            .await;
        self.release_object(&object_id).await;
        let clip = viewport_clip_from_value(&clip?, &element.id)?;
        tokio::time::sleep(Duration::from_millis(150)).await;

        let payload = self
            .connection
            .send_command(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "clip": {
                        "x": clip.x,
                        "y": clip.y,
                        "width": clip.width,
                        "height": clip.height,
                        "scale": 1,
                    }
                }),
            )
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Page.captureScreenshot result payload".to_string())
            })?;
        payload
            .get("data")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "Page.captureScreenshot did not return PNG image data".to_string(),
                )
            })
    }

    pub async fn capture_viewport_screenshot_png_base64(&self) -> Result<String, BlobError> {
        let payload = self
            .connection
            .send_command(
                "Page.captureScreenshot",
                json!({
                    "format": "png",
                    "fromSurface": true,
                }),
            )
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Page.captureScreenshot result payload".to_string())
            })?;
        payload
            .get("data")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                BlobError::Upstream(
                    "Page.captureScreenshot did not return viewport PNG image data".to_string(),
                )
            })
    }

    pub async fn inspect_element_snapshot(
        &self,
        element: &BrowserFlowElement,
    ) -> Result<Option<CdpElementSnapshot>, BlobError> {
        let expression = javascript_element_snapshot_expression(element)?;
        let value = self
            .evaluate_value_in_frame(&expression, element.frame.as_deref())
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|error| {
            BlobError::Upstream(format!(
                "failed to decode CDP element snapshot for {}: {error}",
                element.id
            ))
        })
    }

    async fn resolve_object_id_once(
        &self,
        element: &BrowserFlowElement,
    ) -> Result<Option<String>, BlobError> {
        let expression = javascript_element_resolver_expression(element)?;
        let remote_result = self
            .evaluate_with_context(&expression, element.frame.as_deref(), false)
            .await?;
        extract_runtime_object_id(&remote_result, "Runtime.evaluate")
    }

    pub async fn read_request_header(
        &self,
        request: &BrowserFlowRequest,
        header_name: &str,
    ) -> Result<Option<String>, BlobError> {
        self.connection
            .read_request_header(request, header_name)
            .await
    }

    pub async fn read_request_field(
        &self,
        request: &BrowserFlowRequest,
        field_name: &str,
    ) -> Result<Option<String>, BlobError> {
        self.connection
            .read_request_field(request, field_name)
            .await
    }

    async fn wait_for_object_id(&self, element: &BrowserFlowElement) -> Result<String, BlobError> {
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(DEFAULT_ELEMENT_WAIT_TIMEOUT_MS);

        loop {
            let error = match self.resolve_object_id_once(element).await {
                Ok(Some(object_id)) => return Ok(object_id),
                Ok(None) => BlobError::NotFound(format!(
                    "cdp could not resolve DOM node for {} within {} ms",
                    element.id, DEFAULT_ELEMENT_WAIT_TIMEOUT_MS
                )),
                Err(error) => error,
            };

            if tokio::time::Instant::now() >= deadline {
                return Err(error);
            }
            tokio::time::sleep(Duration::from_millis(ELEMENT_WAIT_POLL_MS)).await;
        }
    }

    async fn call_function_on_object(
        &self,
        object_id: &str,
        function_declaration: &str,
        arguments: &[Value],
    ) -> Result<Value, BlobError> {
        let payload = self
            .connection
            .send_command(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": function_declaration,
                    "arguments": arguments
                        .iter()
                        .cloned()
                        .map(|value| json!({ "value": value }))
                        .collect::<Vec<_>>(),
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?
            .ok_or_else(|| {
                BlobError::Upstream("missing Runtime.callFunctionOn result payload".to_string())
            })?;
        extract_runtime_value(&payload, "Runtime.callFunctionOn")
    }

    async fn release_object(&self, object_id: &str) {
        let _ = self
            .connection
            .send_command("Runtime.releaseObject", json!({ "objectId": object_id }))
            .await;
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
        let object_id = self.wait_for_object_id(element).await?;
        let result = self
            .call_function_on_object(&object_id, "function() { this.click(); return true; }", &[])
            .await;
        self.release_object(&object_id).await;
        result.map(|_| ())
    }

    async fn set_input(
        &self,
        element: &BrowserFlowElement,
        value: &str,
        dispatch_events: &[String],
    ) -> Result<(), BlobError> {
        let object_id = self.wait_for_object_id(element).await?;
        let result = self
            .call_function_on_object(
                &object_id,
                "function(value, eventNames) { this.value = value; for (const eventName of eventNames) { this.dispatchEvent(new Event(eventName, { bubbles: true })); } return true; }",
                &[
                    Value::String(value.to_string()),
                    Value::Array(
                        dispatch_events
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect(),
                    ),
                ],
            )
            .await;
        self.release_object(&object_id).await;
        result.map(|_| ())
    }

    async fn invoke_operation(&self, operation: &BrowserFlowOperation) -> Result<(), BlobError> {
        self.evaluate_value_in_frame(&operation.source, operation.frame.as_deref())
            .await?;
        Ok(())
    }

    async fn set_files(
        &self,
        element: &BrowserFlowElement,
        paths: &[String],
    ) -> Result<(), BlobError> {
        let object_id = self.wait_for_object_id(element).await?;
        let result = self
            .connection
            .send_command(
                "DOM.setFileInputFiles",
                json!({
                    "objectId": object_id,
                    "files": paths,
                }),
            )
            .await;
        self.release_object(&object_id).await;
        result.map(|_| ())
    }

    async fn dispatch_events(
        &self,
        element: &BrowserFlowElement,
        events: &[String],
    ) -> Result<(), BlobError> {
        let object_id = self.wait_for_object_id(element).await?;
        let result = self
            .call_function_on_object(
                &object_id,
                "function(eventNames) { for (const eventName of eventNames) { this.dispatchEvent(new Event(eventName, { bubbles: true })); } return true; }",
                &[Value::Array(
                    events
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                )],
            )
            .await;
        self.release_object(&object_id).await;
        result.map(|_| ())
    }

    async fn validate_visual_layout(
        &self,
        _request: &BrowserFlowVisualLayoutValidationRequest,
    ) -> Result<(), BlobError> {
        Err(BlobError::NotImplemented(
            "visual layout validation requires gateway LLM assistance".to_string(),
        ))
    }

    async fn solve_visual_captcha(
        &self,
        request: &BrowserFlowVisualCaptchaRequest,
    ) -> Result<(), BlobError> {
        if let Some(value) = request.manual_value.as_deref() {
            return self
                .set_input(&request.input_element, value, &request.dispatch_events)
                .await;
        }
        if !self.element_exists(&request.image_element).await? {
            return Err(BlobError::NotFound(format!(
                "visual captcha image element not found: {}",
                request.image_element.id
            )));
        }
        Err(BlobError::NotImplemented(
            "visual captcha solving requires gateway LLM assistance or manual input".to_string(),
        ))
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
    let request_id = params
        .get("requestId")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let method = request.get("method")?.as_str()?.to_string();
    let url = request.get("url")?.as_str()?.to_string();
    let headers = parse_header_map(request.get("headers"));
    let post_data = request
        .get("postData")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Some(CdpObservedRequest {
        sequence: 0,
        request_id,
        method,
        url,
        headers,
        post_data,
        response_status: None,
        response_status_text: None,
        loading_failed_text: None,
    })
}

fn parse_network_request_extra_headers(
    params: &Value,
) -> Option<(String, BTreeMap<String, String>)> {
    let request_id = params.get("requestId")?.as_str()?.trim().to_string();
    if request_id.is_empty() {
        return None;
    }
    Some((request_id, parse_header_map(params.get("headers"))))
}

fn parse_network_response_status(params: &Value) -> Option<(String, u16, Option<String>)> {
    let request_id = params.get("requestId")?.as_str()?.trim().to_string();
    if request_id.is_empty() {
        return None;
    }
    let response = params.get("response")?;
    let status = response.get("status").and_then(|value| {
        value
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .or_else(|| {
                value
                    .as_f64()
                    .filter(|status| status.is_finite() && *status >= 0.0)
                    .and_then(|status| u16::try_from(status as u64).ok())
            })
    })?;
    let status_text = response
        .get("statusText")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    Some((request_id, status, status_text))
}

fn parse_network_loading_failed(params: &Value) -> Option<(String, String)> {
    let request_id = params.get("requestId")?.as_str()?.trim().to_string();
    if request_id.is_empty() {
        return None;
    }
    let error_text = params
        .get("errorText")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("request failed");
    Some((request_id, error_text.to_string()))
}

fn parse_header_map(raw: Option<&Value>) -> BTreeMap<String, String> {
    let Some(Value::Object(headers)) = raw else {
        return BTreeMap::new();
    };

    headers
        .iter()
        .filter_map(|(name, value)| {
            header_value_to_string(value).map(|value| (normalize_header_name(name), value))
        })
        .collect()
}

fn header_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn normalize_header_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn request_query_field_value(url: &str, field_name: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(name, value)| {
            if name == field_name {
                Some(value.into_owned())
            } else {
                None
            }
        })
}

fn request_post_field_value(post_data: &str, field_name: &str) -> Option<String> {
    if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(post_data) {
        if let Some(value) = json_field_value(&Value::Object(object), field_name) {
            return json_value_to_scalar_string(value);
        }
    }

    url::form_urlencoded::parse(post_data.as_bytes()).find_map(|(name, value)| {
        if name == field_name {
            Some(value.into_owned())
        } else {
            None
        }
    })
}

fn json_field_value<'a>(value: &'a Value, field_path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in field_path.split('.') {
        let key = segment.trim();
        if key.is_empty() {
            return None;
        }
        current = current.get(key)?;
    }
    Some(current)
}

fn json_value_to_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

impl CdpObservedRequest {
    fn matches(&self, request: &BrowserFlowRequest) -> bool {
        self.method.eq_ignore_ascii_case(&request.method)
            && wildcard_match(&request.url_pattern, &self.url)
    }

    fn succeeded(&self, request: &BrowserFlowRequest) -> bool {
        self.response_status
            .is_some_and(|status| request.success_codes.contains(&status))
    }

    fn failed(&self, request: &BrowserFlowRequest) -> Option<String> {
        if !self.matches(request) {
            return None;
        }
        if let Some(error_text) = self.loading_failed_text.as_deref() {
            return Some(format!(
                "CDP request {} {} failed before receiving a usable response: {}",
                self.method, self.url, error_text
            ));
        }
        let status = self.response_status?;
        if request.success_codes.contains(&status) {
            return None;
        }
        let status_suffix = self
            .response_status_text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!(" {value}"))
            .unwrap_or_default();
        Some(format!(
            "CDP request {} {} observed non-success response {}{}",
            self.method, self.url, status, status_suffix
        ))
    }

    fn field_value(&self, field_name: &str) -> Option<String> {
        request_query_field_value(&self.url, field_name).or_else(|| {
            self.post_data
                .as_deref()
                .and_then(|post_data| request_post_field_value(post_data, field_name))
        })
    }
}

impl CdpEventState {
    fn latest_request_sequence(&self) -> u64 {
        self.next_request_sequence
    }

    fn observed_request_state_since(
        &self,
        request: &BrowserFlowRequest,
        min_sequence_exclusive: u64,
    ) -> CdpObservedRequestState {
        let Some(item) = self
            .requests
            .iter()
            .rev()
            .find(|item| item.sequence > min_sequence_exclusive && item.matches(request))
        else {
            return CdpObservedRequestState::NotSeen;
        };
        if item.succeeded(request) {
            return CdpObservedRequestState::Succeeded(item.sequence);
        }
        if let Some(message) = item.failed(request) {
            return CdpObservedRequestState::Failed(message);
        }
        CdpObservedRequestState::Pending
    }

    fn request_for_output<'a>(
        &'a self,
        request: &BrowserFlowRequest,
    ) -> Option<&'a CdpObservedRequest> {
        self.request_selections
            .get(&request.id)
            .and_then(|sequence| {
                self.requests.iter().find(|item| {
                    item.sequence == *sequence && item.matches(request) && item.succeeded(request)
                })
            })
            .or_else(|| {
                self.requests
                    .iter()
                    .rev()
                    .find(|item| item.matches(request) && item.succeeded(request))
            })
    }
}

fn apply_event_state(state: &mut CdpEventState, method: &str, params: &Value) -> bool {
    match method {
        "Network.requestWillBeSent" => {
            if let Some(mut request) = parse_network_request(params) {
                state.next_request_sequence = state.next_request_sequence.saturating_add(1);
                request.sequence = state.next_request_sequence;
                if let Some(request_id) = request.request_id.as_deref() {
                    if let Some(extra_headers) =
                        state.pending_request_extra_headers.remove(request_id)
                    {
                        request.headers.extend(extra_headers);
                    }
                }
                state.requests.push(request);
                if state.requests.len() > 128 {
                    let drain = state.requests.len().saturating_sub(128);
                    state.requests.drain(0..drain);
                    state.request_selections.retain(|_, sequence| {
                        state.requests.iter().any(|item| item.sequence == *sequence)
                    });
                }
                true
            } else {
                false
            }
        }
        "Network.requestWillBeSentExtraInfo" => {
            if let Some((request_id, headers)) = parse_network_request_extra_headers(params) {
                if let Some(request) = state
                    .requests
                    .iter_mut()
                    .rev()
                    .find(|item| item.request_id.as_deref() == Some(request_id.as_str()))
                {
                    request.headers.extend(headers);
                } else {
                    state
                        .pending_request_extra_headers
                        .insert(request_id, headers);
                    if state.pending_request_extra_headers.len() > 128 {
                        if let Some(oldest) =
                            state.pending_request_extra_headers.keys().next().cloned()
                        {
                            state.pending_request_extra_headers.remove(&oldest);
                        }
                    }
                }
                true
            } else {
                false
            }
        }
        "Network.responseReceived" => {
            if let Some((request_id, status, status_text)) = parse_network_response_status(params) {
                if let Some(request) = state
                    .requests
                    .iter_mut()
                    .rev()
                    .find(|item| item.request_id.as_deref() == Some(request_id.as_str()))
                {
                    request.response_status = Some(status);
                    request.response_status_text = status_text;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
        "Network.loadingFailed" => {
            if let Some((request_id, error_text)) = parse_network_loading_failed(params) {
                if let Some(request) = state
                    .requests
                    .iter_mut()
                    .rev()
                    .find(|item| item.request_id.as_deref() == Some(request_id.as_str()))
                {
                    request.loading_failed_text = Some(error_text);
                    true
                } else {
                    false
                }
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

fn choose_frame<'a>(
    frame_tree: &'a CdpFrameTreeNode,
    selector: &CdpFrameSelector,
) -> Option<&'a CdpFrameDescriptor> {
    if frame_matches_selector(&frame_tree.frame, selector) {
        return Some(&frame_tree.frame);
    }
    for child in &frame_tree.child_frames {
        if let Some(frame) = choose_frame(child, selector) {
            return Some(frame);
        }
    }
    None
}

fn frame_matches_selector(frame: &CdpFrameDescriptor, selector: &CdpFrameSelector) -> bool {
    match selector {
        CdpFrameSelector::FrameId(frame_id) => frame.frame_id == *frame_id,
        CdpFrameSelector::Name(name) => frame.name == *name,
        CdpFrameSelector::UrlPattern(pattern) => wildcard_match(pattern, &frame.url),
    }
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

fn runtime_evaluate_params(
    expression: &str,
    return_by_value: bool,
    context_id: Option<i64>,
) -> Value {
    let mut payload = json!({
        "expression": expression,
        "returnByValue": return_by_value,
        "awaitPromise": true,
    });
    if let Some(context_id) = context_id {
        payload["contextId"] = Value::Number(context_id.into());
    }
    payload
}

fn javascript_element_resolver_expression(
    element: &BrowserFlowElement,
) -> Result<String, BlobError> {
    if element.selectors.is_empty() {
        return Err(BlobError::Configuration(format!(
            "browser flow element {} has no selectors",
            element.id
        )));
    }

    let attempts = element
        .selectors
        .iter()
        .map(javascript_selector_attempt_expression)
        .collect::<Result<Vec<_>, _>>()?
        .join(",\n");

    Ok(format!(
        r#"(() => {{
  const __ccbgAsElements = (value) => {{
    if (!value) return [];
    if (value instanceof Element) return [value];
    if (Array.isArray(value)) return value.filter((item) => item instanceof Element);
    if (typeof NodeList !== 'undefined' && value instanceof NodeList) {{
      return Array.from(value).filter((item) => item instanceof Element);
    }}
    if (typeof HTMLCollection !== 'undefined' && value instanceof HTMLCollection) {{
      return Array.from(value).filter((item) => item instanceof Element);
    }}
    if (typeof value.length === 'number' && typeof value !== 'string') {{
      try {{
        return Array.from(value).filter((item) => item instanceof Element);
      }} catch (_) {{
        return [];
      }}
    }}
    return [];
  }};
  const __ccbgIsVisible = (element) => {{
    if (!(element instanceof Element)) return false;
    const style = window.getComputedStyle(element);
    if (!style || style.display === 'none' || style.visibility === 'hidden') return false;
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }};
  const __ccbgMatchesText = (element, expectedText) => {{
    if (!expectedText) return true;
    const text = String(element.innerText || element.textContent || '').trim();
    return text.includes(expectedText);
  }};
  const __ccbgSelectors = [
{attempts}
  ];
  for (const selector of __ccbgSelectors) {{
    let candidates = [];
    try {{
      candidates = __ccbgAsElements(selector.candidates());
    }} catch (_) {{
      candidates = [];
    }}
    for (const candidate of candidates) {{
      if (selector.requireVisible && !__ccbgIsVisible(candidate)) continue;
      if (!__ccbgMatchesText(candidate, selector.textContains)) continue;
      return candidate;
    }}
  }}
  return null;
}})()"#
    ))
}

fn javascript_element_snapshot_expression(
    element: &BrowserFlowElement,
) -> Result<String, BlobError> {
    let resolver = javascript_element_resolver_expression(element)?;
    Ok(format!(
        r#"(() => {{
  const element = {resolver};
  if (!(element instanceof Element)) {{
    return null;
  }}
  const style = window.getComputedStyle(element);
  const rect = element.getBoundingClientRect();
  const normalizeText = (value) => {{
    const text = String(value || '').replace(/\s+/g, ' ').trim();
    return text ? text : null;
  }};
  const contextRoot =
    element.closest('.el-form-item, .el-checkbox, .code-module, label, form, .login-main') ||
    element.parentElement ||
    element;
  const contextText = normalizeText(contextRoot?.innerText || contextRoot?.textContent || '');
  return {{
    tag_name: element.tagName || null,
    text: normalizeText(element.innerText || element.textContent || ''),
    context_text: contextText,
    placeholder: element.getAttribute('placeholder') || null,
    class_name: element.getAttribute('class') || null,
    input_type: element.getAttribute('type') || null,
    src: element.getAttribute('src') || null,
    visible: !!style && style.display !== 'none' && style.visibility !== 'hidden' && rect.width > 0 && rect.height > 0,
    rect: {{
      x: rect.x,
      y: rect.y,
      width: rect.width,
      height: rect.height
    }}
  }};
}})()"#
    ))
}

fn javascript_selector_attempt_expression(
    selector: &BrowserFlowSelector,
) -> Result<String, BlobError> {
    let candidate_expression = match selector.engine {
        BrowserFlowSelectorEngine::Css => {
            format!(
                "Array.from(document.querySelectorAll({:?}))",
                selector.value
            )
        }
        BrowserFlowSelectorEngine::Javascript => {
            format!("(() => {{ return {}; }})()", selector.value)
        }
        BrowserFlowSelectorEngine::Xpath => format!(
            "(() => {{ const result = document.evaluate({:?}, document, null, XPathResult.ORDERED_NODE_SNAPSHOT_TYPE, null); const nodes = []; for (let index = 0; index < result.snapshotLength; index += 1) {{ const node = result.snapshotItem(index); if (node instanceof Element) nodes.push(node); }} return nodes; }})()",
            selector.value
        ),
    };
    let text_contains = serde_json::to_string(&selector.text_contains).map_err(|error| {
        BlobError::Configuration(format!(
            "failed to encode selector text constraint: {error}"
        ))
    })?;
    Ok(format!(
        "    {{ candidates: () => ({}), requireVisible: {}, textContains: {} }}",
        candidate_expression,
        if selector.visible { "true" } else { "false" },
        text_contains
    ))
}

fn extract_runtime_value(payload: &Value, operation: &str) -> Result<Value, BlobError> {
    if let Some(message) = runtime_exception_message(payload, operation) {
        return Err(BlobError::Upstream(message));
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

#[derive(Debug, Clone, Copy)]
struct ViewportClip {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn viewport_clip_from_value(value: &Value, element_id: &str) -> Result<ViewportClip, BlobError> {
    let x = value
        .get("x")
        .and_then(Value::as_f64)
        .ok_or_else(|| BlobError::Upstream(format!("missing captcha clip x for {element_id}")))?;
    let y = value
        .get("y")
        .and_then(Value::as_f64)
        .ok_or_else(|| BlobError::Upstream(format!("missing captcha clip y for {element_id}")))?;
    let width = value.get("width").and_then(Value::as_f64).ok_or_else(|| {
        BlobError::Upstream(format!("missing captcha clip width for {element_id}"))
    })?;
    let height = value.get("height").and_then(Value::as_f64).ok_or_else(|| {
        BlobError::Upstream(format!("missing captcha clip height for {element_id}"))
    })?;

    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return Err(BlobError::NotFound(format!(
            "captcha image element {element_id} is not visible"
        )));
    }

    Ok(ViewportClip {
        x: x.max(0.0),
        y: y.max(0.0),
        width,
        height,
    })
}

fn extract_runtime_object_id(
    payload: &Value,
    operation: &str,
) -> Result<Option<String>, BlobError> {
    if let Some(message) = runtime_exception_message(payload, operation) {
        return Err(BlobError::Upstream(message));
    }

    let result = payload
        .get("result")
        .ok_or_else(|| BlobError::Upstream(format!("missing {operation} result")))?;
    if result
        .get("subtype")
        .and_then(Value::as_str)
        .is_some_and(|subtype| subtype == "null")
    {
        return Ok(None);
    }

    Ok(result
        .get("objectId")
        .and_then(Value::as_str)
        .map(ToString::to_string))
}

fn runtime_exception_message(payload: &Value, operation: &str) -> Option<String> {
    let details = payload.get("exceptionDetails")?;
    if details.is_null() {
        return None;
    }

    let description = details
        .get("exception")
        .and_then(|value| value.get("description"))
        .and_then(Value::as_str)
        .or_else(|| {
            payload
                .get("result")
                .and_then(|value| value.get("description"))
                .and_then(Value::as_str)
        });
    let text = details
        .get("text")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());

    Some(match (description, text) {
        (Some(description), _) => format!("CDP {operation} exception: {description}"),
        (None, Some(text)) => format!("CDP {operation} exception: {text}"),
        (None, None) => format!("CDP {operation} returned exceptionDetails"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        CdpEventState, CdpFrameDescriptor, CdpFrameSelector, CdpFrameTreeNode, CdpObservedRequest,
        CdpObservedRequestState, CdpTargetDescriptor, CdpTargetSelector, apply_event_state,
        choose_frame, choose_target, extract_runtime_value, javascript_element_resolver_expression,
        parse_network_request, request_post_field_value, validate_page_websocket_url,
        wildcard_match,
    };
    use blob_core::{
        BlobError, BrowserFlowElement, BrowserFlowRequest, BrowserFlowSelector,
        BrowserFlowSelectorEngine,
    };
    use serde_json::json;

    fn test_request_definition() -> BrowserFlowRequest {
        BrowserFlowRequest {
            id: "mobile_personal_list_files".to_string(),
            method: "POST".to_string(),
            url_pattern: "https://personal-kd-njs.yun.139.com/hcy/file/list*".to_string(),
            required_headers: Vec::new(),
            required_fields: vec!["parentFileId".to_string()],
            success_codes: vec![200],
            notes: Vec::new(),
        }
    }

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
        assert_eq!(
            extract_runtime_value(&payload, "Runtime.evaluate").unwrap(),
            json!("ok")
        );
    }

    #[test]
    fn extract_runtime_value_reports_exception_description() {
        let payload = json!({
            "result": {
                "type": "object",
                "subtype": "error",
                "description": "Error: element not found"
            },
            "exceptionDetails": {
                "text": "Uncaught",
                "exception": {
                    "description": "Error: element not found"
                }
            }
        });
        let error = extract_runtime_value(&payload, "Runtime.evaluate")
            .expect_err("exception payload should fail");
        assert!(matches!(error, BlobError::Upstream(_)));
        assert_eq!(
            error.to_string(),
            "upstream error: CDP Runtime.evaluate exception: Error: element not found"
        );
    }

    #[test]
    fn resolver_expression_honors_visibility_and_text_constraints() {
        let expression = javascript_element_resolver_expression(&BrowserFlowElement {
            id: "login.send_code_button".to_string(),
            page: "login".to_string(),
            role: "button".to_string(),
            required: true,
            frame: None,
            selectors: vec![
                BrowserFlowSelector {
                    engine: BrowserFlowSelectorEngine::Css,
                    value: ".change-code".to_string(),
                    text_contains: Some("发送验证码".to_string()),
                    visible: true,
                },
                BrowserFlowSelector {
                    engine: BrowserFlowSelectorEngine::Javascript,
                    value: "document.querySelector('.fallback')".to_string(),
                    text_contains: None,
                    visible: false,
                },
            ],
            notes: Vec::new(),
        })
        .expect("resolver expression should build");

        assert!(expression.contains(".change-code"));
        assert!(expression.contains("发送验证码"));
        assert!(expression.contains("requireVisible: true"));
        assert!(expression.contains("document.querySelector('.fallback')"));
    }

    #[test]
    fn frame_selector_parses_supported_forms() {
        assert_eq!(
            CdpFrameSelector::parse("name:udb_login").unwrap(),
            CdpFrameSelector::Name("udb_login".to_string())
        );
        assert_eq!(
            CdpFrameSelector::parse("url:https://open.e.189.cn/*").unwrap(),
            CdpFrameSelector::UrlPattern("https://open.e.189.cn/*".to_string())
        );
        assert_eq!(
            CdpFrameSelector::parse("id:frame-123").unwrap(),
            CdpFrameSelector::FrameId("frame-123".to_string())
        );
        assert_eq!(
            CdpFrameSelector::parse("udb_login").unwrap(),
            CdpFrameSelector::Name("udb_login".to_string())
        );
    }

    #[test]
    fn choose_frame_finds_nested_child_by_name() {
        let frame_tree = CdpFrameTreeNode {
            frame: CdpFrameDescriptor {
                frame_id: "root".to_string(),
                name: String::new(),
                url: "https://cloud.189.cn/web/login.html".to_string(),
            },
            child_frames: vec![CdpFrameTreeNode {
                frame: CdpFrameDescriptor {
                    frame_id: "child-1".to_string(),
                    name: "udb_login".to_string(),
                    url: "https://open.e.189.cn/api/logbox/separate/web/index.html".to_string(),
                },
                child_frames: Vec::new(),
            }],
        };

        let selected = choose_frame(
            &frame_tree,
            &CdpFrameSelector::Name("udb_login".to_string()),
        )
        .expect("nested frame should resolve");
        assert_eq!(selected.frame_id, "child-1");
    }

    #[test]
    fn parse_network_request_extracts_method_and_url() {
        let params = json!({
            "requestId": "request-1",
            "request": {
                "method": "POST",
                "url": "https://panservice.mail.wo.cn/wohome/dispatcher",
                "headers": {
                    "AccessToken": "token-123",
                    "Cookie": "sid=abc"
                }
            }
        });
        assert_eq!(
            parse_network_request(&params),
            Some(CdpObservedRequest {
                sequence: 0,
                request_id: Some("request-1".to_string()),
                method: "POST".to_string(),
                url: "https://panservice.mail.wo.cn/wohome/dispatcher".to_string(),
                headers: BTreeMap::from([
                    ("accesstoken".to_string(), "token-123".to_string()),
                    ("cookie".to_string(), "sid=abc".to_string()),
                ]),
                post_data: None,
                response_status: None,
                response_status_text: None,
                loading_failed_text: None,
            })
        );
    }

    #[test]
    fn request_post_field_value_reads_json_body_fields() {
        assert_eq!(
            request_post_field_value(
                r#"{"parentFileId":"Fu1WyOSxcEdoFkLIAdNVNx6BD4vs53Dba","pageInfo":{"pageSize":60}}"#,
                "parentFileId",
            ),
            Some("Fu1WyOSxcEdoFkLIAdNVNx6BD4vs53Dba".to_string())
        );
    }

    #[test]
    fn request_post_field_value_reads_nested_json_body_fields() {
        assert_eq!(
            request_post_field_value(
                r#"{"commonAccountInfo":{"userDomainId":"1283769981849164286","accountType":1}}"#,
                "commonAccountInfo.userDomainId",
            ),
            Some("1283769981849164286".to_string())
        );
    }

    #[test]
    fn request_post_field_value_reads_form_body_fields() {
        assert_eq!(
            request_post_field_value("userDomainId=1283769981849164286&foo=bar", "userDomainId"),
            Some("1283769981849164286".to_string())
        );
    }

    #[test]
    fn apply_event_state_tracks_request_headers_and_page_url() {
        let mut state = CdpEventState::default();

        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSentExtraInfo",
            &json!({
                "requestId": "request-1",
                "headers": {
                    "Cookie": "sid=abc; path=/",
                    "X-Trace": 42
                }
            }),
        ));
        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSent",
            &json!({
                "requestId": "request-1",
                "request": {
                    "method": "POST",
                    "url": "https://panservice.mail.wo.cn/wohome/dispatcher",
                    "headers": {
                        "AccessToken": "token-456"
                    }
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
            state.requests[0].headers.get("cookie").map(String::as_str),
            Some("sid=abc; path=/")
        );
        assert_eq!(
            state.requests[0].headers.get("x-trace").map(String::as_str),
            Some("42")
        );
        assert_eq!(
            state.current_url.as_deref(),
            Some("https://pan.wo.cn/pan/file_list/all")
        );
    }

    #[test]
    fn request_state_requires_success_response_code() {
        let mut state = CdpEventState::default();
        let request = test_request_definition();

        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSent",
            &json!({
                "requestId": "request-404",
                "request": {
                    "method": "POST",
                    "url": "https://personal-kd-njs.yun.139.com/hcy/file/list",
                    "postData": "{\"parentFileId\":\"/\"}"
                }
            }),
        ));
        assert!(matches!(
            state.observed_request_state_since(&request, 0),
            CdpObservedRequestState::Pending
        ));

        assert!(apply_event_state(
            &mut state,
            "Network.responseReceived",
            &json!({
                "requestId": "request-404",
                "response": {
                    "status": 404,
                    "statusText": "Not Found"
                }
            }),
        ));
        assert_eq!(
            state.observed_request_state_since(&request, 0),
            CdpObservedRequestState::Failed(
                "CDP request POST https://personal-kd-njs.yun.139.com/hcy/file/list observed non-success response 404 Not Found".to_string()
            )
        );
        assert!(state.request_for_output(&request).is_none());
    }

    #[test]
    fn request_state_selects_successful_request_for_output() {
        let mut state = CdpEventState::default();
        let request = test_request_definition();

        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSent",
            &json!({
                "requestId": "request-1",
                "request": {
                    "method": "POST",
                    "url": "https://personal-kd-njs.yun.139.com/hcy/file/list",
                    "headers": {
                        "Authorization": "Basic token-1"
                    },
                    "postData": "{\"parentFileId\":\"/\"}"
                }
            }),
        ));
        assert!(apply_event_state(
            &mut state,
            "Network.responseReceived",
            &json!({
                "requestId": "request-1",
                "response": {
                    "status": 200,
                    "statusText": "OK"
                }
            }),
        ));

        let sequence = match state.observed_request_state_since(&request, 0) {
            CdpObservedRequestState::Succeeded(sequence) => sequence,
            other => panic!("expected successful request, got {other:?}"),
        };
        state
            .request_selections
            .insert(request.id.clone(), sequence);

        let selected = state
            .request_for_output(&request)
            .expect("successful request should be selected");
        assert_eq!(
            selected.headers.get("authorization").map(String::as_str),
            Some("Basic token-1")
        );
        assert_eq!(selected.field_value("parentFileId").as_deref(), Some("/"));
    }

    #[test]
    fn request_state_reports_network_loading_failure() {
        let mut state = CdpEventState::default();
        let request = test_request_definition();

        assert!(apply_event_state(
            &mut state,
            "Network.requestWillBeSent",
            &json!({
                "requestId": "request-neterr",
                "request": {
                    "method": "POST",
                    "url": "https://personal-kd-njs.yun.139.com/hcy/file/list",
                    "postData": "{\"parentFileId\":\"/\"}"
                }
            }),
        ));
        assert!(apply_event_state(
            &mut state,
            "Network.loadingFailed",
            &json!({
                "requestId": "request-neterr",
                "errorText": "net::ERR_ABORTED"
            }),
        ));

        assert_eq!(
            state.observed_request_state_since(&request, 0),
            CdpObservedRequestState::Failed(
                "CDP request POST https://personal-kd-njs.yun.139.com/hcy/file/list failed before receiving a usable response: net::ERR_ABORTED".to_string()
            )
        );
    }
}
