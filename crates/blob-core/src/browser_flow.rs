use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::BlobError;

pub const BROWSER_FLOW_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserFlowCatalog {
    pub schema_version: u32,
    pub provider: String,
    pub surface: String,
    #[serde(default)]
    pub description: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub presets: BTreeMap<String, Value>,
    #[serde(default)]
    pub pages: Vec<BrowserFlowPage>,
    #[serde(default)]
    pub elements: Vec<BrowserFlowElement>,
    #[serde(default)]
    pub requests: Vec<BrowserFlowRequest>,
    #[serde(default)]
    pub operations: Vec<BrowserFlowOperation>,
    #[serde(default)]
    pub flows: Vec<BrowserFlow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrowserFlowCatalogDirectoryEntry {
    pub source_path: PathBuf,
    pub catalog: BrowserFlowCatalog,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrowserFlowCatalogCollection {
    entries: Vec<BrowserFlowCatalogDirectoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowPage {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub url_patterns: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowElement {
    pub id: String,
    pub page: String,
    pub role: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub selectors: Vec<BrowserFlowSelector>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowSelector {
    pub engine: BrowserFlowSelectorEngine,
    pub value: String,
    #[serde(default)]
    pub text_contains: Option<String>,
    #[serde(default)]
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowSelectorEngine {
    Css,
    Xpath,
    Javascript,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowRequest {
    pub id: String,
    pub method: String,
    pub url_pattern: String,
    #[serde(default)]
    pub required_headers: Vec<BrowserFlowHeaderMatcher>,
    #[serde(default)]
    pub required_fields: Vec<String>,
    #[serde(default)]
    pub success_codes: Vec<u16>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowHeaderMatcher {
    pub name: String,
    #[serde(default)]
    pub value_template: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowOperation {
    pub id: String,
    #[serde(default)]
    pub page: Option<String>,
    pub kind: BrowserFlowOperationKind,
    pub source: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowOperationKind {
    Javascript,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlow {
    pub id: String,
    pub title: String,
    pub purpose: String,
    pub start_page: String,
    #[serde(default)]
    pub preset_refs: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<BrowserFlowInput>,
    #[serde(default)]
    pub steps: Vec<BrowserFlowStep>,
    #[serde(default)]
    pub expected_requests: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<BrowserFlowOutput>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BrowserFlowBindingContext {
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub runtime: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundBrowserFlowPlan {
    pub provider: String,
    pub surface: String,
    pub flow: BrowserFlow,
    pub context: BrowserFlowBindingContext,
    #[serde(default)]
    pub presets: BTreeMap<String, Value>,
    #[serde(default)]
    pub pages: Vec<BrowserFlowPage>,
    #[serde(default)]
    pub elements: Vec<BrowserFlowElement>,
    #[serde(default)]
    pub requests: Vec<BrowserFlowRequest>,
    #[serde(default)]
    pub operations: Vec<BrowserFlowOperation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowExecutionMode {
    DryRun,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowExecutionStepStatus {
    Planned,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserFlowExecutionStepReport {
    pub step_id: String,
    pub step_kind: String,
    pub status: BrowserFlowExecutionStepStatus,
    #[serde(default)]
    pub detail: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserFlowExecutionReport {
    pub mode: BrowserFlowExecutionMode,
    pub provider: String,
    pub surface: String,
    pub flow_id: String,
    pub step_count: usize,
    #[serde(default)]
    pub expected_requests: Vec<String>,
    #[serde(default)]
    pub steps: Vec<BrowserFlowExecutionStepReport>,
}

#[derive(Debug, Clone, Default)]
pub struct DryRunBrowserFlowExecutor;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowInput {
    pub id: String,
    pub label: String,
    pub kind: BrowserFlowInputKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowInputKind {
    Text,
    Secret,
    File,
    RuntimeValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowOutput {
    pub id: String,
    pub kind: BrowserFlowOutputKind,
    pub source: String,
    #[serde(default)]
    pub redact: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowOutputKind {
    RequestHeader,
    RequestField,
    ResponseField,
    ScriptValue,
    Url,
    DomText,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserFlowStep {
    Navigate {
        id: String,
        url: String,
        #[serde(default)]
        wait_for_page: Option<String>,
    },
    Click {
        id: String,
        element: String,
        #[serde(default)]
        optional: bool,
    },
    SetInput {
        id: String,
        element: String,
        value_template: String,
        #[serde(default)]
        dispatch_events: Vec<String>,
    },
    InvokeOperation {
        id: String,
        operation: String,
    },
    SetFiles {
        id: String,
        element: String,
        input_ref: String,
    },
    DispatchEvents {
        id: String,
        element: String,
        events: Vec<String>,
    },
    WaitForRequest {
        id: String,
        request: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitForPage {
        id: String,
        page: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Wait {
        id: String,
        duration_ms: u64,
    },
}

impl BrowserFlowCatalog {
    pub fn from_json_str(raw: &str) -> Result<Self, BlobError> {
        let catalog = serde_json::from_str::<Self>(raw).map_err(|error| {
            BlobError::Configuration(format!("invalid browser flow catalog JSON: {error}"))
        })?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn from_json_slice(raw: &[u8]) -> Result<Self, BlobError> {
        let catalog = serde_json::from_slice::<Self>(raw).map_err(|error| {
            BlobError::Configuration(format!("invalid browser flow catalog JSON: {error}"))
        })?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, BlobError> {
        let path = path.as_ref();
        let raw = fs::read(path).map_err(|error| {
            BlobError::Configuration(format!(
                "failed to read browser flow catalog {}: {error}",
                path.display()
            ))
        })?;
        Self::from_json_slice(&raw)
    }

    pub fn find_flow(&self, flow_id: &str) -> Option<&BrowserFlow> {
        self.flows.iter().find(|flow| flow.id == flow_id)
    }

    pub fn bind_flow(
        &self,
        flow_id: &str,
        context: &BrowserFlowBindingContext,
    ) -> Result<BoundBrowserFlowPlan, BlobError> {
        let flow = self
            .find_flow(flow_id)
            .ok_or_else(|| BlobError::NotFound(format!("browser flow not found: {flow_id}")))?;
        validate_required_input_values(flow, &context.inputs)?;

        let mut bound_flow = flow.clone();
        bind_flow_templates(&mut bound_flow, context)?;

        let mut referenced_page_ids = BTreeSet::from([bound_flow.start_page.clone()]);
        let mut referenced_element_ids = BTreeSet::new();
        let mut referenced_request_ids = BTreeSet::new();
        let mut referenced_operation_ids = BTreeSet::new();

        for step in &bound_flow.steps {
            match step {
                BrowserFlowStep::Navigate { wait_for_page, .. } => {
                    if let Some(page) = wait_for_page {
                        referenced_page_ids.insert(page.clone());
                    }
                }
                BrowserFlowStep::Click { element, .. }
                | BrowserFlowStep::SetInput { element, .. }
                | BrowserFlowStep::SetFiles { element, .. }
                | BrowserFlowStep::DispatchEvents { element, .. } => {
                    referenced_element_ids.insert(element.clone());
                }
                BrowserFlowStep::InvokeOperation { operation, .. } => {
                    referenced_operation_ids.insert(operation.clone());
                }
                BrowserFlowStep::WaitForRequest { request, .. } => {
                    referenced_request_ids.insert(request.clone());
                }
                BrowserFlowStep::WaitForPage { page, .. } => {
                    referenced_page_ids.insert(page.clone());
                }
                BrowserFlowStep::Wait { .. } => {}
            }
        }

        for request in &bound_flow.expected_requests {
            referenced_request_ids.insert(request.clone());
        }

        let mut presets = BTreeMap::new();
        for preset_ref in &bound_flow.preset_refs {
            let value = self.presets.get(preset_ref).ok_or_else(|| {
                BlobError::Configuration(format!(
                    "flow {} references unknown preset {}",
                    bound_flow.id, preset_ref
                ))
            })?;
            presets.insert(preset_ref.clone(), bind_json_value(value, context)?);
        }

        let mut elements = Vec::new();
        for element in &self.elements {
            if referenced_element_ids.contains(&element.id) {
                let bound = bind_element(element, context)?;
                referenced_page_ids.insert(bound.page.clone());
                elements.push(bound);
            }
        }

        let mut operations = Vec::new();
        for operation in &self.operations {
            if referenced_operation_ids.contains(&operation.id) {
                let bound = bind_operation(operation, context)?;
                if let Some(page) = &bound.page {
                    referenced_page_ids.insert(page.clone());
                }
                operations.push(bound);
            }
        }

        let mut requests = Vec::new();
        for request in &self.requests {
            if referenced_request_ids.contains(&request.id) {
                requests.push(bind_request(request, context)?);
            }
        }

        let mut pages = Vec::new();
        for page in &self.pages {
            if referenced_page_ids.contains(&page.id) {
                pages.push(bind_page(page, context)?);
            }
        }

        Ok(BoundBrowserFlowPlan {
            provider: self.provider.clone(),
            surface: self.surface.clone(),
            flow: bound_flow,
            context: context.clone(),
            presets,
            pages,
            elements,
            requests,
            operations,
        })
    }

    pub fn validate(&self) -> Result<(), BlobError> {
        if self.schema_version != BROWSER_FLOW_SCHEMA_VERSION {
            return Err(BlobError::Configuration(format!(
                "unsupported browser flow schema version {}; expected {}",
                self.schema_version, BROWSER_FLOW_SCHEMA_VERSION
            )));
        }

        ensure_non_empty("provider", self.provider.as_str())?;
        ensure_non_empty("surface", self.surface.as_str())?;
        ensure_non_empty("base_url", self.base_url.as_str())?;

        if self.pages.is_empty() {
            return Err(BlobError::Configuration(
                "browser flow catalog must contain at least one page".to_string(),
            ));
        }
        if self.elements.is_empty() {
            return Err(BlobError::Configuration(
                "browser flow catalog must contain at least one element".to_string(),
            ));
        }
        if self.flows.is_empty() {
            return Err(BlobError::Configuration(
                "browser flow catalog must contain at least one flow".to_string(),
            ));
        }

        let page_ids = collect_unique_ids("page", self.pages.iter().map(|page| page.id.as_str()))?;
        for page in &self.pages {
            ensure_non_empty("page.id", page.id.as_str())?;
            ensure_non_empty("page.title", page.title.as_str())?;
            if page.url_patterns.is_empty() {
                return Err(BlobError::Configuration(format!(
                    "page {} must contain at least one url pattern",
                    page.id
                )));
            }
            for pattern in &page.url_patterns {
                ensure_non_empty("page.url_patterns[]", pattern.as_str())?;
            }
        }

        for preset_id in self.presets.keys() {
            ensure_non_empty("preset id", preset_id.as_str())?;
        }

        let element_ids = collect_unique_ids(
            "element",
            self.elements.iter().map(|element| element.id.as_str()),
        )?;
        for element in &self.elements {
            ensure_non_empty("element.id", element.id.as_str())?;
            ensure_non_empty("element.page", element.page.as_str())?;
            ensure_non_empty("element.role", element.role.as_str())?;
            if !page_ids.contains(element.page.as_str()) {
                return Err(BlobError::Configuration(format!(
                    "element {} references unknown page {}",
                    element.id, element.page
                )));
            }
            if element.selectors.is_empty() {
                return Err(BlobError::Configuration(format!(
                    "element {} must contain at least one selector",
                    element.id
                )));
            }
            for selector in &element.selectors {
                ensure_non_empty("element.selector.value", selector.value.as_str())?;
                if let Some(text_contains) = selector.text_contains.as_deref() {
                    ensure_non_empty("element.selector.text_contains", text_contains)?;
                }
            }
        }

        let request_ids = collect_unique_ids(
            "request",
            self.requests.iter().map(|request| request.id.as_str()),
        )?;
        for request in &self.requests {
            ensure_non_empty("request.id", request.id.as_str())?;
            ensure_non_empty("request.method", request.method.as_str())?;
            ensure_non_empty("request.url_pattern", request.url_pattern.as_str())?;
            if request.success_codes.is_empty() {
                return Err(BlobError::Configuration(format!(
                    "request {} must contain at least one success code",
                    request.id
                )));
            }
            for header in &request.required_headers {
                ensure_non_empty("request.required_headers[].name", header.name.as_str())?;
                if let Some(value_template) = header.value_template.as_deref() {
                    ensure_non_empty("request.required_headers[].value_template", value_template)?;
                }
            }
            for field in &request.required_fields {
                ensure_non_empty("request.required_fields[]", field.as_str())?;
            }
        }

        let operation_ids = collect_unique_ids(
            "operation",
            self.operations
                .iter()
                .map(|operation| operation.id.as_str()),
        )?;
        for operation in &self.operations {
            ensure_non_empty("operation.id", operation.id.as_str())?;
            ensure_non_empty("operation.source", operation.source.as_str())?;
            if let Some(page) = operation.page.as_deref() {
                if !page_ids.contains(page) {
                    return Err(BlobError::Configuration(format!(
                        "operation {} references unknown page {}",
                        operation.id, page
                    )));
                }
            }
        }

        let flow_ids = collect_unique_ids("flow", self.flows.iter().map(|flow| flow.id.as_str()))?;
        let _ = flow_ids;
        for flow in &self.flows {
            ensure_non_empty("flow.id", flow.id.as_str())?;
            ensure_non_empty("flow.title", flow.title.as_str())?;
            ensure_non_empty("flow.purpose", flow.purpose.as_str())?;
            if !page_ids.contains(flow.start_page.as_str()) {
                return Err(BlobError::Configuration(format!(
                    "flow {} references unknown start page {}",
                    flow.id, flow.start_page
                )));
            }
            if flow.steps.is_empty() {
                return Err(BlobError::Configuration(format!(
                    "flow {} must contain at least one step",
                    flow.id
                )));
            }

            let input_ids = collect_unique_ids(
                "flow input",
                flow.inputs.iter().map(|input| input.id.as_str()),
            )?;
            for input in &flow.inputs {
                ensure_non_empty("flow.input.id", input.id.as_str())?;
                ensure_non_empty("flow.input.label", input.label.as_str())?;
                if let Some(description) = input.description.as_deref() {
                    ensure_non_empty("flow.input.description", description)?;
                }
            }

            for preset_ref in &flow.preset_refs {
                ensure_non_empty("flow.preset_refs[]", preset_ref.as_str())?;
                if !self.presets.contains_key(preset_ref) {
                    return Err(BlobError::Configuration(format!(
                        "flow {} references unknown preset {}",
                        flow.id, preset_ref
                    )));
                }
            }

            let _output_ids = collect_unique_ids(
                "flow output",
                flow.outputs.iter().map(|output| output.id.as_str()),
            )?;
            for output in &flow.outputs {
                ensure_non_empty("flow.output.id", output.id.as_str())?;
                ensure_non_empty("flow.output.source", output.source.as_str())?;
                if let Some(description) = output.description.as_deref() {
                    ensure_non_empty("flow.output.description", description)?;
                }
            }

            let step_ids =
                collect_unique_ids("flow step", flow.steps.iter().map(BrowserFlowStep::id))?;
            let _ = step_ids;
            for step in &flow.steps {
                match step {
                    BrowserFlowStep::Navigate {
                        url, wait_for_page, ..
                    } => {
                        ensure_non_empty("flow.step.url", url.as_str())?;
                        if let Some(page) = wait_for_page.as_deref() {
                            if !page_ids.contains(page) {
                                return Err(BlobError::Configuration(format!(
                                    "flow {} step {} references unknown page {}",
                                    flow.id,
                                    step.id(),
                                    page
                                )));
                            }
                        }
                    }
                    BrowserFlowStep::Click { element, .. }
                    | BrowserFlowStep::SetInput { element, .. }
                    | BrowserFlowStep::SetFiles { element, .. }
                    | BrowserFlowStep::DispatchEvents { element, .. } => {
                        if !element_ids.contains(element.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown element {}",
                                flow.id,
                                step.id(),
                                element
                            )));
                        }
                    }
                    BrowserFlowStep::InvokeOperation { operation, .. } => {
                        if !operation_ids.contains(operation.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown operation {}",
                                flow.id,
                                step.id(),
                                operation
                            )));
                        }
                    }
                    BrowserFlowStep::WaitForRequest { request, .. } => {
                        if !request_ids.contains(request.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown request {}",
                                flow.id,
                                step.id(),
                                request
                            )));
                        }
                    }
                    BrowserFlowStep::WaitForPage { page, .. } => {
                        if !page_ids.contains(page.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown page {}",
                                flow.id,
                                step.id(),
                                page
                            )));
                        }
                    }
                    BrowserFlowStep::Wait { duration_ms, .. } => {
                        if *duration_ms == 0 {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} must wait for at least 1 ms",
                                flow.id,
                                step.id()
                            )));
                        }
                    }
                }

                match step {
                    BrowserFlowStep::SetInput { value_template, .. } => {
                        ensure_non_empty("flow.step.value_template", value_template.as_str())?;
                    }
                    BrowserFlowStep::SetFiles { input_ref, .. } => {
                        ensure_non_empty("flow.step.input_ref", input_ref.as_str())?;
                        if !input_ids.contains(input_ref.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown input {}",
                                flow.id,
                                step.id(),
                                input_ref
                            )));
                        }
                    }
                    BrowserFlowStep::DispatchEvents { events, .. } => {
                        if events.is_empty() {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} must contain at least one event",
                                flow.id,
                                step.id()
                            )));
                        }
                        for event in events {
                            ensure_non_empty("flow.step.events[]", event.as_str())?;
                        }
                    }
                    _ => {}
                }
            }

            let mut seen_expected_requests = BTreeSet::new();
            for request in &flow.expected_requests {
                ensure_non_empty("flow.expected_requests[]", request.as_str())?;
                if !request_ids.contains(request.as_str()) {
                    return Err(BlobError::Configuration(format!(
                        "flow {} references unknown expected request {}",
                        flow.id, request
                    )));
                }
                if !seen_expected_requests.insert(request.as_str()) {
                    return Err(BlobError::Configuration(format!(
                        "flow {} references duplicate expected request {}",
                        flow.id, request
                    )));
                }
            }
        }

        Ok(())
    }
}

impl BrowserFlowCatalogCollection {
    pub fn from_json_dir(dir: impl AsRef<Path>) -> Result<Self, BlobError> {
        let dir = dir.as_ref();
        let mut json_paths = fs::read_dir(dir)
            .map_err(|error| {
                BlobError::Configuration(format!(
                    "failed to read browser flow catalog directory {}: {error}",
                    dir.display()
                ))
            })?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("json"))
            })
            .collect::<Vec<_>>();
        json_paths.sort();

        let mut entries = Vec::with_capacity(json_paths.len());
        let mut seen_catalog_keys = BTreeSet::new();
        for path in json_paths {
            let catalog = BrowserFlowCatalog::from_json_file(&path)?;
            let key = format!("{}/{}", catalog.provider, catalog.surface);
            if !seen_catalog_keys.insert(key.clone()) {
                return Err(BlobError::Configuration(format!(
                    "duplicate browser flow catalog detected for {key}"
                )));
            }
            entries.push(BrowserFlowCatalogDirectoryEntry {
                source_path: path,
                catalog,
            });
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[BrowserFlowCatalogDirectoryEntry] {
        &self.entries
    }

    pub fn get(&self, provider: &str, surface: &str) -> Option<&BrowserFlowCatalog> {
        self.entries
            .iter()
            .find(|entry| entry.catalog.provider == provider && entry.catalog.surface == surface)
            .map(|entry| &entry.catalog)
    }

    pub fn bind_flow(
        &self,
        provider: &str,
        surface: &str,
        flow_id: &str,
        context: &BrowserFlowBindingContext,
    ) -> Result<BoundBrowserFlowPlan, BlobError> {
        self.get(provider, surface)
            .ok_or_else(|| {
                BlobError::NotFound(format!(
                    "browser flow catalog not found for {provider}/{surface}"
                ))
            })?
            .bind_flow(flow_id, context)
    }
}

impl BoundBrowserFlowPlan {
    pub fn find_request(&self, request_id: &str) -> Option<&BrowserFlowRequest> {
        self.requests
            .iter()
            .find(|request| request.id == request_id)
    }

    pub fn find_operation(&self, operation_id: &str) -> Option<&BrowserFlowOperation> {
        self.operations
            .iter()
            .find(|operation| operation.id == operation_id)
    }

    pub fn input_value(&self, input_id: &str) -> Option<&Value> {
        self.context.inputs.get(input_id)
    }
}

#[async_trait]
pub trait BrowserFlowExecutor: Send + Sync {
    fn mode(&self) -> BrowserFlowExecutionMode;

    async fn execute(
        &self,
        plan: &BoundBrowserFlowPlan,
    ) -> Result<BrowserFlowExecutionReport, BlobError>;
}

#[async_trait]
impl BrowserFlowExecutor for DryRunBrowserFlowExecutor {
    fn mode(&self) -> BrowserFlowExecutionMode {
        BrowserFlowExecutionMode::DryRun
    }

    async fn execute(
        &self,
        plan: &BoundBrowserFlowPlan,
    ) -> Result<BrowserFlowExecutionReport, BlobError> {
        let steps = plan
            .flow
            .steps
            .iter()
            .map(|step| dry_run_step_report(plan, step))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(BrowserFlowExecutionReport {
            mode: self.mode(),
            provider: plan.provider.clone(),
            surface: plan.surface.clone(),
            flow_id: plan.flow.id.clone(),
            step_count: steps.len(),
            expected_requests: plan.flow.expected_requests.clone(),
            steps,
        })
    }
}

impl BrowserFlowStep {
    pub fn id(&self) -> &str {
        match self {
            Self::Navigate { id, .. }
            | Self::Click { id, .. }
            | Self::SetInput { id, .. }
            | Self::InvokeOperation { id, .. }
            | Self::SetFiles { id, .. }
            | Self::DispatchEvents { id, .. }
            | Self::WaitForRequest { id, .. }
            | Self::WaitForPage { id, .. }
            | Self::Wait { id, .. } => id.as_str(),
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Click { .. } => "click",
            Self::SetInput { .. } => "set_input",
            Self::InvokeOperation { .. } => "invoke_operation",
            Self::SetFiles { .. } => "set_files",
            Self::DispatchEvents { .. } => "dispatch_events",
            Self::WaitForRequest { .. } => "wait_for_request",
            Self::WaitForPage { .. } => "wait_for_page",
            Self::Wait { .. } => "wait",
        }
    }
}

fn ensure_non_empty(field: &str, value: &str) -> Result<(), BlobError> {
    if value.trim().is_empty() {
        return Err(BlobError::Configuration(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

fn collect_unique_ids<'a>(
    kind: &str,
    ids: impl IntoIterator<Item = &'a str>,
) -> Result<BTreeSet<String>, BlobError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        ensure_non_empty(kind, id)?;
        if !seen.insert(id.to_string()) {
            return Err(BlobError::Configuration(format!(
                "duplicate {kind} id detected: {id}"
            )));
        }
    }
    Ok(seen)
}

const fn default_true() -> bool {
    true
}

fn validate_required_input_values(
    flow: &BrowserFlow,
    inputs: &BTreeMap<String, Value>,
) -> Result<(), BlobError> {
    for input in &flow.inputs {
        if !input.required {
            continue;
        }
        let value = inputs.get(&input.id).ok_or_else(|| {
            BlobError::Configuration(format!(
                "missing required browser flow input {} for flow {}",
                input.id, flow.id
            ))
        })?;
        if !value_is_present(value) {
            return Err(BlobError::Configuration(format!(
                "required browser flow input {} for flow {} must not be empty",
                input.id, flow.id
            )));
        }
    }
    Ok(())
}

fn value_is_present(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        _ => true,
    }
}

fn bind_flow_templates(
    flow: &mut BrowserFlow,
    context: &BrowserFlowBindingContext,
) -> Result<(), BlobError> {
    for step in &mut flow.steps {
        match step {
            BrowserFlowStep::Navigate { url, .. } => {
                *url = bind_string_template(url, context)?;
            }
            BrowserFlowStep::SetInput { value_template, .. } => {
                *value_template = bind_string_template(value_template, context)?;
            }
            _ => {}
        }
    }

    for output in &mut flow.outputs {
        output.source = bind_string_template(&output.source, context)?;
    }

    Ok(())
}

fn bind_page(
    page: &BrowserFlowPage,
    context: &BrowserFlowBindingContext,
) -> Result<BrowserFlowPage, BlobError> {
    let mut bound = page.clone();
    for pattern in &mut bound.url_patterns {
        *pattern = bind_string_template(pattern, context)?;
    }
    Ok(bound)
}

fn bind_element(
    element: &BrowserFlowElement,
    context: &BrowserFlowBindingContext,
) -> Result<BrowserFlowElement, BlobError> {
    let mut bound = element.clone();
    for selector in &mut bound.selectors {
        selector.value = bind_string_template(&selector.value, context)?;
        if let Some(text_contains) = &mut selector.text_contains {
            *text_contains = bind_string_template(text_contains, context)?;
        }
    }
    Ok(bound)
}

fn bind_request(
    request: &BrowserFlowRequest,
    context: &BrowserFlowBindingContext,
) -> Result<BrowserFlowRequest, BlobError> {
    let mut bound = request.clone();
    bound.url_pattern = bind_string_template(&bound.url_pattern, context)?;
    for header in &mut bound.required_headers {
        if let Some(value_template) = &mut header.value_template {
            *value_template = bind_string_template(value_template, context)?;
        }
    }
    for field in &mut bound.required_fields {
        *field = bind_string_template(field, context)?;
    }
    Ok(bound)
}

fn bind_operation(
    operation: &BrowserFlowOperation,
    context: &BrowserFlowBindingContext,
) -> Result<BrowserFlowOperation, BlobError> {
    let mut bound = operation.clone();
    bound.source = bind_string_template(&bound.source, context)?;
    Ok(bound)
}

fn bind_json_value(value: &Value, context: &BrowserFlowBindingContext) -> Result<Value, BlobError> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(value.clone()),
        Value::String(raw) => bind_template_value(raw, context),
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| bind_json_value(item, context))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(map) => {
            let mut bound = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                bound.insert(key.clone(), bind_json_value(value, context)?);
            }
            Ok(Value::Object(bound))
        }
    }
}

