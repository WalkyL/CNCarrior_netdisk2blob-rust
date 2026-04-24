use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use policy_engine::TopologyPolicy;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationOperation {
    Put,
    Delete,
}

impl ReplicationOperation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Put => "put",
            Self::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ReplicationParseError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "put" => Ok(Self::Put),
            "delete" => Ok(Self::Delete),
            other => Err(ReplicationParseError::UnknownOperation(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationStatus {
    Pending,
    Completed,
    Failed,
}

impl ReplicationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ReplicationParseError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(ReplicationParseError::UnknownStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationObjectRef {
    pub bucket: String,
    pub key: String,
    pub etag: Option<String>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationJob {
    pub job_id: u64,
    pub target: String,
    pub operation: ReplicationOperation,
    pub object: ReplicationObjectRef,
    pub status: ReplicationStatus,
    pub attempts: u32,
    pub enqueued_at_unix_ms: u128,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationSnapshot {
    pub pending_count: usize,
    pub recent_count: usize,
    pub pending_jobs: Vec<ReplicationJob>,
    pub recent_jobs: Vec<ReplicationJob>,
}

pub struct ReplicationEngine {
    next_job_id: AtomicU64,
    pending_jobs: Mutex<VecDeque<ReplicationJob>>,
    recent_jobs: Mutex<Vec<ReplicationJob>>,
    recent_limit: usize,
}

impl ReplicationEngine {
    pub fn new() -> Self {
        Self::with_recent_limit(64)
    }

    pub fn with_recent_limit(recent_limit: usize) -> Self {
        Self {
            next_job_id: AtomicU64::new(1),
            pending_jobs: Mutex::new(VecDeque::new()),
            recent_jobs: Mutex::new(Vec::new()),
            recent_limit,
        }
    }

    pub fn enqueue_put(
        &self,
        topology: &TopologyPolicy,
        bucket: impl Into<String>,
        key: impl Into<String>,
        etag: Option<String>,
        size: u64,
        content_type: Option<String>,
    ) -> Vec<ReplicationJob> {
        self.enqueue_jobs(
            topology,
            ReplicationOperation::Put,
            ReplicationObjectRef {
                bucket: bucket.into(),
                key: key.into(),
                etag,
                size: Some(size),
                content_type,
            },
        )
    }

    pub fn enqueue_delete(
        &self,
        topology: &TopologyPolicy,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> Vec<ReplicationJob> {
        self.enqueue_jobs(
            topology,
            ReplicationOperation::Delete,
            ReplicationObjectRef {
                bucket: bucket.into(),
                key: key.into(),
                etag: None,
                size: None,
                content_type: None,
            },
        )
    }

    pub fn pending_jobs(&self) -> Vec<ReplicationJob> {
        let pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.iter().cloned().collect()
    }

    pub fn pop_next(&self) -> Option<ReplicationJob> {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.pop_front()
    }

    pub fn record_completed(&self, mut job: ReplicationJob) {
        job.status = ReplicationStatus::Completed;
        self.push_recent(job);
    }

    pub fn record_failed(&self, mut job: ReplicationJob, error: impl Into<String>) {
        job.status = ReplicationStatus::Failed;
        job.last_error = Some(error.into());
        self.push_recent(job);
    }

    pub fn snapshot(&self) -> ReplicationSnapshot {
        let pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        let recent = self.recent_jobs.lock().expect("replication queue poisoned");

        ReplicationSnapshot {
            pending_count: pending.len(),
            recent_count: recent.len(),
            pending_jobs: pending.iter().cloned().collect(),
            recent_jobs: recent.clone(),
        }
    }

    pub fn restore_pending(&self, jobs: Vec<ReplicationJob>) {
        if jobs.is_empty() {
            return;
        }

        let next_job_id = jobs
            .iter()
            .map(|job| job.job_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        self.next_job_id.fetch_max(next_job_id, Ordering::Relaxed);

        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.extend(jobs);
    }

    fn enqueue_jobs(
        &self,
        topology: &TopologyPolicy,
        operation: ReplicationOperation,
        object: ReplicationObjectRef,
    ) -> Vec<ReplicationJob> {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        let mut created = Vec::with_capacity(topology.sync_targets.len());

        for target in &topology.sync_targets {
            let job = ReplicationJob {
                job_id: self.next_job_id.fetch_add(1, Ordering::Relaxed),
                target: target.as_str().to_string(),
                operation: operation.clone(),
                object: object.clone(),
                status: ReplicationStatus::Pending,
                attempts: 0,
                enqueued_at_unix_ms: now_unix_ms(),
                last_error: None,
            };
            pending.push_back(job.clone());
            created.push(job);
        }

        created
    }

    fn push_recent(&self, job: ReplicationJob) {
        let mut recent = self.recent_jobs.lock().expect("replication queue poisoned");
        recent.push(job);

        if recent.len() > self.recent_limit {
            let overflow = recent.len() - self.recent_limit;
            recent.drain(0..overflow);
        }
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_millis()
}

#[derive(Debug, thiserror::Error)]
pub enum ReplicationParseError {
    #[error("unknown replication operation: {0}")]
    UnknownOperation(String),
    #[error("unknown replication status: {0}")]
    UnknownStatus(String),
}

#[cfg(test)]
mod tests {
    use policy_engine::{ProviderId, ReplicationMode, TopologyInput, TopologyPolicy};

    use super::{ReplicationEngine, ReplicationOperation, ReplicationStatus};

    fn topology() -> TopologyPolicy {
        TopologyPolicy::from_input(TopologyInput {
            primary_provider: ProviderId::Unicom,
            sync_targets: vec![ProviderId::Telecom, ProviderId::Onedrive],
            fallback_read_order: Vec::new(),
            onedrive_enabled: true,
            replication_mode: ReplicationMode::AsyncBackup,
        })
        .expect("topology should validate")
    }

    #[test]
    fn enqueue_put_creates_one_job_per_sync_target() {
        let engine = ReplicationEngine::new();
        let jobs = engine.enqueue_put(
            &topology(),
            "bucket-a",
            "hello.txt",
            Some("etag-1".to_string()),
            42,
            Some("text/plain".to_string()),
        );

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].target, "telecom");
        assert_eq!(jobs[1].target, "onedrive");
        assert!(matches!(jobs[0].operation, ReplicationOperation::Put));
    }

    #[test]
    fn completed_jobs_move_to_recent_history() {
        let engine = ReplicationEngine::with_recent_limit(4);
        engine.enqueue_delete(&topology(), "bucket-a", "old.txt");

        let job = engine.pop_next().expect("job should exist");
        engine.record_completed(job);

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.recent_count, 1);
        assert!(matches!(
            snapshot.recent_jobs[0].status,
            ReplicationStatus::Completed
        ));
    }
}
