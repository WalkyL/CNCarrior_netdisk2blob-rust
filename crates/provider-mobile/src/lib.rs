use async_trait::async_trait;
use blob_core::{
    BackendCapabilities, BlobBackend, BlobError, ContainerInfo, HealthStatus, ListObjectsRequest,
    ObjectInfo, ServiceHealth, TokenSource,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileConfig {
    pub base_url: String,
    pub token_source: TokenSource,
    pub cookie_header: Option<String>,
    pub user_agent: String,
    pub request_timeout_secs: u64,
}

pub struct MobileBlobAdapter {
    config: MobileConfig,
}

impl MobileBlobAdapter {
    pub fn new(config: MobileConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl BlobBackend for MobileBlobAdapter {
    fn name(&self) -> &'static str {
        "mobile-cloud-drive"
    }

    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            read: true,
            write: false,
            delete: false,
            multipart_upload: false,
        }
    }

    async fn health(&self) -> Result<ServiceHealth, BlobError> {
        let mut notes = vec![
            format!("base_url={}", self.config.base_url),
            format!("auth_source={}", self.config.token_source.describe()),
            "provider scaffold only; upstream endpoint mapping not implemented".to_string(),
            "browser-session interception is intentionally out of scope".to_string(),
        ];

        let status = match self.config.token_source.load() {
            Ok(_) => HealthStatus::Degraded,
            Err(error) => {
                notes.push(error.to_string());
                HealthStatus::Unavailable
            }
        };

        Ok(ServiceHealth {
            backend: self.name().to_string(),
            status,
            capabilities: self.capabilities(),
            notes,
        })
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, BlobError> {
        Err(BlobError::NotImplemented(
            "list_containers requires confirmed upstream API mapping".to_string(),
        ))
    }

    async fn list_objects(
        &self,
        _request: ListObjectsRequest,
    ) -> Result<Vec<ObjectInfo>, BlobError> {
        Err(BlobError::NotImplemented(
            "list_objects requires confirmed upstream API mapping".to_string(),
        ))
    }
}