fn bind_string_template(
    raw: &str,
    context: &BrowserFlowBindingContext,
) -> Result<String, BlobError> {
    match bind_template_value(raw, context)? {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Array(_) | Value::Object(_) => Err(BlobError::Configuration(format!(
            "template {raw:?} resolved to a non-scalar JSON value"
        ))),
    }
}

fn bind_template_value(raw: &str, context: &BrowserFlowBindingContext) -> Result<Value, BlobError> {
    #[derive(Debug)]
    enum Segment {
        Literal(String),
        Placeholder { token: String, value: Value },
    }

    let mut segments = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = raw[cursor..].find("{{") {
        let start = cursor + relative_start;
        let token_start = start + 2;
        let relative_end = raw[token_start..].find("}}").ok_or_else(|| {
            BlobError::Configuration(format!("unterminated browser flow template: {raw}"))
        })?;
        let token_end = token_start + relative_end;
        if start > cursor {
            segments.push(Segment::Literal(raw[cursor..start].to_string()));
        }
        let token = raw[token_start..token_end].trim().to_string();
        let value = lookup_template_value(&token, context)?;
        segments.push(Segment::Placeholder { token, value });
        cursor = token_end + 2;
    }

    if segments.is_empty() {
        return Ok(Value::String(raw.to_string()));
    }

    if cursor < raw.len() {
        segments.push(Segment::Literal(raw[cursor..].to_string()));
    }

    if segments.len() == 1 {
        if let Segment::Placeholder { value, .. } = &segments[0] {
            return Ok(value.clone());
        }
    }

    let mut rendered = String::new();
    for segment in segments {
        match segment {
            Segment::Literal(value) => rendered.push_str(&value),
            Segment::Placeholder { token, value } => {
                rendered.push_str(&template_scalar_to_string(&token, &value)?);
            }
        }
    }

    Ok(Value::String(rendered))
}

