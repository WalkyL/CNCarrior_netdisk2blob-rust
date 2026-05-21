use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::BlobError;

pub const PROVIDER_BRIDGE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBridgeCatalog {
    pub schema_version: u32,
    pub provider: String,
    pub surface: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub flow_aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub runtime_credential_mappings: Vec<ProviderBridgeCredentialMapping>,
    #[serde(default)]
    pub browser_profile: Option<ProviderBridgeBrowserProfileBinding>,
    #[serde(default)]
    pub logged_in_probes: Vec<ProviderBridgeLoggedInProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBridgeCredentialMapping {
    pub runtime_key: String,
    pub credential_key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBridgeBrowserProfileBinding {
    #[serde(default)]
    pub source_url_runtime_key: Option<String>,
    #[serde(default)]
    pub user_agent_runtime_key: Option<String>,
    #[serde(default)]
    pub headers_runtime_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderBridgeLoggedInProbe {
    pub id: String,
    pub flow_alias: String,
    #[serde(default)]
    pub target_selector: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBridgeCatalogDirectoryEntry {
    pub source_path: PathBuf,
    pub catalog: ProviderBridgeCatalog,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderBridgeCatalogCollection {
    entries: Vec<ProviderBridgeCatalogDirectoryEntry>,
}

impl ProviderBridgeCatalog {
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, BlobError> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|error| {
            BlobError::Configuration(format!(
                "failed to read provider bridge catalog {}: {error}",
                path.display()
            ))
        })?;
        Self::from_json_str(&raw, path.display().to_string().as_str())
    }

    pub fn from_json_str(raw: &str, source_label: &str) -> Result<Self, BlobError> {
        let catalog = serde_json::from_str::<Self>(raw).map_err(|error| {
            BlobError::Configuration(format!(
                "invalid provider bridge catalog {source_label}: {error}"
            ))
        })?;
        catalog.validate(source_label)?;
        Ok(catalog)
    }

    pub fn flow_alias(&self, alias: &str) -> Option<&str> {
        self.flow_aliases
            .get(alias.trim())
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    pub fn credential_mapping_for(&self, credential_key: &str) -> Option<&ProviderBridgeCredentialMapping> {
        let credential_key = credential_key.trim();
        self.runtime_credential_mappings
            .iter()
            .find(|mapping| mapping.credential_key == credential_key)
    }

    fn validate(&self, source_label: &str) -> Result<(), BlobError> {
        if self.schema_version != PROVIDER_BRIDGE_SCHEMA_VERSION {
            return Err(BlobError::Configuration(format!(
                "unsupported provider bridge catalog schema_version={} from {source_label}",
                self.schema_version
            )));
        }
        if self.provider.trim().is_empty() {
            return Err(BlobError::Configuration(format!(
                "provider bridge catalog {source_label} has an empty provider"
            )));
        }
        if self.surface.trim().is_empty() {
            return Err(BlobError::Configuration(format!(
                "provider bridge catalog {} has an empty surface",
                self.provider
            )));
        }

        for (alias, flow_id) in &self.flow_aliases {
            if alias.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty flow alias key",
                    self.provider
                )));
            }
            if flow_id.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty flow id for alias {}",
                    self.provider, alias
                )));
            }
        }

        let mut seen_credential_keys = BTreeSet::new();
        for mapping in &self.runtime_credential_mappings {
            if mapping.runtime_key.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty runtime_key",
                    self.provider
                )));
            }
            if mapping.credential_key.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty credential_key",
                    self.provider
                )));
            }
            if !seen_credential_keys.insert(mapping.credential_key.clone()) {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains duplicate credential mapping for {}",
                    self.provider, mapping.credential_key
                )));
            }
        }

        if let Some(binding) = self.browser_profile.as_ref() {
            for (label, value) in [
                ("source_url_runtime_key", binding.source_url_runtime_key.as_deref()),
                ("user_agent_runtime_key", binding.user_agent_runtime_key.as_deref()),
                ("headers_runtime_key", binding.headers_runtime_key.as_deref()),
            ] {
                if matches!(value, Some(raw) if raw.trim().is_empty()) {
                    return Err(BlobError::Configuration(format!(
                        "provider bridge catalog {} contains an empty browser_profile.{}",
                        self.provider, label
                    )));
                }
            }
        }

        let mut seen_probe_ids = BTreeSet::new();
        for probe in &self.logged_in_probes {
            if probe.id.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty logged_in_probes[].id",
                    self.provider
                )));
            }
            if probe.flow_alias.trim().is_empty() {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty flow_alias for probe {}",
                    self.provider, probe.id
                )));
            }
            if !seen_probe_ids.insert(probe.id.clone()) {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains duplicate logged_in probe id {}",
                    self.provider, probe.id
                )));
            }
            if matches!(probe.target_selector.as_deref(), Some(raw) if raw.trim().is_empty()) {
                return Err(BlobError::Configuration(format!(
                    "provider bridge catalog {} contains an empty target_selector for probe {}",
                    self.provider, probe.id
                )));
            }
        }

        Ok(())
    }
}

