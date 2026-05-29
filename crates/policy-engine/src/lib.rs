// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Stub,
    Unicom,
    Telecom,
    Mobile,
    Onedrive,
}

impl ProviderId {
    pub fn parse(value: &str) -> Result<Self, PolicyError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stub" => Ok(Self::Stub),
            "unicom" => Ok(Self::Unicom),
            "telecom" => Ok(Self::Telecom),
            "mobile" => Ok(Self::Mobile),
            "onedrive" => Ok(Self::Onedrive),
            other => Err(PolicyError::UnknownProvider(other.to_string())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stub => "stub",
            Self::Unicom => "unicom",
            Self::Telecom => "telecom",
            Self::Mobile => "mobile",
            Self::Onedrive => "onedrive",
        }
    }

    pub fn can_be_primary(self) -> bool {
        matches!(
            self,
            Self::Stub | Self::Unicom | Self::Telecom | Self::Mobile
        )
    }

    pub fn can_be_sync_target(self) -> bool {
        matches!(
            self,
            Self::Unicom | Self::Telecom | Self::Mobile | Self::Onedrive
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationMode {
    AsyncBackup,
}

impl ReplicationMode {
    pub fn parse(value: &str) -> Result<Self, PolicyError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "async_backup" => Ok(Self::AsyncBackup),
            other => Err(PolicyError::UnsupportedReplicationMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TopologyInput {
    pub primary_provider: ProviderId,
    pub sync_targets: Vec<ProviderId>,
    pub fallback_read_order: Vec<ProviderId>,
    pub onedrive_enabled: bool,
    pub replication_mode: ReplicationMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyPolicy {
    pub primary_provider: ProviderId,
    pub sync_targets: Vec<ProviderId>,
    pub fallback_read_order: Vec<ProviderId>,
    pub onedrive_enabled: bool,
    pub replication_mode: ReplicationMode,
}

impl TopologyPolicy {
    pub fn from_input(input: TopologyInput) -> Result<Self, PolicyError> {
        if !input.primary_provider.can_be_primary() {
            return Err(PolicyError::InvalidPrimaryProvider(
                input.primary_provider.as_str().to_string(),
            ));
        }

        let mut sync_targets = dedup_preserve_order(input.sync_targets);
        let fallback_read_order = dedup_preserve_order(input.fallback_read_order);

        for provider in &sync_targets {
            if !provider.can_be_sync_target() {
                return Err(PolicyError::InvalidSyncTarget(
                    provider.as_str().to_string(),
                ));
            }
            if *provider == input.primary_provider {
                return Err(PolicyError::PrimaryProviderCannotBeSyncTarget(
                    provider.as_str().to_string(),
                ));
            }
            if *provider == ProviderId::Onedrive && !input.onedrive_enabled {
                return Err(PolicyError::OnedriveDisabledButConfigured);
            }
        }

        if sync_targets.is_empty() && input.onedrive_enabled {
            sync_targets.push(ProviderId::Onedrive);
        }

        for provider in &fallback_read_order {
            if *provider == input.primary_provider {
                return Err(PolicyError::PrimaryProviderCannotBeFallback(
                    provider.as_str().to_string(),
                ));
            }
            if !sync_targets.contains(provider) {
                return Err(PolicyError::FallbackProviderMustBeSyncTarget(
                    provider.as_str().to_string(),
                ));
            }
        }

        Ok(Self {
            primary_provider: input.primary_provider,
            sync_targets,
            fallback_read_order,
            onedrive_enabled: input.onedrive_enabled,
            replication_mode: input.replication_mode,
        })
    }

    pub fn primary_provider_name(&self) -> &'static str {
        self.primary_provider.as_str()
    }

    pub fn sync_target_names(&self) -> Vec<&'static str> {
        self.sync_targets
            .iter()
            .map(|provider| provider.as_str())
            .collect()
    }

    pub fn fallback_read_order_names(&self) -> Vec<&'static str> {
        self.fallback_read_order
            .iter()
            .map(|provider| provider.as_str())
            .collect()
    }
}

pub fn parse_provider_list(value: &str) -> Result<Vec<ProviderId>, PolicyError> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ProviderId::parse)
        .collect()
}

fn dedup_preserve_order(providers: Vec<ProviderId>) -> Vec<ProviderId> {
    let mut unique = Vec::with_capacity(providers.len());

    for provider in providers {
        if !unique.contains(&provider) {
            unique.push(provider);
        }
    }

    unique
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("invalid primary provider: {0}")]
    InvalidPrimaryProvider(String),
    #[error("invalid sync target: {0}")]
    InvalidSyncTarget(String),
    #[error("primary provider cannot also be a sync target: {0}")]
    PrimaryProviderCannotBeSyncTarget(String),
    #[error("primary provider cannot appear in fallback read order: {0}")]
    PrimaryProviderCannotBeFallback(String),
    #[error("fallback provider must also be configured as a sync target: {0}")]
    FallbackProviderMustBeSyncTarget(String),
    #[error("onedrive is disabled but configured as a sync target")]
    OnedriveDisabledButConfigured,
    #[error("unsupported replication mode: {0}")]
    UnsupportedReplicationMode(String),
}

#[cfg(test)]
mod tests {
    use super::{ProviderId, ReplicationMode, TopologyInput, TopologyPolicy, parse_provider_list};

    #[test]
    fn parse_provider_list_ignores_empty_entries() {
        let providers =
            parse_provider_list("telecom, onedrive, ,mobile").expect("provider list should parse");

        assert_eq!(
            providers,
            vec![
                ProviderId::Telecom,
                ProviderId::Onedrive,
                ProviderId::Mobile
            ]
        );
    }

    #[test]
    fn defaults_add_onedrive_sync_target_when_enabled() {
        let topology = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Unicom,
            sync_targets: Vec::new(),
            fallback_read_order: Vec::new(),
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect("topology should validate");

        assert_eq!(topology.sync_targets, vec![ProviderId::Onedrive]);
        assert!(topology.fallback_read_order.is_empty());
    }

    #[test]
    fn fallback_can_be_empty_even_with_sync_targets() {
        let topology = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Telecom,
            sync_targets: vec![ProviderId::Mobile, ProviderId::Onedrive],
            fallback_read_order: Vec::new(),
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect("topology should validate");

        assert_eq!(
            topology.sync_targets,
            vec![ProviderId::Mobile, ProviderId::Onedrive]
        );
        assert!(topology.fallback_read_order.is_empty());
    }

    #[test]
    fn primary_cannot_be_sync_target() {
        let error = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Telecom,
            sync_targets: vec![ProviderId::Telecom],
            fallback_read_order: Vec::new(),
            onedrive_enabled: false,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect_err("invalid topology should fail");

        assert_eq!(
            error,
            super::PolicyError::PrimaryProviderCannotBeSyncTarget("telecom".to_string())
        );
    }

    #[test]
    fn fallback_must_be_subset_of_sync_targets() {
        let error = TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Mobile,
            sync_targets: vec![ProviderId::Onedrive],
            fallback_read_order: vec![ProviderId::Telecom],
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect_err("invalid fallback should fail");

        assert_eq!(
            error,
            super::PolicyError::FallbackProviderMustBeSyncTarget("telecom".to_string())
        );
    }
}