fn lookup_template_value(
    token: &str,
    context: &BrowserFlowBindingContext,
) -> Result<Value, BlobError> {
    let (namespace, key) = token.split_once('.').ok_or_else(|| {
        BlobError::Configuration(format!(
            "unsupported browser flow template reference {token}; expected inputs.<id> or runtime.<id>"
        ))
    })?;
    if key.trim().is_empty() {
        return Err(BlobError::Configuration(format!(
            "unsupported browser flow template reference {token}; missing key"
        )));
    }

    let value = match namespace {
        "inputs" => context.inputs.get(key),
        "runtime" => context.runtime.get(key),
        other => {
            return Err(BlobError::Configuration(format!(
                "unsupported browser flow template namespace {other} in {token}"
            )));
        }
    };

    value.cloned().ok_or_else(|| {
        BlobError::Configuration(format!("missing browser flow {namespace} value: {key}"))
    })
}

fn template_scalar_to_string(token: &str, value: &Value) -> Result<String, BlobError> {
    match value {
        Value::Null => Err(BlobError::Configuration(format!(
            "browser flow template {token} resolved to null"
        ))),
        Value::String(value) => Ok(value.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Array(_) | Value::Object(_) => Err(BlobError::Configuration(format!(
            "browser flow template {token} resolved to a non-scalar JSON value"
        ))),
    }
}

fn dry_run_step_report(
    plan: &BoundBrowserFlowPlan,
    step: &BrowserFlowStep,
) -> Result<BrowserFlowExecutionStepReport, BlobError> {
    let mut detail = BTreeMap::new();
    match step {
        BrowserFlowStep::Navigate {
            url, wait_for_page, ..
        } => {
            detail.insert("url".to_string(), Value::String(url.clone()));
            if let Some(page) = wait_for_page {
                detail.insert("wait_for_page".to_string(), Value::String(page.clone()));
            }
        }
        BrowserFlowStep::Click {
            element, optional, ..
        } => {
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert("optional".to_string(), Value::Bool(*optional));
        }
        BrowserFlowStep::SetInput {
            element,
            dispatch_events,
            ..
        } => {
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert("value_present".to_string(), Value::Bool(true));
            detail.insert(
                "dispatch_events".to_string(),
                Value::Array(dispatch_events.iter().cloned().map(Value::String).collect()),
            );
        }
        BrowserFlowStep::InvokeOperation { operation, .. } => {
            detail.insert("operation".to_string(), Value::String(operation.clone()));
            if let Some(bound_operation) = plan.find_operation(operation) {
                detail.insert(
                    "operation_kind".to_string(),
                    Value::String(format!("{:?}", bound_operation.kind).to_ascii_lowercase()),
                );
                if let Some(page) = &bound_operation.page {
                    detail.insert("page".to_string(), Value::String(page.clone()));
                }
                detail.insert(
                    "source_present".to_string(),
                    Value::Bool(!bound_operation.source.trim().is_empty()),
                );
            }
        }
        BrowserFlowStep::SetFiles {
            element, input_ref, ..
        } => {
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert("input_ref".to_string(), Value::String(input_ref.clone()));
            detail.insert(
                "input_present".to_string(),
                Value::Bool(plan.input_value(input_ref).is_some()),
            );
        }
        BrowserFlowStep::DispatchEvents {
            element, events, ..
        } => {
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert(
                "events".to_string(),
                Value::Array(events.iter().cloned().map(Value::String).collect()),
            );
        }
        BrowserFlowStep::WaitForRequest {
            request,
            timeout_ms,
            ..
        } => {
            detail.insert("request".to_string(), Value::String(request.clone()));
            if let Some(bound_request) = plan.find_request(request) {
                detail.insert(
                    "request_method".to_string(),
                    Value::String(bound_request.method.clone()),
                );
                detail.insert(
                    "request_url_pattern".to_string(),
                    Value::String(bound_request.url_pattern.clone()),
                );
            }
            if let Some(timeout_ms) = timeout_ms {
                detail.insert(
                    "timeout_ms".to_string(),
                    Value::Number(serde_json::Number::from(*timeout_ms)),
                );
            }
        }
        BrowserFlowStep::WaitForPage {
            page, timeout_ms, ..
        } => {
            detail.insert("page".to_string(), Value::String(page.clone()));
            if let Some(timeout_ms) = timeout_ms {
                detail.insert(
                    "timeout_ms".to_string(),
                    Value::Number(serde_json::Number::from(*timeout_ms)),
                );
            }
        }
        BrowserFlowStep::Wait { duration_ms, .. } => {
            detail.insert(
                "duration_ms".to_string(),
                Value::Number(serde_json::Number::from(*duration_ms)),
            );
        }
    }

    Ok(BrowserFlowExecutionStepReport {
        step_id: step.id().to_string(),
        step_kind: step.kind_name().to_string(),
        status: BrowserFlowExecutionStepStatus::Planned,
        detail,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::Value;
    use tokio::runtime::Runtime;

    use super::{
        BrowserFlowBindingContext, BrowserFlowCatalog, BrowserFlowCatalogCollection,
        BrowserFlowExecutionMode, BrowserFlowExecutionStepStatus, BrowserFlowExecutor,
        DryRunBrowserFlowExecutor,
    };
    use crate::BlobError;

    fn temp_catalog_dir(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("blob-core-{name}-{nanos}"))
    }

    fn unicom_catalog_fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/browser-flows/unicom-web.json")
    }

    #[test]
    fn unicom_browser_flow_catalog_parses_and_validates() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");

        assert_eq!(catalog.provider, "unicom");
        assert_eq!(catalog.surface, "pan.wo.cn-web");
        assert_eq!(catalog.flows.len(), 7);
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_create_directory")
        );
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_delete_entry")
        );
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_rename_entry")
        );
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_copy_entry")
        );
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_move_entry")
        );
    }

    #[test]
    fn browser_flow_catalog_rejects_unknown_element_reference() {
        let raw = r#"{
          "schema_version": 1,
          "provider": "example",
          "surface": "example-web",
          "base_url": "https://example.com",
          "pages": [
            {
              "id": "login",
              "title": "Login",
              "url_patterns": ["https://example.com/login"]
            }
          ],
          "elements": [
            {
              "id": "login.phone",
              "page": "login",
              "role": "text_input",
              "required": true,
              "selectors": [
                {
                  "engine": "css",
                  "value": "input[type='tel']"
                }
              ]
            }
          ],
          "requests": [],
          "operations": [],
          "flows": [
            {
              "id": "sms_login",
              "title": "SMS Login",
              "purpose": "Validate references",
              "start_page": "login",
              "inputs": [],
              "steps": [
                {
                  "kind": "click",
                  "id": "click-missing",
                  "element": "login.missing"
                }
              ]
            }
          ]
        }"#;

        let error = BrowserFlowCatalog::from_json_str(raw)
            .expect_err("catalog should fail when it references a missing element");
        assert!(
            error
                .to_string()
                .contains("references unknown element login.missing")
        );
    }

    #[test]
    fn browser_flow_catalog_collection_loads_directory_and_supports_lookup() {
        let dir = temp_catalog_dir("catalog-collection");
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let path = dir.join("unicom-web.json");
        fs::copy(unicom_catalog_fixture_path(), &path).expect("fixture catalog should copy");

        let collection = BrowserFlowCatalogCollection::from_json_dir(&dir)
            .expect("catalog collection should load");
        assert_eq!(collection.entries().len(), 1);
        let catalog = collection
            .get("unicom", "pan.wo.cn-web")
            .expect("unicom catalog should be found");
        assert!(catalog.find_flow("unicom_move_entry").is_some());
        assert_eq!(collection.entries()[0].source_path, path);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn browser_flow_catalog_collection_rejects_duplicate_provider_surface_pairs() {
        let dir = temp_catalog_dir("catalog-duplicate");
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let source = unicom_catalog_fixture_path();
        fs::copy(&source, dir.join("a.json")).expect("first fixture should copy");
        fs::copy(&source, dir.join("b.json")).expect("second fixture should copy");

        let error = BrowserFlowCatalogCollection::from_json_dir(&dir)
            .expect_err("duplicate provider/surface should fail");
        assert!(matches!(error, BlobError::Configuration(_)));
        assert!(
            error
                .to_string()
                .contains("duplicate browser flow catalog detected for unicom/pan.wo.cn-web")
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn browser_flow_catalog_bind_flow_resolves_templates_and_presets() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([
                (
                    "phone_number".to_string(),
                    Value::String("18500001111".to_string()),
                ),
                ("sms_code".to_string(), Value::String("123456".to_string())),
                (
                    "family_id".to_string(),
                    Value::String("family-42".to_string()),
                ),
                (
                    "ps_token".to_string(),
                    Value::String("private-token".to_string()),
                ),
                (
                    "local_file".to_string(),
                    Value::String("/tmp/example.txt".to_string()),
                ),
            ]),
            runtime: BTreeMap::from([
                (
                    "batch_no".to_string(),
                    Value::String("batch-100".to_string()),
                ),
                (
                    "directory_id".to_string(),
                    Value::String("dir-200".to_string()),
                ),
                (
                    "private_space_type".to_string(),
                    Value::String("4".to_string()),
                ),
                (
                    "access_token".to_string(),
                    Value::String("token-300".to_string()),
                ),
            ]),
        };

        let plan = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect("flow binding should succeed");

        assert_eq!(plan.provider, "unicom");
        assert_eq!(plan.surface, "pan.wo.cn-web");
        assert_eq!(plan.flow.id, "unicom_personal_root_upload");
        assert_eq!(plan.requests.len(), 3);
        assert!(plan.requests.iter().any(|request| request.id == "upload2c"
            && request.required_headers.iter().any(|header| {
                header.name == "origin"
                    && header.value_template.as_deref() == Some("https://pan.wo.cn")
            })));
        assert_eq!(
            plan.presets
                .get("family_upload_context")
                .and_then(|value| value.get("familyId"))
                .and_then(Value::as_str),
            Some("family-42")
        );
        assert_eq!(
            plan.presets
                .get("private_upload_context")
                .and_then(|value| value.get("psToken"))
                .and_then(Value::as_str),
            Some("private-token")
        );
    }

    #[test]
    fn browser_flow_catalog_bind_flow_rejects_missing_required_input() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");

        let error = catalog
            .bind_flow("unicom_sms_login", &BrowserFlowBindingContext::default())
            .expect_err("missing required flow inputs should fail");
        assert!(
            error
                .to_string()
                .contains("missing required browser flow input phone_number")
        );
    }

    #[test]
    fn browser_flow_catalog_bind_flow_rejects_missing_runtime_placeholder_value() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([(
                "local_file".to_string(),
                Value::String("/tmp/example.txt".to_string()),
            )]),
            runtime: BTreeMap::new(),
        };

        let error = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect_err("missing runtime template value should fail");
        assert!(
            error
                .to_string()
                .contains("missing browser flow runtime value: batch_no")
        );
    }

    #[test]
    fn dry_run_executor_reports_expected_upload_steps() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([
                (
                    "local_file".to_string(),
                    Value::String("/tmp/example.txt".to_string()),
                ),
                (
                    "family_id".to_string(),
                    Value::String("family-42".to_string()),
                ),
                (
                    "ps_token".to_string(),
                    Value::String("private-token".to_string()),
                ),
            ]),
            runtime: BTreeMap::from([
                (
                    "batch_no".to_string(),
                    Value::String("batch-100".to_string()),
                ),
                (
                    "directory_id".to_string(),
                    Value::String("dir-200".to_string()),
                ),
                (
                    "private_space_type".to_string(),
                    Value::String("4".to_string()),
                ),
                (
                    "access_token".to_string(),
                    Value::String("token-300".to_string()),
                ),
            ]),
        };
        let plan = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect("flow binding should succeed");

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(DryRunBrowserFlowExecutor.execute(&plan))
            .expect("dry run execution should succeed");

        assert_eq!(report.mode, BrowserFlowExecutionMode::DryRun);
        assert_eq!(report.flow_id, "unicom_personal_root_upload");
        assert_eq!(report.step_count, plan.flow.steps.len());
        assert_eq!(
            report.expected_requests,
            vec![
                "upload2c".to_string(),
                "wohome_query_all_files".to_string(),
                "member_card_info".to_string()
            ]
        );
        assert_eq!(report.steps.len(), plan.flow.steps.len());
        assert_eq!(report.steps[0].step_id, "open-uploader");
        assert_eq!(report.steps[0].step_kind, "invoke_operation");
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Planned
        );
        assert_eq!(
            report.steps[0].detail.get("operation"),
            Some(&Value::String(
                "file_list.open_personal_uploader".to_string()
            ))
        );
        assert_eq!(
            report.steps[0].detail.get("operation_kind"),
            Some(&Value::String("javascript".to_string()))
        );
        assert_eq!(
            report.steps[1].detail.get("input_present"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            report.steps[3].detail.get("request_method"),
            Some(&Value::String("POST".to_string()))
        );
    }

    #[test]
    fn dry_run_executor_marks_missing_optional_file_input_as_absent() {
        let raw = r#"{
          "schema_version": 1,
          "provider": "example",
          "surface": "example-web",
          "base_url": "https://example.com",
          "pages": [
            {
              "id": "upload",
              "title": "Upload Page",
              "url_patterns": ["https://example.com/upload"]
            }
          ],
          "elements": [
            {
              "id": "upload.file_input",
              "page": "upload",
              "role": "file_input",
              "required": true,
              "selectors": [
                {
                  "engine": "css",
                  "value": "input[type='file']"
                }
              ]
            }
          ],
          "requests": [],
          "operations": [],
          "flows": [
            {
              "id": "optional_file_upload",
              "title": "Optional File Upload",
              "purpose": "Exercise dry-run file attachment reporting",
              "start_page": "upload",
              "inputs": [
                {
                  "id": "optional_file",
                  "label": "Optional File",
                  "kind": "file",
                  "required": false
                }
              ],
              "steps": [
                {
                  "kind": "set_files",
                  "id": "attach-optional-file",
                  "element": "upload.file_input",
                  "input_ref": "optional_file"
                }
              ]
            }
          ]
        }"#;
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("inline browser flow catalog should parse and validate");
        let plan = catalog
            .bind_flow(
                "optional_file_upload",
                &BrowserFlowBindingContext::default(),
            )
            .expect("flow binding should succeed");

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(DryRunBrowserFlowExecutor.execute(&plan))
            .expect("dry run execution should succeed");

        assert_eq!(
            report.steps[0].detail.get("input_present"),
            Some(&Value::Bool(false))
        );
        assert!(
            report
                .steps
                .iter()
                .all(|step| step.status == BrowserFlowExecutionStepStatus::Planned)
        );
    }

    #[test]
    fn bound_browser_flow_plan_lookups_expose_bound_request_operation_and_input_values() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([
                (
                    "local_file".to_string(),
                    Value::String("/tmp/example.txt".to_string()),
                ),
                (
                    "family_id".to_string(),
                    Value::String("family-42".to_string()),
                ),
                (
                    "ps_token".to_string(),
                    Value::String("private-token".to_string()),
                ),
            ]),
            runtime: BTreeMap::from([
                (
                    "batch_no".to_string(),
                    Value::String("batch-100".to_string()),
                ),
                (
                    "directory_id".to_string(),
                    Value::String("dir-200".to_string()),
                ),
                (
                    "private_space_type".to_string(),
                    Value::String("4".to_string()),
                ),
                (
                    "access_token".to_string(),
                    Value::String("token-300".to_string()),
                ),
            ]),
        };
        let plan = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect("flow binding should succeed");

        assert_eq!(
            plan.find_request("upload2c")
                .map(|request| request.method.as_str()),
            Some("POST")
        );
        assert_eq!(
            plan.find_operation("file_list.open_personal_uploader")
                .map(|operation| operation.kind),
            Some(super::BrowserFlowOperationKind::Javascript)
        );
        assert_eq!(
            plan.input_value("local_file"),
            Some(&Value::String("/tmp/example.txt".to_string()))
        );
        assert!(plan.find_request("missing").is_none());
        assert!(plan.find_operation("missing").is_none());
        assert!(plan.input_value("missing").is_none());
    }
}
