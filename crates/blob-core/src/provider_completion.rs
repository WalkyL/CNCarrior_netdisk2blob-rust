// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use serde::{Deserialize, Serialize};

use crate::{BackendCapabilities, HealthStatus, ServiceHealth, StorageScopeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionDimensionStatus {
    Planned,
    Partial,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionOverallStatus {
    Planned,
    Partial,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCompletionExpectation {
    pub auth_session: CompletionDimensionStatus,
    pub scope_discovery: CompletionDimensionStatus,
    pub native_read_path: CompletionDimensionStatus,
    pub native_write_path: CompletionDimensionStatus,
    pub object_actions: CompletionDimensionStatus,
    pub health_catalog_docs: CompletionDimensionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCompletionObserved {
    pub health: ServiceHealth,
    pub capabilities: BackendCapabilities,
    pub auth_material_confirmed: bool,
    pub native_read_roundtrip: bool,
    pub native_write_roundtrip: bool,
    pub create_directory_supported: bool,
    pub writable_scope_coverage: CompletionDimensionStatus,
    pub supports_rename: bool,
    pub supports_copy: bool,
    pub supports_move: bool,
    pub probe_catalog_confirmed: bool,
    pub capability_catalog_present: bool,
    pub browser_flow_catalog_present: bool,
    pub docs_synced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionDimensionReport {
    pub expected: CompletionDimensionStatus,
    pub observed: CompletionDimensionStatus,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCompletionReport {
    pub provider: String,
    pub overall_expected: CompletionOverallStatus,
    pub overall_observed: CompletionOverallStatus,
    pub coverage_total: u8,
    pub coverage_full: u8,
    pub coverage_partial_or_full: u8,
    pub auth_session: CompletionDimensionReport,
    pub scope_discovery: CompletionDimensionReport,
    pub native_read_path: CompletionDimensionReport,
    pub native_write_path: CompletionDimensionReport,
    pub object_actions: CompletionDimensionReport,
    pub health_catalog_docs: CompletionDimensionReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCompletionAssertions {
    pub strict: bool,
}

pub trait ProviderCompletionFixture {
    fn provider(&self) -> &str;
    fn expected(&self) -> &ProviderCompletionExpectation;
    fn observed(&self) -> &ProviderCompletionObserved;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCompletionCase {
    pub provider: String,
    pub expected: ProviderCompletionExpectation,
    pub observed: ProviderCompletionObserved,
}

impl ProviderCompletionFixture for ProviderCompletionCase {
    fn provider(&self) -> &str {
        self.provider.as_str()
    }

    fn expected(&self) -> &ProviderCompletionExpectation {
        &self.expected
    }

    fn observed(&self) -> &ProviderCompletionObserved {
        &self.observed
    }
}

pub fn assert_provider_completion_fixture<T>(
    fixture: &T,
    assertions: ProviderCompletionAssertions,
) -> ProviderCompletionReport
where
    T: ProviderCompletionFixture + ?Sized,
{
    assert_provider_completion(
        fixture.provider(),
        fixture.expected().clone(),
        fixture.observed().clone(),
        assertions,
    )
}

pub fn assert_provider_completion(
    provider: &str,
    expected: ProviderCompletionExpectation,
    observed: ProviderCompletionObserved,
    assertions: ProviderCompletionAssertions,
) -> ProviderCompletionReport {
    let report = build_report(provider, expected, observed);
    if assertions.strict {
        assert_eq!(
            report.overall_observed, report.overall_expected,
            "provider completion mismatch for {}: expected={:?} observed={:?}",
            report.provider, report.overall_expected, report.overall_observed
        );
        assert_eq!(
            report.coverage_partial_or_full, report.coverage_total,
            "provider completion dimensions are missing for {}",
            report.provider
        );
    }
    report
}

fn build_report(
    provider: &str,
    expected: ProviderCompletionExpectation,
    observed: ProviderCompletionObserved,
) -> ProviderCompletionReport {
    let auth_session = evaluate_auth_session(&expected, &observed);
    let scope_discovery = evaluate_scope_discovery(&expected, &observed);
    let native_read_path = evaluate_native_read_path(&expected, &observed);
    let native_write_path = evaluate_native_write_path(&expected, &observed);
    let object_actions = evaluate_object_actions(&expected, &observed);
    let health_catalog_docs = evaluate_health_catalog_docs(&expected, &observed);

    let dimensions = [
        &auth_session,
        &scope_discovery,
        &native_read_path,
        &native_write_path,
        &object_actions,
        &health_catalog_docs,
    ];
    let coverage_total = dimensions.len() as u8;
    let coverage_full = dimensions
        .iter()
        .filter(|item| item.observed == CompletionDimensionStatus::Full)
        .count() as u8;
    let coverage_partial_or_full = dimensions
        .iter()
        .filter(|item| item.observed != CompletionDimensionStatus::Planned)
        .count() as u8;
    let overall_expected = overall_from_dimension_statuses([
        expected.auth_session,
        expected.scope_discovery,
        expected.native_read_path,
        expected.native_write_path,
        expected.object_actions,
        expected.health_catalog_docs,
    ]);
    let overall_observed = overall_from_dimension_statuses([
        auth_session.observed,
        scope_discovery.observed,
        native_read_path.observed,
        native_write_path.observed,
        object_actions.observed,
        health_catalog_docs.observed,
    ]);

    ProviderCompletionReport {
        provider: provider.to_string(),
        overall_expected,
        overall_observed,
        coverage_total,
        coverage_full,
        coverage_partial_or_full,
        auth_session,
        scope_discovery,
        native_read_path,
        native_write_path,
        object_actions,
        health_catalog_docs,
    }
}

fn overall_from_dimension_statuses(
    dimensions: [CompletionDimensionStatus; 6],
) -> CompletionOverallStatus {
    if dimensions
        .iter()
        .all(|status| *status == CompletionDimensionStatus::Full)
    {
        CompletionOverallStatus::Full
    } else if dimensions
        .iter()
        .any(|status| *status == CompletionDimensionStatus::Partial)
        || dimensions
            .iter()
            .any(|status| *status == CompletionDimensionStatus::Full)
    {
        CompletionOverallStatus::Partial
    } else {
        CompletionOverallStatus::Planned
    }
}

fn evaluate_auth_session(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let has_auth_note = observed
        .health
        .notes
        .iter()
        .any(|note| note.contains("auth_source=") || note.contains("download_token_present="));
    let observed_status = if observed.auth_material_confirmed && has_auth_note {
        CompletionDimensionStatus::Full
    } else if observed.auth_material_confirmed || has_auth_note {
        notes.push("auth evidence is partial".to_string());
        CompletionDimensionStatus::Partial
    } else {
        notes.push("missing auth evidence in health notes".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.auth_session,
        observed: observed_status,
        notes,
    }
}

fn evaluate_scope_discovery(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let has_personal = observed
        .health
        .scopes
        .iter()
        .any(|scope| scope.kind == StorageScopeKind::Personal);
    let has_family = observed
        .health
        .scopes
        .iter()
        .any(|scope| scope.kind == StorageScopeKind::Family);
    let observed_status = if has_personal && has_family {
        CompletionDimensionStatus::Full
    } else if has_personal {
        notes.push("family scope not discovered".to_string());
        CompletionDimensionStatus::Partial
    } else {
        notes.push("personal scope not discovered".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.scope_discovery,
        observed: observed_status,
        notes,
    }
}

fn evaluate_native_read_path(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let observed_status = if observed.capabilities.read
        && observed.capabilities.streaming_get
        && observed.native_read_roundtrip
    {
        CompletionDimensionStatus::Full
    } else if observed.capabilities.read {
        if !observed.capabilities.streaming_get {
            notes.push("streaming_get capability is disabled".to_string());
        }
        if !observed.native_read_roundtrip {
            notes.push("native read roundtrip evidence is missing".to_string());
        }
        CompletionDimensionStatus::Partial
    } else {
        notes.push("read capability is disabled".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.native_read_path,
        observed: observed_status,
        notes,
    }
}

fn evaluate_native_write_path(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let observed_status = if observed.capabilities.write
        && observed.capabilities.delete
        && observed.capabilities.streaming_put
        && observed.native_write_roundtrip
        && observed.create_directory_supported
        && observed.writable_scope_coverage == CompletionDimensionStatus::Full
    {
        CompletionDimensionStatus::Full
    } else if observed.capabilities.write {
        if !observed.capabilities.delete {
            notes.push("delete capability is disabled".to_string());
        }
        if !observed.capabilities.streaming_put {
            notes.push("streaming_put capability is disabled".to_string());
        }
        if !observed.native_write_roundtrip {
            notes.push("native write roundtrip evidence is missing".to_string());
        }
        if !observed.create_directory_supported {
            notes.push("create_directory support is missing".to_string());
        }
        if observed.writable_scope_coverage != CompletionDimensionStatus::Full {
            notes.push(format!(
                "writable scope coverage is {:?}",
                observed.writable_scope_coverage
            ));
        }
        CompletionDimensionStatus::Partial
    } else {
        notes.push("write capability is disabled".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.native_write_path,
        observed: observed_status,
        notes,
    }
}

fn evaluate_object_actions(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let supported = [
        observed.supports_rename,
        observed.supports_copy,
        observed.supports_move,
    ]
    .into_iter()
    .filter(|supported| *supported)
    .count();
    let observed_status = if supported == 3 {
        CompletionDimensionStatus::Full
    } else if supported > 0 {
        notes.push(format!("partial object action support: {supported}/3"));
        CompletionDimensionStatus::Partial
    } else {
        notes.push("rename/copy/move are not available".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.object_actions,
        observed: observed_status,
        notes,
    }
}

fn evaluate_health_catalog_docs(
    expected: &ProviderCompletionExpectation,
    observed: &ProviderCompletionObserved,
) -> CompletionDimensionReport {
    let mut notes = Vec::new();
    let status_is_ok = matches!(
        observed.health.status,
        HealthStatus::Healthy | HealthStatus::Degraded
    );
    let present_count = [
        observed.probe_catalog_confirmed,
        observed.capability_catalog_present,
        observed.browser_flow_catalog_present,
        observed.docs_synced,
        status_is_ok,
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    let observed_status = if present_count == 5 {
        CompletionDimensionStatus::Full
    } else if present_count >= 3 {
        notes.push(format!(
            "health/catalog/docs evidence is partial: {present_count}/5"
        ));
        CompletionDimensionStatus::Partial
    } else {
        notes.push("health/catalog/docs evidence is missing".to_string());
        CompletionDimensionStatus::Planned
    };
    CompletionDimensionReport {
        expected: expected.health_catalog_docs,
        observed: observed_status,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        BackendCapabilities, HealthStatus, ServiceHealth, StorageScopeHealth, StorageScopeKind,
    };

    use super::{
        CompletionDimensionStatus, CompletionOverallStatus, ProviderCompletionAssertions,
        ProviderCompletionExpectation, ProviderCompletionObserved, assert_provider_completion,
    };

    #[test]
    fn provider_completion_fixture_marks_full_when_all_dimensions_are_full() {
        let report = assert_provider_completion(
            "unicom",
            ProviderCompletionExpectation {
                auth_session: CompletionDimensionStatus::Full,
                scope_discovery: CompletionDimensionStatus::Full,
                native_read_path: CompletionDimensionStatus::Full,
                native_write_path: CompletionDimensionStatus::Full,
                object_actions: CompletionDimensionStatus::Full,
                health_catalog_docs: CompletionDimensionStatus::Full,
            },
            ProviderCompletionObserved {
                health: ServiceHealth {
                    backend: "unicom".to_string(),
                    status: HealthStatus::Healthy,
                    capabilities: BackendCapabilities {
                        read: true,
                        write: true,
                        delete: true,
                        multipart_upload: false,
                        streaming_get: true,
                        streaming_put: true,
                        max_single_upload_bytes: None,
                        max_single_download_bytes: None,
                        upload_part_size_bytes: Some(1),
                    },
                    scopes: vec![
                        StorageScopeHealth {
                            id: "personal".to_string(),
                            label: "Personal".to_string(),
                            kind: StorageScopeKind::Personal,
                            writable: true,
                            root: Some("/".to_string()),
                            container: Some("root".to_string()),
                            object_count: Some(1),
                            capacity: None,
                            notes: Vec::new(),
                        },
                        StorageScopeHealth {
                            id: "family".to_string(),
                            label: "Family".to_string(),
                            kind: StorageScopeKind::Family,
                            writable: true,
                            root: Some("/".to_string()),
                            container: Some("family".to_string()),
                            object_count: Some(1),
                            capacity: None,
                            notes: Vec::new(),
                        },
                    ],
                    notes: vec!["auth_source=static".to_string()],
                },
                capabilities: BackendCapabilities {
                    read: true,
                    write: true,
                    delete: true,
                    multipart_upload: false,
                    streaming_get: true,
                    streaming_put: true,
                    max_single_upload_bytes: None,
                    max_single_download_bytes: None,
                    upload_part_size_bytes: Some(1),
                },
                auth_material_confirmed: true,
                native_read_roundtrip: true,
                native_write_roundtrip: true,
                create_directory_supported: true,
                writable_scope_coverage: CompletionDimensionStatus::Full,
                supports_rename: true,
                supports_copy: true,
                supports_move: true,
                probe_catalog_confirmed: true,
                capability_catalog_present: true,
                browser_flow_catalog_present: true,
                docs_synced: true,
            },
            ProviderCompletionAssertions { strict: true },
        );

        assert_eq!(report.overall_observed, CompletionOverallStatus::Full);
        assert_eq!(report.coverage_full, 6);
        let json = serde_json::to_value(&report).expect("report should serialize");
        assert_eq!(json["provider"], "unicom");
        assert_eq!(json["coverage_total"], 6);
        assert!(json.get("native_write_path").is_some());
    }
}
