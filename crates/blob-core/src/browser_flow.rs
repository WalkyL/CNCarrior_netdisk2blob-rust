use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

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

#[cfg(test)]
mod tests {
    use super::BrowserFlowCatalog;

    #[test]
    fn unicom_browser_flow_catalog_parses_and_validates() {
        let raw = include_str!("../../../config/browser-flows/unicom-web.json");
        let catalog = BrowserFlowCatalog::from_json_str(raw)
            .expect("unicom browser flow catalog should parse and validate");

        assert_eq!(catalog.provider, "unicom");
        assert_eq!(catalog.surface, "pan.wo.cn-web");
        assert_eq!(catalog.flows.len(), 4);
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
}