impl ProviderBridgeCatalogCollection {
    pub fn from_json_dir(dir: impl AsRef<Path>) -> Result<Self, BlobError> {
        let dir = dir.as_ref();
        let mut json_paths = fs::read_dir(dir)
            .map_err(|error| {
                BlobError::Configuration(format!(
                    "failed to read provider bridge catalog directory {}: {error}",
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
        let mut seen_providers = BTreeSet::new();
        for path in json_paths {
            let catalog = ProviderBridgeCatalog::from_json_file(&path)?;
            if !seen_providers.insert(catalog.provider.clone()) {
                return Err(BlobError::Configuration(format!(
                    "duplicate provider bridge catalog detected for {}",
                    catalog.provider
                )));
            }
            entries.push(ProviderBridgeCatalogDirectoryEntry {
                source_path: path,
                catalog,
            });
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[ProviderBridgeCatalogDirectoryEntry] {
        &self.entries
    }

    pub fn get(&self, provider: &str) -> Option<&ProviderBridgeCatalog> {
        self.entries
            .iter()
            .find(|entry| entry.catalog.provider == provider)
            .map(|entry| &entry.catalog)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{
        PROVIDER_BRIDGE_SCHEMA_VERSION, ProviderBridgeCatalog, ProviderBridgeCatalogCollection,
    };

    #[test]
    fn provider_bridge_catalog_collection_loads_directory() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("mobile.json");
        fs::write(
            &path,
            r#"{
              "schema_version": 1,
              "provider": "mobile",
              "surface": "yun.139.com-web",
              "flow_aliases": {
                "capture_session": "mobile_capture_current_session"
              },
              "runtime_credential_mappings": [
                { "runtime_key": "token", "credential_key": "token" }
              ],
              "logged_in_probes": [
                { "id": "main", "flow_alias": "capture_session" }
              ]
            }"#,
        )
        .expect("write catalog");

        let collection =
            ProviderBridgeCatalogCollection::from_json_dir(dir.path()).expect("load collection");
        let catalog = collection.get("mobile").expect("mobile catalog");
        assert_eq!(catalog.surface, "yun.139.com-web");
        assert_eq!(
            catalog.flow_alias("capture_session"),
            Some("mobile_capture_current_session")
        );
    }

    #[test]
    fn provider_bridge_catalog_rejects_duplicate_credential_keys() {
        let error = ProviderBridgeCatalog::from_json_str(
            &format!(
                r#"{{
                  "schema_version": {PROVIDER_BRIDGE_SCHEMA_VERSION},
                  "provider": "mobile",
                  "surface": "yun.139.com-web",
                  "runtime_credential_mappings": [
                    {{ "runtime_key": "token", "credential_key": "token" }},
                    {{ "runtime_key": "access_token", "credential_key": "token" }}
                  ]
                }}"#
            ),
            "inline",
        )
        .expect_err("duplicate credential key should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate credential mapping for token"),
            "unexpected error: {error}"
        );
    }
}
