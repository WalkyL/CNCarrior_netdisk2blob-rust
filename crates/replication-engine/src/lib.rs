// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationStatus {
    Pending,
    RetryScheduled,
    Completed,
    Failed,
}

impl ReplicationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::RetryScheduled => "retry_scheduled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Result<Self, ReplicationParseError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pending" => Ok(Self::Pending),
            "retry_scheduled" => Ok(Self::RetryScheduled),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            other => Err(ReplicationParseError::UnknownStatus(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationObjectRef {
    pub bucket: String,
    pub key: String,
    pub etag: Option<String>,
    pub size: Option<u64>,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplicationJob {
    pub job_id: u64,
    pub target: String,
    #[serde(default)]
    pub source_provider: Option<String>,
    pub operation: ReplicationOperation,
    pub object: ReplicationObjectRef,
    pub status: ReplicationStatus,
    pub attempts: u32,
    pub enqueued_at_unix_ms: u128,
    #[serde(default)]
    pub next_attempt_at_unix_ms: Option<u128>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationSnapshot {
    pub pending_count: usize,
    pub retry_scheduled_count: usize,
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

    pub fn ensure_next_job_id_at_least(&self, next_job_id: u64) {
        self.next_job_id
            .fetch_max(next_job_id.max(1), Ordering::Relaxed);
    }

    pub fn allocate_job_id(&self) -> u64 {
        self.next_job_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn enqueue_put(
        &self,
        topology: &TopologyPolicy,
        source_provider: Option<String>,
        bucket: impl Into<String>,
        key: impl Into<String>,
        etag: Option<String>,
        size: u64,
        content_type: Option<String>,
    ) -> Vec<ReplicationJob> {
        self.enqueue_jobs(
            topology,
            source_provider,
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
        source_provider: Option<String>,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> Vec<ReplicationJob> {
        self.enqueue_jobs(
            topology,
            source_provider,
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

    pub fn pop_next_ready(&self) -> Option<ReplicationJob> {
        self.pop_next_ready_at(now_unix_ms())
    }

    pub fn pop_next_ready_at(&self, now_unix_ms: u128) -> Option<ReplicationJob> {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        let queue_len = pending.len();
        for _ in 0..queue_len {
            let Some(job) = pending.pop_front() else {
                return None;
            };
            let Some(next_attempt_at) = job.next_attempt_at_unix_ms else {
                return Some(job);
            };
            if next_attempt_at <= now_unix_ms {
                return Some(job);
            }
            pending.push_back(job);
        }
        None
    }

    pub fn schedule_retry(&self, job: ReplicationJob) {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.push_back(job);
    }

    pub fn enqueue_existing_job(&self, job: ReplicationJob) {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.push_back(job);
    }

    pub fn record_completed(&self, mut job: ReplicationJob) {
        job.status = ReplicationStatus::Completed;
        job.next_attempt_at_unix_ms = None;
        self.push_recent(job);
    }

    pub fn record_failed(&self, mut job: ReplicationJob, error: impl Into<String>) {
        job.status = ReplicationStatus::Failed;
        job.next_attempt_at_unix_ms = None;
        job.last_error = Some(error.into());
        self.push_recent(job);
    }

    pub fn snapshot(&self) -> ReplicationSnapshot {
        let pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        let recent = self.recent_jobs.lock().expect("replication queue poisoned");
        let retry_scheduled_count = pending
            .iter()
            .filter(|job| matches!(job.status, ReplicationStatus::RetryScheduled))
            .count();

        ReplicationSnapshot {
            pending_count: pending.len(),
            retry_scheduled_count,
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
        self.ensure_next_job_id_at_least(next_job_id);

        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.extend(jobs);
    }

    pub fn replace_pending(&self, jobs: Vec<ReplicationJob>) {
        let next_job_id = jobs
            .iter()
            .map(|job| job.job_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        self.ensure_next_job_id_at_least(next_job_id);

        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        pending.clear();
        pending.extend(jobs);

        let mut recent = self.recent_jobs.lock().expect("replication queue poisoned");
        recent.clear();
    }

    fn enqueue_jobs(
        &self,
        topology: &TopologyPolicy,
        source_provider: Option<String>,
        operation: ReplicationOperation,
        object: ReplicationObjectRef,
    ) -> Vec<ReplicationJob> {
        let mut pending = self
            .pending_jobs
            .lock()
            .expect("replication queue poisoned");
        let mut created = Vec::with_capacity(topology.sync_targets.len());

        for target in &topology.sync_targets {
            if source_provider.as_deref() == Some(target.as_str()) {
                continue;
            }
            let job = ReplicationJob {
                job_id: self.allocate_job_id(),
                target: target.as_str().to_string(),
                source_provider: source_provider.clone(),
                operation: operation.clone(),
                object: object.clone(),
                status: ReplicationStatus::Pending,
                attempts: 0,
                enqueued_at_unix_ms: now_unix_ms(),
                next_attempt_at_unix_ms: None,
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

    use super::{
        ReplicationEngine, ReplicationJob, ReplicationObjectRef, ReplicationOperation,
        ReplicationStatus,
    };

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
            Some("unicom".to_string()),
            "bucket-a",
            "hello.txt",
            Some("etag-1".to_string()),
            42,
            Some("text/plain".to_string()),
        );

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].target, "telecom");
        assert_eq!(jobs[1].target, "onedrive");
        assert_eq!(jobs[0].source_provider.as_deref(), Some("unicom"));
        assert!(matches!(jobs[0].operation, ReplicationOperation::Put));
    }

    #[test]
    fn enqueue_put_skips_target_that_matches_source_provider() {
        let engine = ReplicationEngine::new();
        let jobs = engine.enqueue_put(
            &topology(),
            Some("telecom".to_string()),
            "bucket-a",
            "hello.txt",
            Some("etag-1".to_string()),
            42,
            Some("text/plain".to_string()),
        );

        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].target, "onedrive");
        assert_eq!(jobs[0].source_provider.as_deref(), Some("telecom"));
    }

    #[test]
    fn completed_jobs_move_to_recent_history() {
        let engine = ReplicationEngine::with_recent_limit(4);
        engine.enqueue_delete(
            &topology(),
            Some("unicom".to_string()),
            "bucket-a",
            "old.txt",
        );

        let job = engine.pop_next_ready().expect("job should exist");
        engine.record_completed(job);

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.retry_scheduled_count, 0);
        assert_eq!(snapshot.recent_count, 1);
        assert!(matches!(
            snapshot.recent_jobs[0].status,
            ReplicationStatus::Completed
        ));
    }

    #[test]
    fn retry_scheduled_jobs_wait_until_due_before_dequeue() {
        let engine = ReplicationEngine::with_recent_limit(4);
        engine.restore_pending(vec![
            ReplicationJob {
                job_id: 1,
                target: "onedrive".to_string(),
                source_provider: Some("unicom".to_string()),
                operation: ReplicationOperation::Put,
                object: ReplicationObjectRef {
                    bucket: "bucket-a".to_string(),
                    key: "later.txt".to_string(),
                    etag: None,
                    size: Some(4),
                    content_type: Some("text/plain".to_string()),
                },
                status: ReplicationStatus::RetryScheduled,
                attempts: 1,
                enqueued_at_unix_ms: 100,
                next_attempt_at_unix_ms: Some(2_000),
                last_error: Some("temporary upstream outage".to_string()),
            },
            ReplicationJob {
                job_id: 2,
                target: "telecom".to_string(),
                source_provider: Some("unicom".to_string()),
                operation: ReplicationOperation::Put,
                object: ReplicationObjectRef {
                    bucket: "bucket-a".to_string(),
                    key: "ready.txt".to_string(),
                    etag: None,
                    size: Some(4),
                    content_type: Some("text/plain".to_string()),
                },
                status: ReplicationStatus::Pending,
                attempts: 0,
                enqueued_at_unix_ms: 101,
                next_attempt_at_unix_ms: None,
                last_error: None,
            },
        ]);

        let first = engine
            .pop_next_ready_at(1_500)
            .expect("ready job should be dequeued first");
        assert_eq!(first.job_id, 2);

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.retry_scheduled_count, 1);
        assert_eq!(snapshot.pending_jobs[0].job_id, 1);

        let retry = engine
            .pop_next_ready_at(2_000)
            .expect("retry job should become ready when due");
        assert_eq!(retry.job_id, 1);
    }

    #[test]
    fn replace_pending_resets_queue_and_recent_history() {
        let engine = ReplicationEngine::with_recent_limit(4);
        engine.enqueue_delete(
            &topology(),
            Some("unicom".to_string()),
            "bucket-a",
            "old.txt",
        );
        let completed = engine.pop_next_ready().expect("job should exist");
        engine.record_completed(completed);

        let replacement = ReplicationJob {
            job_id: 99,
            target: "telecom".to_string(),
            source_provider: Some("mobile".to_string()),
            operation: ReplicationOperation::Put,
            object: ReplicationObjectRef {
                bucket: "root".to_string(),
                key: "restore.txt".to_string(),
                etag: Some("etag-99".to_string()),
                size: Some(123),
                content_type: Some("text/plain".to_string()),
            },
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms: 9_999,
            next_attempt_at_unix_ms: None,
            last_error: None,
        };
        engine.replace_pending(vec![replacement.clone()]);

        let snapshot = engine.snapshot();
        assert_eq!(snapshot.pending_jobs, vec![replacement]);
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.recent_jobs.len(), 0);
        assert_eq!(snapshot.recent_count, 0);
    }

    #[test]
    fn seeded_next_job_id_is_used_for_new_jobs() {
        let engine = ReplicationEngine::new();
        engine.ensure_next_job_id_at_least(9);

        let jobs = engine.enqueue_delete(
            &topology(),
            Some("unicom".to_string()),
            "bucket-a",
            "seeded.txt",
        );

        assert_eq!(jobs[0].job_id, 9);
        assert_eq!(jobs[1].job_id, 10);
    }
}
