// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

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
    pub frame: Option<String>,
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
    #[serde(default)]
    pub frame: Option<String>,
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
    pub prerequisite_flow_id: Option<String>,
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
    Session,
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

pub struct BrowserFlowSessionExecutor<S> {
    session: S,
}

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
    pub transient: bool,
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
    #[serde(default)]
    pub frame: Option<String>,
    pub source: String,
    #[serde(default)]
    pub fallback_sources: Vec<String>,
    #[serde(default)]
    pub header_names: Vec<String>,
    #[serde(default = "default_true")]
    pub required_for_prerequisite: bool,
    #[serde(default)]
    pub redact: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFlowOutputKind {
    RequestHeader,
    RequestHeaders,
    RequestField,
    ResponseField,
    ScriptValue,
    Url,
    DomText,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowVisualCaptchaRequest {
    pub image_element: BrowserFlowElement,
    pub input_element: BrowserFlowElement,
    #[serde(default)]
    pub refresh_element: Option<BrowserFlowElement>,
    pub input_id: String,
    #[serde(default)]
    pub manual_value: Option<String>,
    #[serde(default)]
    pub field_label: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub dispatch_events: Vec<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub expected_length: Option<usize>,
    #[serde(default)]
    pub llm_system_prompt: Option<String>,
    #[serde(default)]
    pub llm_prompt_template: Option<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub max_attempts: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowVisualLayoutTarget {
    pub element: String,
    pub role: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowVisualLayoutValidationTargetRequest {
    pub element: BrowserFlowElement,
    pub role: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrowserFlowVisualLayoutValidationRequest {
    pub targets: Vec<BrowserFlowVisualLayoutValidationTargetRequest>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub relationship_rules: Vec<String>,
    #[serde(default)]
    pub optional: bool,
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
    ValidateVisualLayout {
        id: String,
        targets: Vec<BrowserFlowVisualLayoutTarget>,
        #[serde(default)]
        instruction: Option<String>,
        #[serde(default)]
        relationship_rules: Vec<String>,
        #[serde(default)]
        optional: bool,
    },
    SolveVisualCaptcha {
        id: String,
        image_element: String,
        input_element: String,
        input_id: String,
        #[serde(default)]
        field_label: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        dispatch_events: Vec<String>,
        #[serde(default)]
        instruction: Option<String>,
        #[serde(default)]
        expected_length: Option<usize>,
        #[serde(default)]
        llm_system_prompt: Option<String>,
        #[serde(default)]
        llm_prompt_template: Option<String>,
        #[serde(default)]
        refresh_element: Option<String>,
        #[serde(default)]
        optional: bool,
        #[serde(default)]
        max_attempts: Option<u64>,
    },
    WaitForRequest {
        id: String,
        request: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        optional: bool,
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
                BrowserFlowStep::ValidateVisualLayout { targets, .. } => {
                    for target in targets {
                        referenced_element_ids.insert(target.element.clone());
                    }
                }
                BrowserFlowStep::SolveVisualCaptcha {
                    image_element,
                    input_element,
                    refresh_element,
                    ..
                } => {
                    referenced_element_ids.insert(image_element.clone());
                    referenced_element_ids.insert(input_element.clone());
                    if let Some(refresh_element) = refresh_element {
                        referenced_element_ids.insert(refresh_element.clone());
                    }
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

        for output in &bound_flow.outputs {
            match output.kind {
                BrowserFlowOutputKind::RequestHeader
                | BrowserFlowOutputKind::RequestHeaders
                | BrowserFlowOutputKind::RequestField => {
                    for source in std::iter::once(output.source.as_str())
                        .chain(output.fallback_sources.iter().map(String::as_str))
                    {
                        if let Some((request_id, _)) = source.split_once(':') {
                            let request_id = request_id.trim();
                            if !request_id.is_empty() {
                                referenced_request_ids.insert(request_id.to_string());
                            }
                        } else if matches!(output.kind, BrowserFlowOutputKind::RequestHeaders) {
                            let request_id = source.trim();
                            if !request_id.is_empty() {
                                referenced_request_ids.insert(request_id.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
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
            if let Some(frame) = element.frame.as_deref() {
                ensure_non_empty("element.frame", frame)?;
            }
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
            if let Some(frame) = operation.frame.as_deref() {
                ensure_non_empty("operation.frame", frame)?;
            }
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
            if let Some(prerequisite_flow_id) = flow.prerequisite_flow_id.as_deref() {
                ensure_non_empty("flow.prerequisite_flow_id", prerequisite_flow_id)?;
                if prerequisite_flow_id == flow.id {
                    return Err(BlobError::Configuration(format!(
                        "flow {} must not reference itself as prerequisite_flow_id",
                        flow.id
                    )));
                }
                if !flow_ids.contains(prerequisite_flow_id) {
                    return Err(BlobError::Configuration(format!(
                        "flow {} references unknown prerequisite flow {}",
                        flow.id, prerequisite_flow_id
                    )));
                }
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
                for source in &output.fallback_sources {
                    ensure_non_empty("flow.output.fallback_sources[]", source.as_str())?;
                }
                if matches!(output.kind, BrowserFlowOutputKind::RequestHeaders) {
                    if output.header_names.is_empty() {
                        return Err(BlobError::Configuration(format!(
                            "flow output {} requires at least one header_names entry",
                            output.id
                        )));
                    }
                    for header_name in &output.header_names {
                        ensure_non_empty("flow.output.header_names[]", header_name.as_str())?;
                    }
                }
                if let Some(frame) = output.frame.as_deref() {
                    ensure_non_empty("flow.output.frame", frame)?;
                }
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
                    BrowserFlowStep::ValidateVisualLayout { targets, .. } => {
                        if targets.is_empty() {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} must contain at least one visual validation target",
                                flow.id,
                                step.id()
                            )));
                        }
                        for target in targets {
                            ensure_non_empty(
                                "flow.step.visual_target.element",
                                target.element.as_str(),
                            )?;
                            ensure_non_empty("flow.step.visual_target.role", target.role.as_str())?;
                            if !element_ids.contains(target.element.as_str()) {
                                return Err(BlobError::Configuration(format!(
                                    "flow {} step {} references unknown visual validation element {}",
                                    flow.id,
                                    step.id(),
                                    target.element
                                )));
                            }
                        }
                    }
                    BrowserFlowStep::SolveVisualCaptcha {
                        image_element,
                        input_element,
                        refresh_element,
                        input_id,
                        expected_length,
                        max_attempts,
                        ..
                    } => {
                        ensure_non_empty("flow.step.image_element", image_element.as_str())?;
                        ensure_non_empty("flow.step.input_element", input_element.as_str())?;
                        ensure_non_empty("flow.step.input_id", input_id.as_str())?;
                        if !element_ids.contains(image_element.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown image element {}",
                                flow.id,
                                step.id(),
                                image_element
                            )));
                        }
                        if !element_ids.contains(input_element.as_str()) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} references unknown input element {}",
                                flow.id,
                                step.id(),
                                input_element
                            )));
                        }
                        if let Some(refresh_element) = refresh_element {
                            if !element_ids.contains(refresh_element.as_str()) {
                                return Err(BlobError::Configuration(format!(
                                    "flow {} step {} references unknown refresh element {}",
                                    flow.id,
                                    step.id(),
                                    refresh_element
                                )));
                            }
                        }
                        if expected_length.is_some_and(|value| value == 0) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} expected_length must be at least 1",
                                flow.id,
                                step.id()
                            )));
                        }
                        if max_attempts.is_some_and(|value| value == 0) {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} max_attempts must be at least 1",
                                flow.id,
                                step.id()
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
                    BrowserFlowStep::ValidateVisualLayout {
                        instruction,
                        relationship_rules,
                        ..
                    } => {
                        if let Some(instruction) = instruction.as_deref() {
                            ensure_non_empty("flow.step.instruction", instruction)?;
                        }
                        for rule in relationship_rules {
                            ensure_non_empty("flow.step.relationship_rules[]", rule.as_str())?;
                        }
                    }
                    BrowserFlowStep::SolveVisualCaptcha {
                        dispatch_events,
                        llm_system_prompt,
                        llm_prompt_template,
                        ..
                    } => {
                        if dispatch_events.is_empty() {
                            return Err(BlobError::Configuration(format!(
                                "flow {} step {} must contain at least one dispatch event",
                                flow.id,
                                step.id()
                            )));
                        }
                        for event in dispatch_events {
                            ensure_non_empty("flow.step.dispatch_events[]", event.as_str())?;
                        }
                        if let Some(llm_system_prompt) = llm_system_prompt.as_deref() {
                            ensure_non_empty("flow.step.llm_system_prompt", llm_system_prompt)?;
                        }
                        if let Some(llm_prompt_template) = llm_prompt_template.as_deref() {
                            ensure_non_empty("flow.step.llm_prompt_template", llm_prompt_template)?;
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

        let prerequisite_by_flow = self
            .flows
            .iter()
            .map(|flow| (flow.id.as_str(), flow.prerequisite_flow_id.as_deref()))
            .collect::<BTreeMap<_, _>>();
        for flow in &self.flows {
            let mut seen_prerequisites = BTreeSet::new();
            let mut cursor = flow.prerequisite_flow_id.as_deref();
            while let Some(prerequisite_flow_id) = cursor {
                if !seen_prerequisites.insert(prerequisite_flow_id) {
                    return Err(BlobError::Configuration(format!(
                        "flow {} participates in a prerequisite cycle at {}",
                        flow.id, prerequisite_flow_id
                    )));
                }
                cursor = prerequisite_by_flow
                    .get(prerequisite_flow_id)
                    .copied()
                    .flatten();
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
    pub fn find_page(&self, page_id: &str) -> Option<&BrowserFlowPage> {
        self.pages.iter().find(|page| page.id == page_id)
    }

    pub fn find_element(&self, element_id: &str) -> Option<&BrowserFlowElement> {
        self.elements
            .iter()
            .find(|element| element.id == element_id)
    }

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

    pub fn find_input(&self, input_id: &str) -> Option<&BrowserFlowInput> {
        self.flow.inputs.iter().find(|input| input.id == input_id)
    }

    pub fn input_value(&self, input_id: &str) -> Option<&Value> {
        self.context.inputs.get(input_id)
    }

    pub fn input_file_paths(&self, input_id: &str) -> Result<Option<Vec<String>>, BlobError> {
        let Some(value) = self.input_value(input_id) else {
            return Ok(None);
        };
        browser_flow_file_paths(input_id, value)
    }
}

impl<S> BrowserFlowSessionExecutor<S> {
    pub fn new(session: S) -> Self {
        Self { session }
    }

    pub fn session(&self) -> &S {
        &self.session
    }

    pub fn into_inner(self) -> S {
        self.session
    }
}

#[async_trait]
pub trait BrowserFlowSession: Send + Sync {
    async fn navigate(&self, url: &str) -> Result<(), BlobError>;

    async fn click(&self, element: &BrowserFlowElement) -> Result<(), BlobError>;

    async fn set_input(
        &self,
        element: &BrowserFlowElement,
        value: &str,
        dispatch_events: &[String],
    ) -> Result<(), BlobError>;

    async fn invoke_operation(&self, operation: &BrowserFlowOperation) -> Result<(), BlobError>;

    async fn set_files(
        &self,
        element: &BrowserFlowElement,
        paths: &[String],
    ) -> Result<(), BlobError>;

    async fn dispatch_events(
        &self,
        element: &BrowserFlowElement,
        events: &[String],
    ) -> Result<(), BlobError>;

    async fn validate_visual_layout(
        &self,
        request: &BrowserFlowVisualLayoutValidationRequest,
    ) -> Result<(), BlobError>;

    async fn solve_visual_captcha(
        &self,
        request: &BrowserFlowVisualCaptchaRequest,
    ) -> Result<(), BlobError>;

    async fn wait_for_request(
        &self,
        request: &BrowserFlowRequest,
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError>;

    async fn wait_for_page(
        &self,
        page: &BrowserFlowPage,
        timeout_ms: Option<u64>,
    ) -> Result<(), BlobError>;

    async fn wait(&self, duration_ms: u64) -> Result<(), BlobError>;
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

#[async_trait]
impl<S> BrowserFlowExecutor for BrowserFlowSessionExecutor<S>
where
    S: BrowserFlowSession,
{
    fn mode(&self) -> BrowserFlowExecutionMode {
        BrowserFlowExecutionMode::Session
    }

    async fn execute(
        &self,
        plan: &BoundBrowserFlowPlan,
    ) -> Result<BrowserFlowExecutionReport, BlobError> {
        let mut state = BrowserFlowSessionExecutionState::default();
        let mut steps = Vec::with_capacity(plan.flow.steps.len());
        for step in &plan.flow.steps {
            steps.push(execute_session_step(plan, &self.session, step, &mut state).await?);
        }

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

#[derive(Default)]
struct BrowserFlowSessionExecutionState {
    consumed_transient_inputs: BTreeSet<String>,
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
            | Self::ValidateVisualLayout { id, .. }
            | Self::SolveVisualCaptcha { id, .. }
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
            Self::ValidateVisualLayout { .. } => "validate_visual_layout",
            Self::SolveVisualCaptcha { .. } => "solve_visual_captcha",
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
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
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
            BrowserFlowStep::ValidateVisualLayout {
                instruction,
                relationship_rules,
                ..
            } => {
                if let Some(instruction) = instruction {
                    *instruction = bind_string_template(instruction, context)?;
                }
                for rule in relationship_rules {
                    *rule = bind_string_template(rule, context)?;
                }
            }
            BrowserFlowStep::SolveVisualCaptcha {
                field_label,
                placeholder,
                instruction,
                llm_system_prompt,
                llm_prompt_template,
                ..
            } => {
                if let Some(field_label) = field_label {
                    *field_label = bind_string_template(field_label, context)?;
                }
                if let Some(placeholder) = placeholder {
                    *placeholder = bind_string_template(placeholder, context)?;
                }
                if let Some(instruction) = instruction {
                    *instruction = bind_string_template(instruction, context)?;
                }
                if let Some(llm_system_prompt) = llm_system_prompt {
                    *llm_system_prompt = bind_string_template(llm_system_prompt, context)?;
                }
                if let Some(llm_prompt_template) = llm_prompt_template {
                    *llm_prompt_template = bind_string_template(llm_prompt_template, context)?;
                }
            }
            _ => {}
        }
    }

    for output in &mut flow.outputs {
        if let Some(frame) = &mut output.frame {
            *frame = bind_string_template(frame, context)?;
        }
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
    if let Some(frame) = &mut bound.frame {
        *frame = bind_string_template(frame, context)?;
    }
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
    if let Some(frame) = &mut bound.frame {
        *frame = bind_string_template(frame, context)?;
    }
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

fn browser_flow_file_paths(
    input_id: &str,
    value: &Value,
) -> Result<Option<Vec<String>>, BlobError> {
    match value {
        Value::Null => Ok(None),
        Value::String(path) => {
            if path.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(vec![path.clone()]))
            }
        }
        Value::Array(items) => {
            let mut paths = Vec::new();
            for item in items {
                let path = item.as_str().ok_or_else(|| {
                    BlobError::Configuration(format!(
                        "browser flow file input {input_id} must contain only string paths"
                    ))
                })?;
                if !path.trim().is_empty() {
                    paths.push(path.to_string());
                }
            }
            if paths.is_empty() {
                Ok(None)
            } else {
                Ok(Some(paths))
            }
        }
        _ => Err(BlobError::Configuration(format!(
            "browser flow file input {input_id} must resolve to a string path or array of string paths"
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
            value_template,
            dispatch_events,
            ..
        } => {
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert(
                "value_present".to_string(),
                Value::Bool(value_is_present(&Value::String(value_template.clone()))),
            );
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
            let file_paths = plan.input_file_paths(input_ref)?;
            detail.insert("element".to_string(), Value::String(element.clone()));
            detail.insert("input_ref".to_string(), Value::String(input_ref.clone()));
            detail.insert(
                "input_present".to_string(),
                Value::Bool(file_paths.is_some()),
            );
            if let Some(file_paths) = file_paths {
                detail.insert(
                    "file_count".to_string(),
                    Value::Number(serde_json::Number::from(file_paths.len())),
                );
            }
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
        BrowserFlowStep::ValidateVisualLayout {
            targets,
            instruction,
            relationship_rules,
            optional,
            ..
        } => {
            detail.insert("optional".to_string(), Value::Bool(*optional));
            detail.insert(
                "targets".to_string(),
                Value::Array(
                    targets
                        .iter()
                        .map(|target| {
                            Value::Object(serde_json::Map::from_iter([
                                ("element".to_string(), Value::String(target.element.clone())),
                                ("role".to_string(), Value::String(target.role.clone())),
                                ("required".to_string(), Value::Bool(target.required)),
                            ]))
                        })
                        .collect(),
                ),
            );
            if let Some(instruction) = instruction {
                detail.insert(
                    "instruction".to_string(),
                    Value::String(instruction.clone()),
                );
            }
            if !relationship_rules.is_empty() {
                detail.insert(
                    "relationship_rules".to_string(),
                    Value::Array(
                        relationship_rules
                            .iter()
                            .cloned()
                            .map(Value::String)
                            .collect(),
                    ),
                );
            }
        }
        BrowserFlowStep::SolveVisualCaptcha {
            image_element,
            input_element,
            input_id,
            field_label,
            placeholder,
            dispatch_events,
            instruction,
            expected_length,
            llm_system_prompt,
            llm_prompt_template,
            refresh_element,
            optional,
            max_attempts,
            ..
        } => {
            detail.insert(
                "image_element".to_string(),
                Value::String(image_element.clone()),
            );
            detail.insert(
                "input_element".to_string(),
                Value::String(input_element.clone()),
            );
            detail.insert("input_id".to_string(), Value::String(input_id.clone()));
            detail.insert("optional".to_string(), Value::Bool(*optional));
            detail.insert(
                "dispatch_events".to_string(),
                Value::Array(dispatch_events.iter().cloned().map(Value::String).collect()),
            );
            detail.insert(
                "manual_value_present".to_string(),
                Value::Bool(
                    plan.input_value(input_id)
                        .is_some_and(|value| value_is_present(value)),
                ),
            );
            if let Some(field_label) = field_label {
                detail.insert(
                    "field_label".to_string(),
                    Value::String(field_label.clone()),
                );
            }
            if let Some(placeholder) = placeholder {
                detail.insert(
                    "placeholder".to_string(),
                    Value::String(placeholder.clone()),
                );
            }
            if let Some(instruction) = instruction {
                detail.insert(
                    "instruction".to_string(),
                    Value::String(instruction.clone()),
                );
            }
            if let Some(llm_system_prompt) = llm_system_prompt {
                detail.insert(
                    "llm_system_prompt".to_string(),
                    Value::String(llm_system_prompt.clone()),
                );
            }
            if let Some(llm_prompt_template) = llm_prompt_template {
                detail.insert(
                    "llm_prompt_template".to_string(),
                    Value::String(llm_prompt_template.clone()),
                );
            }
            if let Some(expected_length) = expected_length {
                detail.insert(
                    "expected_length".to_string(),
                    Value::Number(serde_json::Number::from(*expected_length as u64)),
                );
            }
            if let Some(refresh_element) = refresh_element {
                detail.insert(
                    "refresh_element".to_string(),
                    Value::String(refresh_element.clone()),
                );
            }
            if let Some(max_attempts) = max_attempts {
                detail.insert(
                    "max_attempts".to_string(),
                    Value::Number(serde_json::Number::from(*max_attempts)),
                );
            }
        }
        BrowserFlowStep::WaitForRequest {
            request,
            timeout_ms,
            optional,
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
            detail.insert("optional".to_string(), Value::Bool(*optional));
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

async fn execute_session_step<S>(
    plan: &BoundBrowserFlowPlan,
    session: &S,
    step: &BrowserFlowStep,
    state: &mut BrowserFlowSessionExecutionState,
) -> Result<BrowserFlowExecutionStepReport, BlobError>
where
    S: BrowserFlowSession,
{
    let mut report = dry_run_step_report(plan, step)?;
    match step {
        BrowserFlowStep::Navigate {
            url, wait_for_page, ..
        } => {
            session.navigate(url).await?;
            if let Some(page_id) = wait_for_page {
                session
                    .wait_for_page(
                        plan.find_page(page_id).ok_or_else(|| {
                            missing_bound_reference("page", page_id, &plan.flow.id)
                        })?,
                        None,
                    )
                    .await?;
            }
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::Click {
            element, optional, ..
        } => {
            let element = plan
                .find_element(element)
                .ok_or_else(|| missing_bound_reference("element", element, &plan.flow.id))?;
            match session.click(element).await {
                Ok(()) => {
                    report.status = BrowserFlowExecutionStepStatus::Succeeded;
                }
                Err(BlobError::NotFound(_)) if *optional => {
                    report.status = BrowserFlowExecutionStepStatus::Skipped;
                    report.detail.insert(
                        "skip_reason".to_string(),
                        Value::String("optional_element_not_found".to_string()),
                    );
                }
                Err(error) => return Err(error),
            }
        }
        BrowserFlowStep::SetInput {
            element,
            value_template,
            dispatch_events,
            ..
        } => {
            session
                .set_input(
                    plan.find_element(element).ok_or_else(|| {
                        missing_bound_reference("element", element, &plan.flow.id)
                    })?,
                    value_template,
                    dispatch_events,
                )
                .await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::InvokeOperation { operation, .. } => {
            session
                .invoke_operation(plan.find_operation(operation).ok_or_else(|| {
                    missing_bound_reference("operation", operation, &plan.flow.id)
                })?)
                .await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::SetFiles {
            element, input_ref, ..
        } => {
            let Some(file_paths) = plan.input_file_paths(input_ref)? else {
                report.status = BrowserFlowExecutionStepStatus::Skipped;
                report.detail.insert(
                    "skip_reason".to_string(),
                    Value::String("missing_input".to_string()),
                );
                return Ok(report);
            };
            session
                .set_files(
                    plan.find_element(element).ok_or_else(|| {
                        missing_bound_reference("element", element, &plan.flow.id)
                    })?,
                    &file_paths,
                )
                .await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::DispatchEvents {
            element, events, ..
        } => {
            session
                .dispatch_events(
                    plan.find_element(element).ok_or_else(|| {
                        missing_bound_reference("element", element, &plan.flow.id)
                    })?,
                    events,
                )
                .await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::ValidateVisualLayout {
            targets,
            instruction,
            relationship_rules,
            optional,
            ..
        } => {
            let request = BrowserFlowVisualLayoutValidationRequest {
                targets: targets
                    .iter()
                    .map(|target| {
                        Ok(BrowserFlowVisualLayoutValidationTargetRequest {
                            element: plan
                                .find_element(&target.element)
                                .ok_or_else(|| {
                                    missing_bound_reference(
                                        "element",
                                        &target.element,
                                        &plan.flow.id,
                                    )
                                })?
                                .clone(),
                            role: target.role.clone(),
                            required: target.required,
                        })
                    })
                    .collect::<Result<Vec<_>, BlobError>>()?,
                instruction: instruction.clone(),
                relationship_rules: relationship_rules.clone(),
                optional: *optional,
            };
            match session.validate_visual_layout(&request).await {
                Ok(()) => {
                    report.status = BrowserFlowExecutionStepStatus::Succeeded;
                }
                Err(BlobError::NotImplemented(_)) if *optional => {
                    report.status = BrowserFlowExecutionStepStatus::Skipped;
                    report.detail.insert(
                        "skip_reason".to_string(),
                        Value::String("visual_validation_unavailable".to_string()),
                    );
                }
                Err(error) => return Err(error),
            }
        }
        BrowserFlowStep::SolveVisualCaptcha {
            image_element,
            input_element,
            input_id,
            field_label,
            placeholder,
            dispatch_events,
            instruction,
            expected_length,
            llm_system_prompt,
            llm_prompt_template,
            refresh_element,
            optional,
            max_attempts,
            ..
        } => {
            let input_is_transient = plan
                .find_input(input_id)
                .is_some_and(|input| input.transient);
            let manual_value =
                if input_is_transient && state.consumed_transient_inputs.contains(input_id) {
                    None
                } else {
                    plan.input_value(input_id)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToString::to_string)
                };
            let request = BrowserFlowVisualCaptchaRequest {
                image_element: plan
                    .find_element(image_element)
                    .ok_or_else(|| {
                        missing_bound_reference("element", image_element, &plan.flow.id)
                    })?
                    .clone(),
                input_element: plan
                    .find_element(input_element)
                    .ok_or_else(|| {
                        missing_bound_reference("element", input_element, &plan.flow.id)
                    })?
                    .clone(),
                refresh_element: refresh_element
                    .as_deref()
                    .map(|element_id| {
                        plan.find_element(element_id).ok_or_else(|| {
                            missing_bound_reference("element", element_id, &plan.flow.id)
                        })
                    })
                    .transpose()?
                    .cloned(),
                input_id: input_id.clone(),
                manual_value,
                field_label: field_label.clone(),
                placeholder: placeholder.clone(),
                dispatch_events: dispatch_events.clone(),
                instruction: instruction.clone(),
                expected_length: *expected_length,
                llm_system_prompt: llm_system_prompt.clone(),
                llm_prompt_template: llm_prompt_template.clone(),
                optional: *optional,
                max_attempts: *max_attempts,
            };
            match session.solve_visual_captcha(&request).await {
                Ok(()) => {
                    if input_is_transient && request.manual_value.is_some() {
                        state.consumed_transient_inputs.insert(input_id.clone());
                    }
                    report.status = BrowserFlowExecutionStepStatus::Succeeded;
                }
                Err(BlobError::NotFound(_)) if *optional => {
                    report.status = BrowserFlowExecutionStepStatus::Skipped;
                    report.detail.insert(
                        "skip_reason".to_string(),
                        Value::String("optional_captcha_not_found".to_string()),
                    );
                }
                Err(error) => return Err(error),
            }
        }
        BrowserFlowStep::WaitForRequest {
            request,
            timeout_ms,
            optional,
            ..
        } => {
            match session
                .wait_for_request(
                    plan.find_request(request).ok_or_else(|| {
                        missing_bound_reference("request", request, &plan.flow.id)
                    })?,
                    *timeout_ms,
                )
                .await
            {
                Ok(()) => {
                    report.status = BrowserFlowExecutionStepStatus::Succeeded;
                }
                Err(BlobError::Upstream(_)) if *optional => {
                    report.status = BrowserFlowExecutionStepStatus::Skipped;
                    report.detail.insert(
                        "skip_reason".to_string(),
                        Value::String("optional_request_not_observed".to_string()),
                    );
                }
                Err(error) => return Err(error),
            }
        }
        BrowserFlowStep::WaitForPage {
            page, timeout_ms, ..
        } => {
            session
                .wait_for_page(
                    plan.find_page(page)
                        .ok_or_else(|| missing_bound_reference("page", page, &plan.flow.id))?,
                    *timeout_ms,
                )
                .await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
        BrowserFlowStep::Wait { duration_ms, .. } => {
            session.wait(*duration_ms).await?;
            report.status = BrowserFlowExecutionStepStatus::Succeeded;
        }
    }

    Ok(report)
}

fn missing_bound_reference(kind: &str, id: &str, flow_id: &str) -> BlobError {
    BlobError::Configuration(format!(
        "bound browser flow {flow_id} is missing referenced {kind} {id}"
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashSet},
        fs,
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use async_trait::async_trait;
    use serde_json::Value;
    use tokio::runtime::Runtime;

    use super::{
        BrowserFlowBindingContext, BrowserFlowCatalog, BrowserFlowCatalogCollection,
        BrowserFlowExecutionMode, BrowserFlowExecutionStepStatus, BrowserFlowExecutor,
        BrowserFlowSession, BrowserFlowSessionExecutor, BrowserFlowStep,
        BrowserFlowVisualCaptchaRequest, BrowserFlowVisualLayoutValidationRequest,
        DryRunBrowserFlowExecutor,
    };
    use crate::BlobError;

    #[derive(Default)]
    struct RecordingBrowserFlowSession {
        actions: Mutex<Vec<String>>,
        missing_click_elements: Mutex<HashSet<String>>,
        failing_operation: Mutex<Option<String>>,
        missing_wait_requests: Mutex<HashSet<String>>,
    }

    impl RecordingBrowserFlowSession {
        fn actions(&self) -> Vec<String> {
            self.actions
                .lock()
                .expect("recorded actions should lock")
                .clone()
        }

        fn mark_click_missing(&self, element_id: &str) {
            self.missing_click_elements
                .lock()
                .expect("missing click set should lock")
                .insert(element_id.to_string());
        }

        fn fail_operation(&self, operation_id: &str) {
            *self
                .failing_operation
                .lock()
                .expect("failing operation should lock") = Some(operation_id.to_string());
        }

        fn mark_wait_request_missing(&self, request_id: &str) {
            self.missing_wait_requests
                .lock()
                .expect("missing wait request set should lock")
                .insert(request_id.to_string());
        }

        fn record(&self, action: impl Into<String>) {
            self.actions
                .lock()
                .expect("recorded actions should lock")
                .push(action.into());
        }
    }

    #[async_trait]
    impl BrowserFlowSession for RecordingBrowserFlowSession {
        async fn navigate(&self, url: &str) -> Result<(), BlobError> {
            self.record(format!("navigate:{url}"));
            Ok(())
        }

        async fn click(&self, element: &super::BrowserFlowElement) -> Result<(), BlobError> {
            if self
                .missing_click_elements
                .lock()
                .expect("missing click set should lock")
                .contains(&element.id)
            {
                return Err(BlobError::NotFound(format!(
                    "element not found: {}",
                    element.id
                )));
            }
            self.record(format!("click:{}", element.id));
            Ok(())
        }

        async fn set_input(
            &self,
            element: &super::BrowserFlowElement,
            value: &str,
            dispatch_events: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!(
                "set_input:{}:{}:{}",
                element.id,
                value,
                dispatch_events.join(",")
            ));
            Ok(())
        }

        async fn invoke_operation(
            &self,
            operation: &super::BrowserFlowOperation,
        ) -> Result<(), BlobError> {
            if self
                .failing_operation
                .lock()
                .expect("failing operation should lock")
                .as_deref()
                == Some(operation.id.as_str())
            {
                return Err(BlobError::Upstream(format!(
                    "operation failed: {}",
                    operation.id
                )));
            }
            self.record(format!("invoke_operation:{}", operation.id));
            Ok(())
        }

        async fn set_files(
            &self,
            element: &super::BrowserFlowElement,
            paths: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!("set_files:{}:{}", element.id, paths.join("|")));
            Ok(())
        }

        async fn dispatch_events(
            &self,
            element: &super::BrowserFlowElement,
            events: &[String],
        ) -> Result<(), BlobError> {
            self.record(format!(
                "dispatch_events:{}:{}",
                element.id,
                events.join(",")
            ));
            Ok(())
        }

        async fn validate_visual_layout(
            &self,
            request: &BrowserFlowVisualLayoutValidationRequest,
        ) -> Result<(), BlobError> {
            self.record(format!(
                "validate_visual_layout:{}",
                request
                    .targets
                    .iter()
                    .map(|target| format!("{}={}", target.role, target.element.id))
                    .collect::<Vec<_>>()
                    .join("|")
            ));
            Ok(())
        }

        async fn solve_visual_captcha(
            &self,
            request: &BrowserFlowVisualCaptchaRequest,
        ) -> Result<(), BlobError> {
            match request.manual_value.as_deref() {
                Some(value) => {
                    self.record(format!(
                        "solve_visual_captcha:{}:{}:{}",
                        request.image_element.id, request.input_element.id, value
                    ));
                    Ok(())
                }
                None if request.optional => Err(BlobError::NotFound(format!(
                    "optional captcha not present: {}",
                    request.image_element.id
                ))),
                None => Err(BlobError::InteractiveInputRequired(
                    request.input_id.clone(),
                )),
            }
        }

        async fn wait_for_request(
            &self,
            request: &super::BrowserFlowRequest,
            timeout_ms: Option<u64>,
        ) -> Result<(), BlobError> {
            if self
                .missing_wait_requests
                .lock()
                .expect("missing wait request set should lock")
                .contains(&request.id)
            {
                return Err(BlobError::Upstream(format!(
                    "timed out waiting for request {}",
                    request.id
                )));
            }
            self.record(format!(
                "wait_for_request:{}:{}",
                request.id,
                timeout_ms
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            ));
            Ok(())
        }

        async fn wait_for_page(
            &self,
            page: &super::BrowserFlowPage,
            timeout_ms: Option<u64>,
        ) -> Result<(), BlobError> {
            self.record(format!(
                "wait_for_page:{}:{}",
                page.id,
                timeout_ms
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            ));
            Ok(())
        }

        async fn wait(&self, duration_ms: u64) -> Result<(), BlobError> {
            self.record(format!("wait:{duration_ms}"));
            Ok(())
        }
    }

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

    fn session_executor_catalog() -> BrowserFlowCatalog {
        BrowserFlowCatalog::from_json_str(
            r#"{
              "schema_version": 1,
              "provider": "example",
              "surface": "example-web",
              "base_url": "https://example.com",
              "pages": [
                {
                  "id": "start",
                  "title": "Start Page",
                  "url_patterns": ["https://example.com/start"]
                },
                {
                  "id": "done",
                  "title": "Done Page",
                  "url_patterns": ["https://example.com/done"]
                }
              ],
              "elements": [
                {
                  "id": "form.submit_button",
                  "page": "start",
                  "role": "button",
                  "required": true,
                  "selectors": [{ "engine": "css", "value": "button[type='submit']" }]
                },
                {
                  "id": "form.name_input",
                  "page": "start",
                  "role": "text_input",
                  "required": true,
                  "selectors": [{ "engine": "css", "value": "input[name='name']" }]
                },
                {
                  "id": "form.file_input",
                  "page": "start",
                  "role": "file_input",
                  "required": true,
                  "selectors": [{ "engine": "css", "value": "input[type='file']" }]
                },
                {
                  "id": "form.optional_button",
                  "page": "start",
                  "role": "button",
                  "required": false,
                  "selectors": [{ "engine": "css", "value": ".optional-button" }]
                },
                {
                  "id": "form.captcha_image",
                  "page": "start",
                  "role": "image",
                  "required": false,
                  "selectors": [{ "engine": "css", "value": "img.captcha" }]
                },
                {
                  "id": "form.captcha_input",
                  "page": "start",
                  "role": "text_input",
                  "required": false,
                  "selectors": [{ "engine": "css", "value": "input[name='captcha']" }]
                }
              ],
              "requests": [
                {
                  "id": "save_request",
                  "method": "POST",
                  "url_pattern": "https://example.com/api/save",
                  "required_headers": [],
                  "required_fields": [],
                  "success_codes": [200]
                }
              ],
              "operations": [
                {
                  "id": "form.prepare_save",
                  "kind": "javascript",
                  "source": "window.prepareSave()"
                }
              ],
              "flows": [
                {
                  "id": "complete_flow",
                  "title": "Complete Flow",
                  "purpose": "Exercise session executor",
                  "start_page": "start",
                  "inputs": [
                    {
                      "id": "name",
                      "label": "Name",
                      "kind": "text",
                      "required": true
                    },
                    {
                      "id": "files",
                      "label": "Files",
                      "kind": "file",
                      "required": true
                    }
                  ],
                  "steps": [
                    {
                      "kind": "navigate",
                      "id": "open-start",
                      "url": "https://example.com/start",
                      "wait_for_page": "start"
                    },
                    {
                      "kind": "set_input",
                      "id": "fill-name",
                      "element": "form.name_input",
                      "value_template": "{{inputs.name}}",
                      "dispatch_events": ["input", "change"]
                    },
                    {
                      "kind": "set_files",
                      "id": "attach-files",
                      "element": "form.file_input",
                      "input_ref": "files"
                    },
                    {
                      "kind": "dispatch_events",
                      "id": "fire-file-events",
                      "element": "form.file_input",
                      "events": ["input", "change"]
                    },
                    {
                      "kind": "invoke_operation",
                      "id": "prepare-save",
                      "operation": "form.prepare_save"
                    },
                    {
                      "kind": "click",
                      "id": "submit-form",
                      "element": "form.submit_button"
                    },
                    {
                      "kind": "wait_for_request",
                      "id": "wait-save-request",
                      "request": "save_request",
                      "timeout_ms": 30000
                    },
                    {
                      "kind": "wait_for_page",
                      "id": "wait-done-page",
                      "page": "done",
                      "timeout_ms": 4000
                    },
                    {
                      "kind": "wait",
                      "id": "settle",
                      "duration_ms": 250
                    }
                  ],
                  "expected_requests": ["save_request"]
                },
                {
                  "id": "optional_flow",
                  "title": "Optional Flow",
                  "purpose": "Exercise skipped steps",
                  "start_page": "start",
                  "inputs": [
                {
                  "id": "optional_files",
                  "label": "Optional Files",
                  "kind": "file",
                  "required": false
                },
                    {
                      "id": "captcha_code",
                      "label": "Captcha Code",
                      "kind": "text",
                      "transient": true,
                      "required": false
                    }
              ],
                  "steps": [
                    {
                      "kind": "click",
                      "id": "optional-click",
                      "element": "form.optional_button",
                      "optional": true
                    },
                    {
                  "kind": "set_files",
                  "id": "optional-files",
                  "element": "form.file_input",
                  "input_ref": "optional_files"
                },
                    {
                      "kind": "solve_visual_captcha",
                      "id": "optional-captcha",
                      "image_element": "form.captcha_image",
                      "input_element": "form.captcha_input",
                  "input_id": "captcha_code",
                      "dispatch_events": ["input", "change"],
                      "optional": true
                    }
                  ]
                },
                {
                  "id": "double_captcha_flow",
                  "title": "Double Captcha Flow",
                  "purpose": "Exercise one-shot transient captcha inputs",
                  "start_page": "start",
                  "inputs": [
                    {
                      "id": "captcha_code",
                      "label": "Captcha Code",
                      "kind": "text",
                      "transient": true,
                      "required": false
                    }
                  ],
                  "steps": [
                    {
                      "kind": "solve_visual_captcha",
                      "id": "first-captcha",
                      "image_element": "form.captcha_image",
                      "input_element": "form.captcha_input",
                      "input_id": "captcha_code",
                      "dispatch_events": ["input", "change"],
                      "optional": true
                    },
                    {
                      "kind": "solve_visual_captcha",
                      "id": "second-captcha",
                      "image_element": "form.captcha_image",
                      "input_element": "form.captcha_input",
                      "input_id": "captcha_code",
                      "dispatch_events": ["input", "change"],
                      "optional": true
                    }
                  ]
                },
                {
                  "id": "visual_validation_flow",
                  "title": "Visual Validation Flow",
                  "purpose": "Exercise layout validation",
                  "start_page": "start",
                  "steps": [
                    {
                      "kind": "validate_visual_layout",
                      "id": "validate-layout",
                      "targets": [
                        {
                          "element": "form.name_input",
                          "role": "name_input"
                        },
                        {
                          "element": "form.submit_button",
                          "role": "submit_button"
                        }
                      ],
                      "instruction": "Confirm the form still looks like the expected start page.",
                      "relationship_rules": [
                        "The submit button should appear below or after the main input."
                      ],
                      "optional": false
                    }
                  ]
                }
              ]
            }"#,
        )
        .expect("session executor catalog should parse")
    }

    #[test]
    fn unicom_browser_flow_catalog_parses_and_validates() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");

        assert_eq!(catalog.provider, "unicom");
        assert_eq!(catalog.surface, "pan.wo.cn-web");
        assert_eq!(catalog.flows.len(), 14);
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_capture_current_session")
        );
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
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_sms_login_capture_send_code_before")
        );
        assert!(
            catalog
                .flows
                .iter()
                .any(|flow| flow.id == "unicom_sms_login_request_code_validate")
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
    fn browser_flow_catalog_rejects_prerequisite_cycles() {
        let raw = r#"{
          "schema_version": 1,
          "provider": "example",
          "surface": "example-web",
          "base_url": "https://example.com",
          "pages": [
            {
              "id": "root",
              "title": "Root",
              "url_patterns": ["https://example.com/*"]
            }
          ],
          "elements": [
            {
              "id": "root.button",
              "page": "root",
              "role": "button",
              "required": true,
              "selectors": [
                {
                  "engine": "css",
                  "value": "button"
                }
              ]
            }
          ],
          "requests": [],
          "operations": [],
          "flows": [
            {
              "id": "flow_a",
              "title": "Flow A",
              "purpose": "Cycle test",
              "start_page": "root",
              "prerequisite_flow_id": "flow_b",
              "steps": [
                {
                  "kind": "click",
                  "id": "click-a",
                  "element": "root.button"
                }
              ]
            },
            {
              "id": "flow_b",
              "title": "Flow B",
              "purpose": "Cycle test",
              "start_page": "root",
              "prerequisite_flow_id": "flow_a",
              "steps": [
                {
                  "kind": "click",
                  "id": "click-b",
                  "element": "root.button"
                }
              ]
            }
          ]
        }"#;

        let error = BrowserFlowCatalog::from_json_str(raw)
            .expect_err("catalog should reject prerequisite cycles");
        assert!(
            error
                .to_string()
                .contains("participates in a prerequisite cycle")
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
                    "local_file".to_string(),
                    Value::String("/tmp/example.txt".to_string()),
                ),
            ]),
            runtime: BTreeMap::from([(
                "access_token".to_string(),
                Value::String("token-300".to_string()),
            )]),
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
        assert!(plan.presets.is_empty());
        assert_eq!(
            plan.flow.prerequisite_flow_id.as_deref(),
            Some("unicom_prepare_personal_root_upload")
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
    fn browser_flow_catalog_bind_flow_upload_only_requires_access_token_runtime() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([(
                "local_file".to_string(),
                Value::String("/tmp/example.txt".to_string()),
            )]),
            runtime: BTreeMap::from([(
                "access_token".to_string(),
                Value::String("token-300".to_string()),
            )]),
        };

        let plan = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect("personal root upload should bind with access_token only");
        assert_eq!(plan.flow.id, "unicom_personal_root_upload");
    }

    #[test]
    fn browser_flow_catalog_bind_flow_prepare_upload_captures_runtime_outputs() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext::default();

        let plan = catalog
            .bind_flow("unicom_prepare_personal_root_upload", &context)
            .expect("prepare upload flow should bind");

        assert_eq!(plan.flow.outputs.len(), 3);
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "batch_no")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "directory_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "personal_space_type")
        );
        assert_eq!(
            plan.flow.prerequisite_flow_id.as_deref(),
            Some("unicom_capture_current_session")
        );
    }

    #[test]
    fn browser_flow_catalog_bind_flow_capture_current_session_exposes_runtime_outputs() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext::default();

        let plan = catalog
            .bind_flow("unicom_capture_current_session", &context)
            .expect("capture current session flow should bind");

        assert_eq!(plan.flow.outputs.len(), 8);
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "access_token")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "family_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "client_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "current_url")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "session_expires_at_unix_ms")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "session_timeout_ms")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "cookie_header")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "browser_profile_headers")
        );
    }

    #[test]
    fn browser_flow_catalog_bind_flow_validation_flow_uses_prerequisite_outputs() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([(
                "phone_number".to_string(),
                Value::String("18500001111".to_string()),
            )]),
            runtime: BTreeMap::new(),
        };

        let plan = catalog
            .bind_flow("unicom_sms_login_request_code_validate", &context)
            .expect("validation flow should bind");

        assert_eq!(plan.flow.id, "unicom_sms_login_request_code_validate");
        assert_eq!(
            plan.flow.prerequisite_flow_id.as_deref(),
            Some("unicom_sms_login_capture_send_code_before")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "send_code_text_after")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "send_code_countdown_detected")
        );
    }

    #[test]
    fn browser_flow_catalog_bind_flow_resume_submit_code_avoids_llm_steps() {
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
            ]),
            runtime: BTreeMap::new(),
        };

        let plan = catalog
            .bind_flow("unicom_sms_login_resume_submit_code", &context)
            .expect("resume submit code flow should bind");

        assert_eq!(plan.flow.id, "unicom_sms_login_resume_submit_code");
        assert!(plan.flow.steps.iter().all(|step| {
            !matches!(
                step,
                BrowserFlowStep::ValidateVisualLayout { .. }
                    | BrowserFlowStep::SolveVisualCaptcha { .. }
            )
        }));
    }

    #[test]
    fn telecom_browser_flow_catalog_binds_frame_aware_sms_request_flow() {
        let raw = include_str!("../../../config/browser-flows/telecom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("telecom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([(
                "phone_number".to_string(),
                Value::String("18900001111".to_string()),
            )]),
            runtime: BTreeMap::new(),
        };

        let plan = catalog
            .bind_flow("telecom_sms_login_request_code", &context)
            .expect("telecom sms request flow should bind");

        let phone_input = plan
            .find_element("login.phone_input")
            .expect("telecom phone input should be bound");
        assert_eq!(phone_input.frame.as_deref(), Some("name:udb_login"));
        assert_eq!(
            phone_input
                .selectors
                .first()
                .map(|selector| selector.value.as_str()),
            Some("#dynamicUserName")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "send_code_feedback_last_message"
                    && output.frame.as_deref() == Some("name:udb_login"))
        );
    }

    #[test]
    fn telecom_browser_flow_catalog_binds_resume_flow_capture_outputs() {
        let raw = include_str!("../../../config/browser-flows/telecom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("telecom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([
                (
                    "phone_number".to_string(),
                    Value::String("18900001111".to_string()),
                ),
                ("sms_code".to_string(), Value::String("123456".to_string())),
            ]),
            runtime: BTreeMap::new(),
        };

        let plan = catalog
            .bind_flow("telecom_sms_login_resume_submit_code", &context)
            .expect("telecom sms resume flow should bind");

        assert_eq!(
            plan.flow.expected_requests,
            vec!["telecom_list_files".to_string()]
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "browser_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "cookie_header")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "root_folder_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "browser_profile_headers")
        );
    }

    #[test]
    fn telecom_browser_flow_catalog_binds_upload_probe_flows() {
        let raw = include_str!("../../../config/browser-flows/telecom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("telecom browser flow catalog should parse and validate");

        let prepare_plan = catalog
            .bind_flow(
                "telecom_prepare_upload_probe",
                &BrowserFlowBindingContext::default(),
            )
            .expect("telecom prepare upload probe flow should bind");
        assert_eq!(
            prepare_plan.flow.prerequisite_flow_id.as_deref(),
            Some("telecom_capture_current_session")
        );
        assert!(
            prepare_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "install-upload-hooks")
        );
        assert!(
            prepare_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_summary")
        );
        assert!(
            prepare_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_surface_latest")
        );

        let capture_plan = catalog
            .bind_flow(
                "telecom_capture_upload_probe_state",
                &BrowserFlowBindingContext::default(),
            )
            .expect("telecom capture upload probe state flow should bind");
        assert!(
            capture_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_recent_events")
        );
        assert!(
            capture_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_last_candidate_event")
        );
    }

    #[test]
    fn mobile_browser_flow_catalog_binds_capture_current_session_outputs() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext::default();

        let plan = catalog
            .bind_flow("mobile_capture_current_session", &context)
            .expect("mobile capture current session flow should bind");

        assert_eq!(plan.flow.steps.len(), 9);
        assert!(
            plan.flow
                .steps
                .iter()
                .any(|step| step.id() == "guard-against-expired-or-login-page")
        );
        assert!(plan.flow.outputs.iter().any(|output| output.id == "token"));
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "root_folder_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "user_domain_id")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "browser_profile_headers")
        );
        let user_domain_output = plan
            .flow
            .outputs
            .iter()
            .find(|output| output.id == "user_domain_id")
            .expect("user_domain_id output should be present");
        assert_eq!(user_domain_output.fallback_sources.len(), 4);
    }

    #[test]
    fn mobile_browser_flow_catalog_binds_sms_login_flows() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");

        let request_plan = catalog
            .bind_flow(
                "mobile_sms_login_request_code_validate",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([(
                        "phone_number".to_string(),
                        Value::String("13800138000".to_string()),
                    )]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("mobile sms request-code flow should bind");
        assert!(
            request_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "resolve-expired-session-dialog")
        );
        assert!(
            request_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "send-code")
        );
        assert!(
            request_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "send_code_feedback_last_message")
        );
        assert!(
            request_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "send_code_countdown_detected")
        );

        let resume_plan = catalog
            .bind_flow(
                "mobile_sms_login_resume_submit_code",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([
                        (
                            "phone_number".to_string(),
                            Value::String("13800138000".to_string()),
                        ),
                        ("sms_code".to_string(), Value::String("123456".to_string())),
                    ]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("mobile sms resume flow should bind");
        assert!(
            resume_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "resolve-expired-session-dialog")
        );
        assert!(
            resume_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "submit-login")
        );
        assert!(
            resume_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "token")
        );
        assert!(
            resume_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "browser_profile_headers")
        );
    }

    #[test]
    fn mobile_browser_flow_catalog_uses_state_aware_agreement_toggle() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");

        let operation = catalog
            .operations
            .iter()
            .find(|operation| operation.id == "login.ensure_login_options_checked")
            .expect("mobile agreement operation should exist");

        assert!(operation.source.contains("checkFlag"));
        assert!(operation.source.contains("changeCheckFlag"));
        assert!(operation.source.contains("collectVueInstances"));
        assert!(operation.source.contains(".code-sms-check-img-wrap"));
        assert!(operation.source.contains("split(/[^a-z0-9_-]+/)"));
        assert!(!operation.source.contains("is-checked|on"));
    }

    #[test]
    fn mobile_browser_flow_catalog_accepts_index_as_logged_in_main_route() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");

        let main_page = catalog
            .pages
            .iter()
            .find(|page| page.id == "main")
            .expect("mobile main page should exist");
        assert!(
            main_page
                .url_patterns
                .iter()
                .any(|pattern| pattern == "https://yun.139.com/w/#/index")
        );
        assert!(
            main_page
                .url_patterns
                .iter()
                .any(|pattern| pattern == "https://yun.139.com/w/#/index*")
        );

        let capture_flow = catalog
            .flows
            .iter()
            .find(|flow| flow.id == "mobile_capture_current_session_from_logged_in_page")
            .expect("mobile logged-in capture flow should exist");
        let open_main_page_step = capture_flow
            .steps
            .iter()
            .find(|step| step.id() == "open-main-page")
            .expect("mobile capture flow should open the main page");
        let serialized_step =
            serde_json::to_string(open_main_page_step).expect("step should serialize");
        assert!(serialized_step.contains("https://yun.139.com/w/#/index"));
    }

    #[test]
    fn mobile_browser_flow_catalog_binds_upload_probe_flows() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");

        let prepare_plan = catalog
            .bind_flow(
                "mobile_prepare_upload_probe",
                &BrowserFlowBindingContext::default(),
            )
            .expect("mobile prepare upload probe flow should bind");
        assert_eq!(
            prepare_plan.flow.prerequisite_flow_id.as_deref(),
            Some("mobile_capture_current_session")
        );
        assert!(
            prepare_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "install-upload-hooks")
        );
        assert!(
            prepare_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_surface_after")
        );
        assert!(
            prepare_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_recent_events")
        );

        let attach_plan = catalog
            .bind_flow(
                "mobile_probe_upload_attach",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([(
                        "local_file".to_string(),
                        Value::String("/tmp/mobile-upload-probe.txt".to_string()),
                    )]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("mobile upload attach probe flow should bind");
        assert_eq!(
            attach_plan.flow.prerequisite_flow_id.as_deref(),
            Some("mobile_prepare_upload_probe")
        );
        assert!(
            attach_plan
                .flow
                .steps
                .iter()
                .any(|step| step.id() == "attach-local-file")
        );
        assert!(
            attach_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_last_candidate_event")
        );
        assert_eq!(
            attach_plan.input_value("local_file"),
            Some(&Value::String("/tmp/mobile-upload-probe.txt".to_string()))
        );

        let capture_plan = catalog
            .bind_flow(
                "mobile_capture_upload_probe_state",
                &BrowserFlowBindingContext::default(),
            )
            .expect("mobile capture upload probe state flow should bind");
        assert!(
            capture_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_probe_summary")
        );
        assert!(
            capture_plan
                .flow
                .outputs
                .iter()
                .any(|output| output.id == "upload_surface_latest")
        );
    }

    #[test]
    fn mobile_browser_flow_catalog_binds_personal_root_upload_flow() {
        let raw = include_str!("../../../config/browser-flows/mobile-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("mobile browser flow catalog should parse and validate");

        let plan = catalog
            .bind_flow(
                "mobile_personal_root_upload",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([(
                        "local_file".to_string(),
                        Value::String("/tmp/mobile-upload.txt".to_string()),
                    )]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("mobile personal root upload flow should bind");

        assert_eq!(
            plan.flow.prerequisite_flow_id.as_deref(),
            Some("mobile_capture_current_session")
        );
        assert!(
            plan.flow
                .steps
                .iter()
                .any(|step| step.id() == "attach-local-file")
        );
        assert!(
            plan.flow
                .steps
                .iter()
                .any(|step| step.id() == "wait-file-create-request")
        );
        assert!(
            plan.flow
                .outputs
                .iter()
                .any(|output| output.id == "uploaded_content_hash")
        );
    }

    #[test]
    fn dry_run_executor_reports_expected_upload_steps() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");
        let context = BrowserFlowBindingContext {
            inputs: BTreeMap::from([(
                "local_file".to_string(),
                Value::String("/tmp/example.txt".to_string()),
            )]),
            runtime: BTreeMap::from([(
                "access_token".to_string(),
                Value::String("token-300".to_string()),
            )]),
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
        assert_eq!(report.steps[0].step_id, "attach-local-file");
        assert_eq!(report.steps[0].step_kind, "set_files");
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Planned
        );
        assert_eq!(
            report.steps[0].detail.get("element"),
            Some(&Value::String(
                "file_list.global_uploader_input".to_string()
            ))
        );
        assert_eq!(
            report.steps[0].detail.get("input_present"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            report.steps[1].detail.get("events"),
            Some(&Value::Array(vec![
                Value::String("input".to_string()),
                Value::String("change".to_string())
            ]))
        );
        assert_eq!(
            report.steps[2].detail.get("request_method"),
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
        let upload_plan = catalog
            .bind_flow("unicom_personal_root_upload", &context)
            .expect("flow binding should succeed");
        let prepare_plan = catalog
            .bind_flow(
                "unicom_prepare_personal_root_upload",
                &BrowserFlowBindingContext::default(),
            )
            .expect("prepare flow binding should succeed");

        assert_eq!(
            upload_plan
                .find_request("upload2c")
                .map(|request| request.method.as_str()),
            Some("POST")
        );
        assert_eq!(
            prepare_plan
                .find_operation("file_list.open_personal_uploader")
                .map(|operation| operation.kind),
            Some(crate::BrowserFlowOperationKind::Javascript)
        );
        assert_eq!(
            upload_plan.input_value("local_file"),
            Some(&Value::String("/tmp/example.txt".to_string()))
        );
        assert!(upload_plan.find_request("missing").is_none());
        assert!(prepare_plan.find_operation("missing").is_none());
        assert!(upload_plan.input_value("missing").is_none());
    }

    #[test]
    fn session_executor_runs_bound_steps_in_order() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow(
                "complete_flow",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([
                        ("name".to_string(), Value::String("alice".to_string())),
                        (
                            "files".to_string(),
                            Value::Array(vec![
                                Value::String("/tmp/a.txt".to_string()),
                                Value::String("/tmp/b.txt".to_string()),
                            ]),
                        ),
                    ]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect("session execution should succeed");

        assert_eq!(report.mode, BrowserFlowExecutionMode::Session);
        assert_eq!(report.steps.len(), 9);
        assert!(
            report
                .steps
                .iter()
                .all(|step| step.status == BrowserFlowExecutionStepStatus::Succeeded)
        );
        assert_eq!(
            report.steps[2].detail.get("file_count"),
            Some(&Value::Number(serde_json::Number::from(2usize)))
        );
        assert_eq!(
            executor.session().actions(),
            vec![
                "navigate:https://example.com/start".to_string(),
                "wait_for_page:start:".to_string(),
                "set_input:form.name_input:alice:input,change".to_string(),
                "set_files:form.file_input:/tmp/a.txt|/tmp/b.txt".to_string(),
                "dispatch_events:form.file_input:input,change".to_string(),
                "invoke_operation:form.prepare_save".to_string(),
                "click:form.submit_button".to_string(),
                "wait_for_request:save_request:30000".to_string(),
                "wait_for_page:done:4000".to_string(),
                "wait:250".to_string(),
            ]
        );
    }

    #[test]
    fn session_executor_skips_optional_click_and_missing_file_input() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow("optional_flow", &BrowserFlowBindingContext::default())
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());
        executor
            .session()
            .mark_click_missing("form.optional_button");

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect("session execution should succeed");

        assert_eq!(report.steps.len(), 3);
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Skipped
        );
        assert_eq!(
            report.steps[0].detail.get("skip_reason"),
            Some(&Value::String("optional_element_not_found".to_string()))
        );
        assert_eq!(
            report.steps[1].status,
            BrowserFlowExecutionStepStatus::Skipped
        );
        assert_eq!(
            report.steps[1].detail.get("skip_reason"),
            Some(&Value::String("missing_input".to_string()))
        );
        assert_eq!(
            report.steps[2].status,
            BrowserFlowExecutionStepStatus::Skipped
        );
        assert_eq!(
            report.steps[2].detail.get("skip_reason"),
            Some(&Value::String("optional_captcha_not_found".to_string()))
        );
        assert!(executor.session().actions().is_empty());
    }

    #[test]
    fn session_executor_skips_optional_wait_for_request() {
        let catalog = BrowserFlowCatalog::from_json_str(
            r#"{
              "schema_version": 1,
              "provider": "example",
              "surface": "example-web",
              "base_url": "https://example.com",
              "pages": [
                { "id": "main", "title": "Main", "url_patterns": ["https://example.com/*"] }
              ],
              "elements": [
                {
                  "id": "main.page_root",
                  "page": "main",
                  "role": "component_root",
                  "required": false,
                  "selectors": [
                    { "engine": "javascript", "value": "document.body", "visible": false }
                  ]
                }
              ],
              "requests": [
                {
                  "id": "optional_request",
                  "method": "POST",
                  "url_pattern": "https://example.com/api/optional",
                  "success_codes": [200]
                }
              ],
              "flows": [
                {
                  "id": "flow",
                  "title": "Flow",
                  "purpose": "Test optional wait_for_request",
                  "start_page": "main",
                  "steps": [
                    {
                      "kind": "wait_for_request",
                      "id": "wait-optional",
                      "request": "optional_request",
                      "timeout_ms": 1000,
                      "optional": true
                    }
                  ]
                }
              ]
            }"#,
        )
        .expect("catalog should parse");
        let plan = catalog
            .bind_flow("flow", &BrowserFlowBindingContext::default())
            .expect("flow should bind");
        let session = RecordingBrowserFlowSession::default();
        session.mark_wait_request_missing("optional_request");
        let executor = BrowserFlowSessionExecutor::new(session);
        let runtime = Runtime::new().expect("tokio runtime should build");
        let report = runtime
            .block_on(executor.execute(&plan))
            .expect("optional wait_for_request should skip instead of failing");
        assert_eq!(report.steps.len(), 1);
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Skipped
        );
        assert_eq!(
            report.steps[0].detail.get("skip_reason"),
            Some(&Value::String("optional_request_not_observed".to_string()))
        );
    }

    #[test]
    fn session_executor_fills_manual_captcha_value() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow(
                "optional_flow",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([(
                        "captcha_code".to_string(),
                        Value::String("a1b2".to_string()),
                    )]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());
        executor
            .session()
            .mark_click_missing("form.optional_button");

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect("session execution should succeed");

        assert_eq!(
            report.steps[2].status,
            BrowserFlowExecutionStepStatus::Succeeded
        );
        assert_eq!(
            executor.session().actions(),
            vec!["solve_visual_captcha:form.captcha_image:form.captcha_input:a1b2".to_string()]
        );
    }

    #[test]
    fn session_executor_consumes_transient_manual_captcha_after_first_use() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow(
                "double_captcha_flow",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([(
                        "captcha_code".to_string(),
                        Value::String("a1b2".to_string()),
                    )]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect("session execution should succeed");

        assert_eq!(report.steps.len(), 2);
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Succeeded
        );
        assert_eq!(
            report.steps[1].status,
            BrowserFlowExecutionStepStatus::Skipped
        );
        assert_eq!(
            report.steps[1].detail.get("skip_reason"),
            Some(&Value::String("optional_captcha_not_found".to_string()))
        );
        assert_eq!(
            executor.session().actions(),
            vec!["solve_visual_captcha:form.captcha_image:form.captcha_input:a1b2".to_string()]
        );
    }

    #[test]
    fn session_executor_runs_visual_layout_validation() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow(
                "visual_validation_flow",
                &BrowserFlowBindingContext::default(),
            )
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());

        let report = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect("session execution should succeed");

        assert_eq!(report.steps.len(), 1);
        assert_eq!(
            report.steps[0].status,
            BrowserFlowExecutionStepStatus::Succeeded
        );
        assert_eq!(
            executor.session().actions(),
            vec!["validate_visual_layout:name_input=form.name_input|submit_button=form.submit_button".to_string()]
        );
    }

    #[test]
    fn session_executor_stops_after_session_error() {
        let catalog = session_executor_catalog();
        let plan = catalog
            .bind_flow(
                "complete_flow",
                &BrowserFlowBindingContext {
                    inputs: BTreeMap::from([
                        ("name".to_string(), Value::String("alice".to_string())),
                        ("files".to_string(), Value::String("/tmp/a.txt".to_string())),
                    ]),
                    runtime: BTreeMap::new(),
                },
            )
            .expect("flow binding should succeed");
        let executor = BrowserFlowSessionExecutor::new(RecordingBrowserFlowSession::default());
        executor.session().fail_operation("form.prepare_save");

        let error = Runtime::new()
            .expect("tokio runtime should build")
            .block_on(executor.execute(&plan))
            .expect_err("session execution should fail");

        assert!(matches!(error, BlobError::Upstream(_)));
        assert_eq!(
            executor.session().actions(),
            vec![
                "navigate:https://example.com/start".to_string(),
                "wait_for_page:start:".to_string(),
                "set_input:form.name_input:alice:input,change".to_string(),
                "set_files:form.file_input:/tmp/a.txt".to_string(),
                "dispatch_events:form.file_input:input,change".to_string(),
            ]
        );
    }
}
