# Component Dependency Map

This file is generated from the Rust workspace using `cargo metadata` plus a `syn` AST walk.

Generator:
- `tools/component-ast-map`
- Regenerate with `cargo run --manifest-path tools/component-ast-map/Cargo.toml -- --workspace-root . --output docs/component-dependency-map.md --json-output docs/component-dependency-map.json`
- Workspace root: `carrier-cloud-blob-gateway`

## Workspace Crates

| Crate | Module Path Root | Manifest | Source Dir |
| --- | --- | --- | --- |
| `blob-core` | `blob_core` | `crates/blob-core/Cargo.toml` | `crates/blob-core/src` |
| `gatewayd` | `gatewayd` | `crates/gatewayd/Cargo.toml` | `crates/gatewayd/src` |
| `metadata-store` | `metadata_store` | `crates/metadata-store/Cargo.toml` | `crates/metadata-store/src` |
| `policy-engine` | `policy_engine` | `crates/policy-engine/Cargo.toml` | `crates/policy-engine/src` |
| `provider-mobile` | `provider_mobile` | `crates/provider-mobile/Cargo.toml` | `crates/provider-mobile/src` |
| `provider-onedrive` | `provider_onedrive` | `crates/provider-onedrive/Cargo.toml` | `crates/provider-onedrive/src` |
| `provider-telecom` | `provider_telecom` | `crates/provider-telecom/Cargo.toml` | `crates/provider-telecom/src` |
| `provider-unicom` | `provider_unicom` | `crates/provider-unicom/Cargo.toml` | `crates/provider-unicom/src` |
| `replication-engine` | `replication_engine` | `crates/replication-engine/Cargo.toml` | `crates/replication-engine/src` |

## Dependency Edges

| From | To | Declared in Cargo | Seen in AST | AST Nodes | Files | Example Symbols |
| --- | --- | --- | --- | ---: | ---: | --- |
| `gatewayd` | `blob-core` | yes | yes | 34 | 1 | blob_core::BlobBackend<br>blob_core::BlobError<br>blob_core::ListObjectsRequest<br>blob_core::ObjectInfo |
| `gatewayd` | `metadata-store` | yes | yes | 10 | 1 | metadata_store<br>metadata_store::MetadataRetentionPolicy<br>metadata_store::MetadataSnapshot<br>metadata_store::MetadataStore |
| `gatewayd` | `policy-engine` | yes | yes | 5 | 1 | policy_engine::ProviderId<br>policy_engine::ReplicationMode<br>policy_engine::TopologyInput<br>policy_engine::TopologyPolicy |
| `gatewayd` | `provider-mobile` | yes | yes | 2 | 1 | provider_mobile::MobileBlobAdapter<br>provider_mobile::MobileConfig |
| `gatewayd` | `provider-onedrive` | yes | yes | 7 | 1 | provider_onedrive::DEFAULT_ONEDRIVE_AUTH_BASE_URL<br>provider_onedrive::DEFAULT_ONEDRIVE_SCOPES<br>provider_onedrive::OneDriveBlobAdapter<br>provider_onedrive::OneDriveConfig |
| `gatewayd` | `provider-telecom` | yes | yes | 2 | 1 | provider_telecom::TelecomBlobAdapter<br>provider_telecom::TelecomConfig |
| `gatewayd` | `provider-unicom` | yes | yes | 2 | 1 | provider_unicom::UnicomBlobAdapter<br>provider_unicom::UnicomConfig |
| `gatewayd` | `replication-engine` | yes | yes | 12 | 1 | replication_engine::ReplicationEngine<br>replication_engine::ReplicationJob<br>replication_engine::ReplicationOperation<br>replication_engine::ReplicationOperation::Put |
| `metadata-store` | `replication-engine` | yes | yes | 8 | 1 | replication_engine::ReplicationJob<br>replication_engine::ReplicationObjectRef<br>replication_engine::ReplicationOperation<br>replication_engine::ReplicationStatus |
| `provider-mobile` | `blob-core` | yes | yes | 12 | 1 | blob_core::BackendCapabilities<br>blob_core::BlobBackend<br>blob_core::BlobError<br>blob_core::ContainerInfo |
| `provider-onedrive` | `blob-core` | yes | yes | 14 | 1 | blob_core::BackendCapabilities<br>blob_core::BlobBackend<br>blob_core::BlobError<br>blob_core::ContainerInfo |
| `provider-telecom` | `blob-core` | yes | yes | 18 | 1 | blob_core::BackendCapabilities<br>blob_core::BlobBackend<br>blob_core::BlobError<br>blob_core::ContainerInfo |
| `provider-unicom` | `blob-core` | yes | yes | 20 | 1 | blob_core::BackendCapabilities<br>blob_core::BlobBackend<br>blob_core::BlobError<br>blob_core::ContainerInfo |
| `replication-engine` | `policy-engine` | yes | yes | 5 | 1 | policy_engine::ProviderId<br>policy_engine::ReplicationMode<br>policy_engine::TopologyInput<br>policy_engine::TopologyPolicy |

## Reverse Dependencies

### `blob-core`

- `gatewayd`
- `provider-mobile`
- `provider-onedrive`
- `provider-telecom`
- `provider-unicom`

### `gatewayd`

- No workspace crate currently depends on this crate.

### `metadata-store`

- `gatewayd`

### `policy-engine`

- `gatewayd`
- `replication-engine`

### `provider-mobile`

- `gatewayd`

### `provider-onedrive`

- `gatewayd`

### `provider-telecom`

- `gatewayd`

### `provider-unicom`

- `gatewayd`

### `replication-engine`

- `gatewayd`
- `metadata-store`

## Notes

- `Declared in Cargo` comes from workspace package manifests.
- `Seen in AST` comes from source-level references such as `use foo::...`, type paths, expression paths, and macro paths.
- This map is intended to keep the gateway-lite data plane and the future auth-broker sidecar loosely coupled.
