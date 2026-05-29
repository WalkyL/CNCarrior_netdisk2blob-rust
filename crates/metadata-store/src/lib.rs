// SPDX-License-Identifier: LicenseRef-CCBG-Commercial
// Copyright (c) 2026 walky

use std::{
    collections::BTreeMap,
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use replication_engine::{
    ReplicationJob, ReplicationObjectRef, ReplicationOperation, ReplicationStatus,
};
use rusqlite::{Connection, OptionalExtension, ToSql, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataSnapshot {
    pub pending_count: usize,
    pub retry_scheduled_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub target_statuses: Vec<MetadataTargetStatus>,
    pub recent_jobs: Vec<ReplicationJob>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationDeadLetterStatus {
    Open,
    Replayed,
}

impl ReplicationDeadLetterStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Replayed => "replayed",
        }
    }

    fn parse(value: &str) -> Result<Self, MetadataError> {
        match value {
            "open" => Ok(Self::Open),
            "replayed" => Ok(Self::Replayed),
            _ => Err(MetadataError::InvalidDeadLetterStatus(value.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplicationDeadLetterEntry {
    pub original_job: ReplicationJob,
    pub status: ReplicationDeadLetterStatus,
    pub dead_lettered_at_unix_ms: u64,
    pub reason: String,
    pub replay_count: u32,
    pub last_replayed_job_id: Option<u64>,
    pub last_replayed_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectPlacementRecord {
    pub provider: String,
    pub bucket: String,
    pub key: String,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectPlacementProviderSummaryRecord {
    pub provider: String,
    pub object_count: usize,
    pub latest_updated_at_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogicalObjectRecord {
    pub bucket: String,
    pub key: String,
    pub application_id: Option<String>,
    pub encrypted: bool,
    pub encryption_profile_id: Option<String>,
    pub algorithm: Option<String>,
    pub key_id: Option<String>,
    pub key_source_kind: Option<String>,
    pub key_source_ref: Option<String>,
    pub chunk_plaintext_bytes: Option<u64>,
    pub plaintext_size: u64,
    pub stored_size: u64,
    pub logical_content_type: Option<String>,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectProtectionPlanRecord {
    pub bucket: String,
    pub key: String,
    pub sync_targets_csv: String,
    pub fallback_read_order_csv: String,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayWriteAheadLogStateRecord {
    pub next_lsn: u64,
    pub last_checkpoint_lsn: Option<u64>,
    pub last_replayed_lsn: Option<u64>,
    pub updated_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultipartUploadSessionRecord {
    pub upload_id: String,
    pub bucket: String,
    pub key: String,
    pub application_id: Option<String>,
    pub content_type: Option<String>,
    pub initiated_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MultipartUploadPartRecord {
    pub upload_id: String,
    pub part_number: u32,
    pub etag: String,
    pub size_bytes: u64,
    pub offset_bytes: u64,
    pub updated_at_unix_ms: u64,
}

impl Default for GatewayWriteAheadLogStateRecord {
    fn default() -> Self {
        Self {
            next_lsn: 1,
            last_checkpoint_lsn: None,
            last_replayed_lsn: None,
            updated_at_unix_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataTargetStatus {
    pub target: String,
    pub queued_count: usize,
    pub pending_count: usize,
    pub retry_scheduled_count: usize,
    pub completed_count: usize,
    pub failed_count: usize,
    pub latest_job: Option<ReplicationJob>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataRetentionPolicy {
    pub completed_history_limit: usize,
    pub failed_history_limit: usize,
}

impl Default for MetadataRetentionPolicy {
    fn default() -> Self {
        Self {
            completed_history_limit: 512,
            failed_history_limit: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataStoreOptions {
    pub retention: MetadataRetentionPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetadataPruneResult {
    pub deleted_completed_jobs: usize,
    pub deleted_failed_jobs: usize,
}

impl MetadataPruneResult {
    pub fn total_deleted(self) -> usize {
        self.deleted_completed_jobs + self.deleted_failed_jobs
    }
}

pub struct MetadataStore {
    db_path: PathBuf,
    connection: Mutex<Connection>,
    retention: MetadataRetentionPolicy,
}

impl MetadataStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MetadataError> {
        Self::open_with_options(path, MetadataStoreOptions::default())
    }

    pub fn open_with_options(
        path: impl Into<PathBuf>,
        options: MetadataStoreOptions,
    ) -> Result<Self, MetadataError> {
        let db_path = path.into();

        if db_path != Path::new(":memory:") {
            if let Some(parent) = db_path.parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).map_err(|error| MetadataError::Io {
                        path: parent.to_path_buf(),
                        source: error,
                    })?;
                }
            }
        }

        let connection = Connection::open(&db_path).map_err(MetadataError::Sqlite)?;
        let store = Self {
            db_path,
            connection: Mutex::new(connection),
            retention: options.retention,
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn retention(&self) -> MetadataRetentionPolicy {
        self.retention
    }

    pub fn enqueue_jobs(&self, jobs: &[ReplicationJob]) -> Result<(), MetadataError> {
        if jobs.is_empty() {
            return Ok(());
        }

        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        for job in jobs {
            upsert_job(&transaction, job)?;
        }

        transaction.commit().map_err(MetadataError::Sqlite)
    }

    pub fn save_job(&self, job: &ReplicationJob) -> Result<(), MetadataError> {
        self.enqueue_jobs(std::slice::from_ref(job))
    }

    pub fn load_pending_jobs(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<ReplicationJob>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let sql = match limit {
            Some(_) => {
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE status IN ('pending', 'retry_scheduled')
                 ORDER BY job_id ASC
                 LIMIT ?1"
            }
            None => {
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE status IN ('pending', 'retry_scheduled')
                 ORDER BY job_id ASC"
            }
        };

        let mut statement = connection.prepare(sql).map_err(MetadataError::Sqlite)?;
        let rows = match limit {
            Some(limit) => statement
                .query_map([limit as i64], row_to_job)
                .map_err(MetadataError::Sqlite)?,
            None => statement
                .query_map([], row_to_job)
                .map_err(MetadataError::Sqlite)?,
        };

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn max_job_id(&self) -> Result<Option<u64>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let job_id = connection
            .query_row("SELECT MAX(job_id) FROM replication_jobs", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(MetadataError::Sqlite)?;

        Ok(job_id.map(|value| value as u64))
    }

    pub fn mark_job_status(
        &self,
        job_id: u64,
        status: ReplicationStatus,
        attempts: u32,
        last_error: Option<&str>,
    ) -> Result<MetadataPruneResult, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;
        let updated = transaction
            .execute(
                "UPDATE replication_jobs
                 SET status = ?1, attempts = ?2, last_error = ?3, next_attempt_at_unix_ms = NULL
                 WHERE job_id = ?4",
                params![
                    status.as_str(),
                    i64::from(attempts),
                    last_error,
                    job_id as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;

        if updated == 0 {
            return Err(MetadataError::MissingJob(job_id));
        }

        let prune_result = prune_history(&transaction, self.retention)?;
        transaction.commit().map_err(MetadataError::Sqlite)?;
        Ok(prune_result)
    }

    pub fn retry_failed_job(
        &self,
        job_id: u64,
        enqueued_at_unix_ms: u128,
    ) -> Result<ReplicationJob, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        let job = transaction
            .query_row(
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE job_id = ?1",
                [job_id as i64],
                row_to_job,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .ok_or(MetadataError::MissingJob(job_id))?;

        if !matches!(job.status, ReplicationStatus::Failed) {
            return Err(MetadataError::JobNotFailed(job_id));
        }

        let latest_job_id = transaction
            .query_row(
                "SELECT job_id
                 FROM replication_jobs
                 WHERE target = ?1 AND bucket = ?2 AND key = ?3
                 ORDER BY job_id DESC
                 LIMIT 1",
                params![job.target, job.object.bucket, job.object.key],
                |row| row.get::<_, i64>(0),
            )
            .map_err(MetadataError::Sqlite)? as u64;

        if latest_job_id != job_id {
            return Err(MetadataError::JobNotLatest {
                requested_job_id: job_id,
                latest_job_id,
            });
        }

        transaction
            .execute(
                "UPDATE replication_jobs
                 SET status = ?1,
                     attempts = 0,
                     enqueued_at_unix_ms = ?2,
                     next_attempt_at_unix_ms = NULL,
                     last_error = NULL
                 WHERE job_id = ?3",
                params![
                    ReplicationStatus::Pending.as_str(),
                    enqueued_at_unix_ms as i64,
                    job_id as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;

        transaction.commit().map_err(MetadataError::Sqlite)?;

        Ok(ReplicationJob {
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms,
            next_attempt_at_unix_ms: None,
            last_error: None,
            ..job
        })
    }

    pub fn retry_failed_jobs_for_target(
        &self,
        target: &str,
        enqueued_at_unix_ms: u128,
    ) -> Result<Vec<ReplicationJob>, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        let failed_jobs = {
            let mut statement = transaction
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
                     FROM replication_jobs jobs
                     INNER JOIN (
                        SELECT bucket, key, MAX(job_id) AS max_job_id
                        FROM replication_jobs
                        WHERE target = ?1
                        GROUP BY bucket, key
                     ) latest
                     ON latest.max_job_id = jobs.job_id
                     WHERE jobs.target = ?1 AND jobs.status = 'failed'
                     ORDER BY jobs.job_id ASC",
                )
                .map_err(MetadataError::Sqlite)?;

            statement
                .query_map([target], row_to_job)
                .map_err(MetadataError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(MetadataError::Sqlite)?
        };

        for job in &failed_jobs {
            transaction
                .execute(
                    "UPDATE replication_jobs
                     SET status = ?1,
                         attempts = 0,
                         enqueued_at_unix_ms = ?2,
                         next_attempt_at_unix_ms = NULL,
                         last_error = NULL
                     WHERE job_id = ?3",
                    params![
                        ReplicationStatus::Pending.as_str(),
                        enqueued_at_unix_ms as i64,
                        job.job_id as i64,
                    ],
                )
                .map_err(MetadataError::Sqlite)?;
        }

        transaction.commit().map_err(MetadataError::Sqlite)?;

        Ok(failed_jobs
            .into_iter()
            .map(|job| ReplicationJob {
                status: ReplicationStatus::Pending,
                attempts: 0,
                enqueued_at_unix_ms,
                next_attempt_at_unix_ms: None,
                last_error: None,
                ..job
            })
            .collect())
    }

    pub fn apply_retention(&self) -> Result<MetadataPruneResult, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;
        let prune_result = prune_history(&transaction, self.retention)?;
        transaction.commit().map_err(MetadataError::Sqlite)?;
        Ok(prune_result)
    }

    pub fn mark_job_dead_letter(
        &self,
        job_id: u64,
        attempts: u32,
        last_error: Option<&str>,
        reason: &str,
        dead_lettered_at_unix_ms: u64,
    ) -> Result<ReplicationDeadLetterEntry, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        let original_job = transaction
            .query_row(
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE job_id = ?1",
                [job_id as i64],
                row_to_job,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .ok_or(MetadataError::MissingJob(job_id))?;

        transaction
            .execute(
                "UPDATE replication_jobs
                 SET status = ?1, attempts = ?2, last_error = ?3, next_attempt_at_unix_ms = NULL
                 WHERE job_id = ?4",
                params![
                    ReplicationStatus::Failed.as_str(),
                    i64::from(attempts),
                    last_error,
                    job_id as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;

        transaction
            .execute(
                "INSERT INTO replication_dead_letters (
                    original_job_id, target, bucket, key, operation, attempts,
                    dead_lettered_at_unix_ms, last_error, reason, status, replay_count,
                    last_replayed_job_id, last_replayed_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'open', 0, NULL, NULL)
                 ON CONFLICT(original_job_id) DO UPDATE SET
                    target = excluded.target,
                    bucket = excluded.bucket,
                    key = excluded.key,
                    operation = excluded.operation,
                    attempts = excluded.attempts,
                    dead_lettered_at_unix_ms = excluded.dead_lettered_at_unix_ms,
                    last_error = excluded.last_error,
                    reason = excluded.reason,
                    status = 'open',
                    last_replayed_job_id = NULL,
                    last_replayed_at_unix_ms = NULL",
                params![
                    job_id as i64,
                    original_job.target,
                    original_job.object.bucket,
                    original_job.object.key,
                    original_job.operation.as_str(),
                    i64::from(attempts),
                    dead_lettered_at_unix_ms as i64,
                    last_error,
                    reason,
                ],
            )
            .map_err(MetadataError::Sqlite)?;

        let _prune_result = prune_history(&transaction, self.retention)?;
        transaction.commit().map_err(MetadataError::Sqlite)?;

        Ok(ReplicationDeadLetterEntry {
            original_job: ReplicationJob {
                status: ReplicationStatus::Failed,
                attempts,
                next_attempt_at_unix_ms: None,
                last_error: last_error.map(ToString::to_string),
                ..original_job
            },
            status: ReplicationDeadLetterStatus::Open,
            dead_lettered_at_unix_ms,
            reason: reason.to_string(),
            replay_count: 0,
            last_replayed_job_id: None,
            last_replayed_at_unix_ms: None,
        })
    }

    pub fn list_dead_letter_jobs(
        &self,
        target: Option<&str>,
        include_replayed: bool,
        limit: usize,
    ) -> Result<Vec<ReplicationDeadLetterEntry>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let limit = limit.max(1).min(i64::MAX as usize);
        let mut entries = Vec::new();
        let mut statement = match (target, include_replayed) {
            (Some(_), true) => connection
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error,
                            dlq.status, dlq.dead_lettered_at_unix_ms, dlq.reason, dlq.replay_count, dlq.last_replayed_job_id, dlq.last_replayed_at_unix_ms
                     FROM replication_dead_letters dlq
                     INNER JOIN replication_jobs jobs ON jobs.job_id = dlq.original_job_id
                     WHERE dlq.target = ?1
                     ORDER BY dlq.dead_lettered_at_unix_ms DESC, dlq.original_job_id DESC
                     LIMIT ?2",
                )
                .map_err(MetadataError::Sqlite)?,
            (Some(_), false) => connection
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error,
                            dlq.status, dlq.dead_lettered_at_unix_ms, dlq.reason, dlq.replay_count, dlq.last_replayed_job_id, dlq.last_replayed_at_unix_ms
                     FROM replication_dead_letters dlq
                     INNER JOIN replication_jobs jobs ON jobs.job_id = dlq.original_job_id
                     WHERE dlq.target = ?1 AND dlq.status = 'open'
                     ORDER BY dlq.dead_lettered_at_unix_ms DESC, dlq.original_job_id DESC
                     LIMIT ?2",
                )
                .map_err(MetadataError::Sqlite)?,
            (None, true) => connection
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error,
                            dlq.status, dlq.dead_lettered_at_unix_ms, dlq.reason, dlq.replay_count, dlq.last_replayed_job_id, dlq.last_replayed_at_unix_ms
                     FROM replication_dead_letters dlq
                     INNER JOIN replication_jobs jobs ON jobs.job_id = dlq.original_job_id
                     ORDER BY dlq.dead_lettered_at_unix_ms DESC, dlq.original_job_id DESC
                     LIMIT ?1",
                )
                .map_err(MetadataError::Sqlite)?,
            (None, false) => connection
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error,
                            dlq.status, dlq.dead_lettered_at_unix_ms, dlq.reason, dlq.replay_count, dlq.last_replayed_job_id, dlq.last_replayed_at_unix_ms
                     FROM replication_dead_letters dlq
                     INNER JOIN replication_jobs jobs ON jobs.job_id = dlq.original_job_id
                     WHERE dlq.status = 'open'
                     ORDER BY dlq.dead_lettered_at_unix_ms DESC, dlq.original_job_id DESC
                     LIMIT ?1",
                )
                .map_err(MetadataError::Sqlite)?,
        };

        let mut rows = match (target, include_replayed) {
            (Some(target), true) | (Some(target), false) => statement
                .query(params![target, limit as i64])
                .map_err(MetadataError::Sqlite)?,
            (None, _) => statement
                .query([limit as i64])
                .map_err(MetadataError::Sqlite)?,
        };
        while let Some(row) = rows.next().map_err(MetadataError::Sqlite)? {
            entries.push(row_to_dead_letter_entry(row)?);
        }
        Ok(entries)
    }

    pub fn count_dead_letter_jobs(
        &self,
        target: Option<&str>,
        include_replayed: bool,
    ) -> Result<usize, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let sql_all = "SELECT COUNT(*) FROM replication_dead_letters";
        let sql_all_open = "SELECT COUNT(*) FROM replication_dead_letters WHERE status = 'open'";
        let sql_target = "SELECT COUNT(*) FROM replication_dead_letters WHERE target = ?1";
        let sql_target_open =
            "SELECT COUNT(*) FROM replication_dead_letters WHERE target = ?1 AND status = 'open'";
        let count = match (target, include_replayed) {
            (Some(target), true) => {
                connection.query_row(sql_target, [target], |row| row.get::<_, i64>(0))
            }
            (Some(target), false) => {
                connection.query_row(sql_target_open, [target], |row| row.get::<_, i64>(0))
            }
            (None, true) => connection.query_row(sql_all, [], |row| row.get::<_, i64>(0)),
            (None, false) => connection.query_row(sql_all_open, [], |row| row.get::<_, i64>(0)),
        }
        .map_err(MetadataError::Sqlite)?;
        Ok(count.max(0) as usize)
    }

    pub fn open_dead_letter_target_counts(&self) -> Result<BTreeMap<String, usize>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT target, COUNT(*)
                 FROM replication_dead_letters
                 WHERE status = 'open'
                 GROUP BY target
                 ORDER BY target ASC",
            )
            .map_err(MetadataError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(MetadataError::Sqlite)?;
        let mut counts = BTreeMap::new();
        for row in rows {
            let (target, count) = row.map_err(MetadataError::Sqlite)?;
            counts.insert(target, count.max(0) as usize);
        }
        Ok(counts)
    }

    pub fn replay_dead_letter_job(
        &self,
        original_job_id: u64,
        new_job_id: u64,
        enqueued_at_unix_ms: u128,
    ) -> Result<ReplicationJob, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        let original_job = transaction
            .query_row(
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE job_id = ?1",
                [original_job_id as i64],
                row_to_job,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .ok_or(MetadataError::MissingJob(original_job_id))?;
        let (dlq_status, replay_count): (String, i64) = transaction
            .query_row(
                "SELECT status, replay_count
                 FROM replication_dead_letters
                 WHERE original_job_id = ?1",
                [original_job_id as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .ok_or(MetadataError::DeadLetterMissing(original_job_id))?;
        let dlq_status = ReplicationDeadLetterStatus::parse(&dlq_status)?;
        if !matches!(dlq_status, ReplicationDeadLetterStatus::Open) {
            return Err(MetadataError::DeadLetterNotOpen(original_job_id));
        }
        ensure_job_id_available(&transaction, new_job_id)?;

        let replayed_job = ReplicationJob {
            job_id: new_job_id,
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms,
            next_attempt_at_unix_ms: None,
            last_error: None,
            ..original_job
        };
        upsert_job(&transaction, &replayed_job)?;

        transaction
            .execute(
                "UPDATE replication_dead_letters
                 SET status = 'replayed',
                     replay_count = ?1,
                     last_replayed_job_id = ?2,
                     last_replayed_at_unix_ms = ?3
                 WHERE original_job_id = ?4",
                params![
                    replay_count + 1,
                    new_job_id as i64,
                    enqueued_at_unix_ms as i64,
                    original_job_id as i64
                ],
            )
            .map_err(MetadataError::Sqlite)?;

        transaction.commit().map_err(MetadataError::Sqlite)?;
        Ok(replayed_job)
    }

    pub fn replay_dead_letter_jobs_for_target(
        &self,
        target: &str,
        new_job_ids: &[u64],
        enqueued_at_unix_ms: u128,
        limit: usize,
    ) -> Result<Vec<(u64, ReplicationJob)>, MetadataError> {
        if limit == 0 || new_job_ids.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.min(i64::MAX as usize);
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        let original_jobs = {
            let mut statement = transaction
                .prepare(
                    "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
                     FROM replication_dead_letters dlq
                     INNER JOIN replication_jobs jobs ON jobs.job_id = dlq.original_job_id
                     WHERE dlq.target = ?1 AND dlq.status = 'open'
                     ORDER BY dlq.dead_lettered_at_unix_ms DESC, dlq.original_job_id DESC
                     LIMIT ?2",
                )
                .map_err(MetadataError::Sqlite)?;
            statement
                .query_map(params![target, limit as i64], row_to_job)
                .map_err(MetadataError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(MetadataError::Sqlite)?
        };

        if original_jobs.len() > new_job_ids.len() {
            return Err(MetadataError::InsufficientReplayJobIds {
                required: original_jobs.len(),
                provided: new_job_ids.len(),
            });
        }

        let mut allocated_ids = HashSet::new();
        for new_job_id in new_job_ids.iter().copied().take(original_jobs.len()) {
            if !allocated_ids.insert(new_job_id) {
                return Err(MetadataError::DuplicateReplayJobId(new_job_id));
            }
            ensure_job_id_available(&transaction, new_job_id)?;
        }

        let mut replayed_jobs = Vec::with_capacity(original_jobs.len());
        for (original_job, new_job_id) in original_jobs.into_iter().zip(new_job_ids.iter().copied())
        {
            let original_job_id = original_job.job_id;
            let replay_count = transaction
                .query_row(
                    "SELECT replay_count
                     FROM replication_dead_letters
                     WHERE original_job_id = ?1 AND status = 'open'",
                    [original_job_id as i64],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(MetadataError::Sqlite)?
                .ok_or(MetadataError::DeadLetterNotOpen(original_job_id))?;

            let replayed_job = ReplicationJob {
                job_id: new_job_id,
                status: ReplicationStatus::Pending,
                attempts: 0,
                enqueued_at_unix_ms,
                next_attempt_at_unix_ms: None,
                last_error: None,
                ..original_job
            };
            upsert_job(&transaction, &replayed_job)?;
            transaction
                .execute(
                    "UPDATE replication_dead_letters
                     SET status = 'replayed',
                         replay_count = ?1,
                         last_replayed_job_id = ?2,
                         last_replayed_at_unix_ms = ?3
                     WHERE original_job_id = ?4 AND status = 'open'",
                    params![
                        replay_count + 1,
                        new_job_id as i64,
                        enqueued_at_unix_ms as i64,
                        original_job_id as i64
                    ],
                )
                .map_err(MetadataError::Sqlite)?;
            replayed_jobs.push((original_job_id, replayed_job));
        }

        transaction.commit().map_err(MetadataError::Sqlite)?;
        Ok(replayed_jobs)
    }

    pub fn snapshot(&self, recent_limit: usize) -> Result<MetadataSnapshot, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let aggregated = latest_status_aggregate(&connection)?;
        let target_statuses = target_statuses(&connection)?;

        let mut statement = connection
            .prepare(
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 ORDER BY job_id DESC
                 LIMIT ?1",
            )
            .map_err(MetadataError::Sqlite)?;
        let recent_jobs = statement
            .query_map([recent_limit as i64], row_to_job)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)?;

        Ok(MetadataSnapshot {
            pending_count: aggregated.pending_count,
            retry_scheduled_count: aggregated.retry_scheduled_count,
            completed_count: aggregated.completed_count,
            failed_count: aggregated.failed_count,
            target_statuses,
            recent_jobs,
        })
    }

    pub fn latest_job_for_object(
        &self,
        target: &str,
        bucket: &str,
        key: &str,
    ) -> Result<Option<ReplicationJob>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT job_id, target, source_provider, operation, bucket, key, etag, size, content_type, status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
                 FROM replication_jobs
                 WHERE target = ?1 AND bucket = ?2 AND key = ?3
                 ORDER BY job_id DESC
                 LIMIT 1",
                params![target, bucket, key],
                row_to_job,
            )
            .optional()
            .map_err(MetadataError::Sqlite)
    }

    pub fn latest_jobs_for_bucket(
        &self,
        target: &str,
        bucket: &str,
    ) -> Result<Vec<ReplicationJob>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
                 FROM replication_jobs jobs
                 INNER JOIN (
                    SELECT key, MAX(job_id) AS max_job_id
                    FROM replication_jobs
                    WHERE target = ?1 AND bucket = ?2
                    GROUP BY key
                 ) latest
                 ON latest.max_job_id = jobs.job_id
                 ORDER BY jobs.key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map(params![target, bucket], row_to_job)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn latest_failed_jobs(
        &self,
        target: Option<&str>,
    ) -> Result<Vec<ReplicationJob>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let sql_all = "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
                 FROM replication_jobs jobs
                 INNER JOIN (
                    SELECT target, bucket, key, MAX(job_id) AS max_job_id
                    FROM replication_jobs
                    GROUP BY target, bucket, key
                 ) latest
                 ON latest.max_job_id = jobs.job_id
                 WHERE jobs.status = 'failed'
                 ORDER BY jobs.job_id DESC";
        let sql_target = "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
                 FROM replication_jobs jobs
                 INNER JOIN (
                    SELECT target, bucket, key, MAX(job_id) AS max_job_id
                    FROM replication_jobs
                    WHERE target = ?1
                    GROUP BY target, bucket, key
                 ) latest
                 ON latest.max_job_id = jobs.job_id
                 WHERE jobs.target = ?1 AND jobs.status = 'failed'
                 ORDER BY jobs.job_id DESC";

        let mut statement = connection
            .prepare(if target.is_some() {
                sql_target
            } else {
                sql_all
            })
            .map_err(MetadataError::Sqlite)?;
        let rows = match target {
            Some(target) => statement
                .query_map([target], row_to_job)
                .map_err(MetadataError::Sqlite)?,
            None => statement
                .query_map([], row_to_job)
                .map_err(MetadataError::Sqlite)?,
        };

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn fallback_readable_buckets(&self, target: &str) -> Result<Vec<String>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT jobs.bucket
                 FROM replication_jobs jobs
                 INNER JOIN (
                    SELECT bucket, key, MAX(job_id) AS max_job_id
                    FROM replication_jobs
                    WHERE target = ?1
                    GROUP BY bucket, key
                 ) latest
                 ON latest.max_job_id = jobs.job_id
                 WHERE jobs.operation = 'put' AND jobs.status = 'completed'
                 ORDER BY jobs.bucket ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([target], |row| row.get::<_, String>(0))
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn upsert_object_placement(
        &self,
        provider: &str,
        bucket: &str,
        key: &str,
        updated_at_unix_ms: u64,
    ) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO object_placements (provider, bucket, key, updated_at_unix_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(bucket, key) DO UPDATE SET
                    provider = excluded.provider,
                    updated_at_unix_ms = excluded.updated_at_unix_ms",
                params![provider, bucket, key, updated_at_unix_ms as i64],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn delete_object_placement(&self, bucket: &str, key: &str) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "DELETE FROM object_placements WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn object_placement(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Option<ObjectPlacementRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT provider, bucket, key, updated_at_unix_ms
                 FROM object_placements
                 WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
                |row| {
                    Ok(ObjectPlacementRecord {
                        provider: row.get(0)?,
                        bucket: row.get(1)?,
                        key: row.get(2)?,
                        updated_at_unix_ms: row.get::<_, i64>(3)? as u64,
                    })
                },
            )
            .optional()
            .map_err(MetadataError::Sqlite)
    }

    pub fn object_placements_for_bucket(
        &self,
        bucket: &str,
    ) -> Result<Vec<ObjectPlacementRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT provider, bucket, key, updated_at_unix_ms
                 FROM object_placements
                 WHERE bucket = ?1
                 ORDER BY key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([bucket], |row| {
                Ok(ObjectPlacementRecord {
                    provider: row.get(0)?,
                    bucket: row.get(1)?,
                    key: row.get(2)?,
                    updated_at_unix_ms: row.get::<_, i64>(3)? as u64,
                })
            })
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn object_placements_for_provider(
        &self,
        provider: &str,
        limit: usize,
    ) -> Result<Vec<ObjectPlacementRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT provider, bucket, key, updated_at_unix_ms
                 FROM object_placements
                 WHERE provider = ?1
                 ORDER BY updated_at_unix_ms DESC, bucket ASC, key ASC
                 LIMIT ?2",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map(params![provider, limit as i64], |row| {
                Ok(ObjectPlacementRecord {
                    provider: row.get(0)?,
                    bucket: row.get(1)?,
                    key: row.get(2)?,
                    updated_at_unix_ms: row.get::<_, i64>(3)? as u64,
                })
            })
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn all_object_placements(&self) -> Result<Vec<ObjectPlacementRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT provider, bucket, key, updated_at_unix_ms
                 FROM object_placements
                 ORDER BY bucket ASC, key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([], row_to_object_placement_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn upsert_logical_object(&self, record: &LogicalObjectRecord) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO logical_objects (
                    bucket, key, application_id, encrypted, encryption_profile_id, algorithm, key_id,
                    key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                    stored_size, logical_content_type, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(bucket, key) DO UPDATE SET
                    application_id = excluded.application_id,
                    encrypted = excluded.encrypted,
                    encryption_profile_id = excluded.encryption_profile_id,
                    algorithm = excluded.algorithm,
                    key_id = excluded.key_id,
                    key_source_kind = excluded.key_source_kind,
                    key_source_ref = excluded.key_source_ref,
                    chunk_plaintext_bytes = excluded.chunk_plaintext_bytes,
                    plaintext_size = excluded.plaintext_size,
                    stored_size = excluded.stored_size,
                    logical_content_type = excluded.logical_content_type,
                    updated_at_unix_ms = excluded.updated_at_unix_ms",
                params![
                    record.bucket.as_str(),
                    record.key.as_str(),
                    record.application_id.as_deref(),
                    if record.encrypted { 1 } else { 0 },
                    record.encryption_profile_id.as_deref(),
                    record.algorithm.as_deref(),
                    record.key_id.as_deref(),
                    record.key_source_kind.as_deref(),
                    record.key_source_ref.as_deref(),
                    record.chunk_plaintext_bytes.map(|value| value as i64),
                    record.plaintext_size as i64,
                    record.stored_size as i64,
                    record.logical_content_type.as_deref(),
                    record.updated_at_unix_ms as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn delete_logical_object(&self, bucket: &str, key: &str) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "DELETE FROM logical_objects WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn upsert_object_protection_plan(
        &self,
        record: &ObjectProtectionPlanRecord,
    ) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO object_protection_plans (
                    bucket, key, sync_targets_csv, fallback_read_order_csv, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(bucket, key) DO UPDATE SET
                    sync_targets_csv = excluded.sync_targets_csv,
                    fallback_read_order_csv = excluded.fallback_read_order_csv,
                    updated_at_unix_ms = excluded.updated_at_unix_ms",
                params![
                    record.bucket.as_str(),
                    record.key.as_str(),
                    record.sync_targets_csv.as_str(),
                    record.fallback_read_order_csv.as_str(),
                    record.updated_at_unix_ms as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn delete_object_protection_plan(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "DELETE FROM object_protection_plans WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn object_protection_plan(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Option<ObjectProtectionPlanRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT bucket, key, sync_targets_csv, fallback_read_order_csv, updated_at_unix_ms
                 FROM object_protection_plans
                 WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
                row_to_object_protection_plan_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)
    }

    pub fn gateway_write_ahead_log_state(
        &self,
    ) -> Result<GatewayWriteAheadLogStateRecord, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let record = connection
            .query_row(
                "SELECT next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
                 FROM gateway_write_ahead_log_state
                 WHERE singleton_id = 1",
                [],
                row_to_gateway_write_ahead_log_state_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?;
        Ok(record.unwrap_or_default())
    }

    pub fn gateway_write_ahead_log_replay_floor_lsn(&self) -> Result<Option<u64>, MetadataError> {
        let state = self.gateway_write_ahead_log_state()?;
        Ok(state.last_replayed_lsn.or(state.last_checkpoint_lsn))
    }

    pub fn allocate_gateway_write_ahead_log_lsn(
        &self,
        updated_at_unix_ms: u64,
    ) -> Result<u64, MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;
        let state = transaction
            .query_row(
                "SELECT next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
                 FROM gateway_write_ahead_log_state
                 WHERE singleton_id = 1",
                [],
                row_to_gateway_write_ahead_log_state_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .unwrap_or_default();
        let allocated_lsn = state.next_lsn.max(1);
        upsert_gateway_write_ahead_log_state(
            &transaction,
            &GatewayWriteAheadLogStateRecord {
                next_lsn: allocated_lsn.saturating_add(1),
                last_checkpoint_lsn: state.last_checkpoint_lsn,
                last_replayed_lsn: state.last_replayed_lsn,
                updated_at_unix_ms,
            },
        )?;
        transaction.commit().map_err(MetadataError::Sqlite)?;
        Ok(allocated_lsn)
    }

    pub fn update_gateway_write_ahead_log_checkpoint_lsn(
        &self,
        checkpoint_lsn: u64,
        updated_at_unix_ms: u64,
    ) -> Result<(), MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;
        let state = transaction
            .query_row(
                "SELECT next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
                 FROM gateway_write_ahead_log_state
                 WHERE singleton_id = 1",
                [],
                row_to_gateway_write_ahead_log_state_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .unwrap_or_default();
        upsert_gateway_write_ahead_log_state(
            &transaction,
            &GatewayWriteAheadLogStateRecord {
                next_lsn: state.next_lsn.max(checkpoint_lsn.saturating_add(1)),
                last_checkpoint_lsn: Some(checkpoint_lsn),
                last_replayed_lsn: Some(state.last_replayed_lsn.unwrap_or(0).max(checkpoint_lsn))
                    .filter(|value| *value > 0),
                updated_at_unix_ms,
            },
        )?;
        transaction.commit().map_err(MetadataError::Sqlite)
    }

    pub fn update_gateway_write_ahead_log_replayed_lsn(
        &self,
        replayed_lsn: u64,
        updated_at_unix_ms: u64,
    ) -> Result<(), MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;
        let state = transaction
            .query_row(
                "SELECT next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
                 FROM gateway_write_ahead_log_state
                 WHERE singleton_id = 1",
                [],
                row_to_gateway_write_ahead_log_state_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)?
            .unwrap_or_default();
        upsert_gateway_write_ahead_log_state(
            &transaction,
            &GatewayWriteAheadLogStateRecord {
                next_lsn: state.next_lsn.max(replayed_lsn.saturating_add(1)),
                last_checkpoint_lsn: state.last_checkpoint_lsn,
                last_replayed_lsn: Some(state.last_replayed_lsn.unwrap_or(0).max(replayed_lsn))
                    .filter(|value| *value > 0),
                updated_at_unix_ms,
            },
        )?;
        transaction.commit().map_err(MetadataError::Sqlite)
    }

    pub fn logical_object(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Option<LogicalObjectRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT bucket, key, application_id, encrypted, encryption_profile_id, algorithm, key_id,
                        key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                        stored_size, logical_content_type, updated_at_unix_ms
                 FROM logical_objects
                 WHERE bucket = ?1 AND key = ?2",
                params![bucket, key],
                row_to_logical_object_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)
    }

    pub fn logical_objects_for_bucket(
        &self,
        bucket: &str,
    ) -> Result<Vec<LogicalObjectRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT bucket, key, application_id, encrypted, encryption_profile_id, algorithm, key_id,
                        key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                        stored_size, logical_content_type, updated_at_unix_ms
                 FROM logical_objects
                 WHERE bucket = ?1
                 ORDER BY key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([bucket], row_to_logical_object_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn all_logical_objects(&self) -> Result<Vec<LogicalObjectRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT bucket, key, application_id, encrypted, encryption_profile_id, algorithm, key_id,
                        key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                        stored_size, logical_content_type, updated_at_unix_ms
                 FROM logical_objects
                 ORDER BY bucket ASC, key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([], row_to_logical_object_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn all_object_protection_plans(
        &self,
    ) -> Result<Vec<ObjectProtectionPlanRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT bucket, key, sync_targets_csv, fallback_read_order_csv, updated_at_unix_ms
                 FROM object_protection_plans
                 ORDER BY bucket ASC, key ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([], row_to_object_protection_plan_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn replace_backup_state(
        &self,
        object_placements: &[ObjectPlacementRecord],
        logical_objects: &[LogicalObjectRecord],
        object_protection_plans: &[ObjectProtectionPlanRecord],
        pending_replication_jobs: &[ReplicationJob],
        gateway_write_ahead_log_state: &GatewayWriteAheadLogStateRecord,
    ) -> Result<(), MetadataError> {
        let mut connection = self.connection.lock().expect("metadata store poisoned");
        let transaction = connection.transaction().map_err(MetadataError::Sqlite)?;

        transaction
            .execute("DELETE FROM replication_jobs", [])
            .map_err(MetadataError::Sqlite)?;
        transaction
            .execute("DELETE FROM object_placements", [])
            .map_err(MetadataError::Sqlite)?;
        transaction
            .execute("DELETE FROM logical_objects", [])
            .map_err(MetadataError::Sqlite)?;
        transaction
            .execute("DELETE FROM object_protection_plans", [])
            .map_err(MetadataError::Sqlite)?;
        transaction
            .execute("DELETE FROM gateway_write_ahead_log_state", [])
            .map_err(MetadataError::Sqlite)?;

        for job in pending_replication_jobs {
            upsert_job(&transaction, job)?;
        }
        for record in object_placements {
            transaction
                .execute(
                    "INSERT INTO object_placements (provider, bucket, key, updated_at_unix_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        record.provider.as_str(),
                        record.bucket.as_str(),
                        record.key.as_str(),
                        record.updated_at_unix_ms as i64,
                    ],
                )
                .map_err(MetadataError::Sqlite)?;
        }
        for record in logical_objects {
            transaction
                .execute(
                    "INSERT INTO logical_objects (
                        bucket, key, application_id, encrypted, encryption_profile_id, algorithm, key_id,
                        key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                        stored_size, logical_content_type, updated_at_unix_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                    params![
                        record.bucket.as_str(),
                        record.key.as_str(),
                        record.application_id.as_deref(),
                        if record.encrypted { 1 } else { 0 },
                        record.encryption_profile_id.as_deref(),
                        record.algorithm.as_deref(),
                        record.key_id.as_deref(),
                        record.key_source_kind.as_deref(),
                        record.key_source_ref.as_deref(),
                        record.chunk_plaintext_bytes.map(|value| value as i64),
                        record.plaintext_size as i64,
                        record.stored_size as i64,
                        record.logical_content_type.as_deref(),
                        record.updated_at_unix_ms as i64,
                    ],
                )
                .map_err(MetadataError::Sqlite)?;
        }
        for record in object_protection_plans {
            transaction
                .execute(
                    "INSERT INTO object_protection_plans (
                        bucket, key, sync_targets_csv, fallback_read_order_csv, updated_at_unix_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        record.bucket.as_str(),
                        record.key.as_str(),
                        record.sync_targets_csv.as_str(),
                        record.fallback_read_order_csv.as_str(),
                        record.updated_at_unix_ms as i64,
                    ],
                )
                .map_err(MetadataError::Sqlite)?;
        }
        upsert_gateway_write_ahead_log_state(&transaction, gateway_write_ahead_log_state)?;

        transaction.commit().map_err(MetadataError::Sqlite)
    }

    pub fn list_object_placements(
        &self,
        provider: Option<&str>,
        bucket: Option<&str>,
        key_prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ObjectPlacementRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let provider = provider
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let bucket = bucket
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let key_prefix = key_prefix.map(str::trim).filter(|value| !value.is_empty());
        let key_prefix_like = key_prefix.map(sqlite_like_prefix_pattern);
        let limit_i64 = limit.max(1) as i64;

        let mut sql = String::from(
            "SELECT provider, bucket, key, updated_at_unix_ms
             FROM object_placements",
        );
        let mut filters = Vec::new();
        let mut query_params: Vec<&dyn ToSql> = Vec::new();

        if let Some(ref provider) = provider {
            filters.push(format!("provider = ?{}", query_params.len() + 1));
            query_params.push(provider as &dyn ToSql);
        }
        if let Some(ref bucket) = bucket {
            filters.push(format!("bucket = ?{}", query_params.len() + 1));
            query_params.push(bucket as &dyn ToSql);
        }
        if let Some(ref key_prefix_like) = key_prefix_like {
            filters.push(format!("key LIKE ?{} ESCAPE '\\'", query_params.len() + 1));
            query_params.push(key_prefix_like as &dyn ToSql);
        }
        if !filters.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&filters.join(" AND "));
        }
        sql.push_str(&format!(
            " ORDER BY updated_at_unix_ms DESC, provider ASC, bucket ASC, key ASC LIMIT ?{}",
            query_params.len() + 1
        ));
        query_params.push(&limit_i64 as &dyn ToSql);

        let mut statement = connection.prepare(&sql).map_err(MetadataError::Sqlite)?;
        statement
            .query_map(query_params.as_slice(), row_to_object_placement_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn object_placement_provider_summaries(
        &self,
    ) -> Result<Vec<ObjectPlacementProviderSummaryRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT provider, COUNT(*) AS object_count, MAX(updated_at_unix_ms) AS latest_updated_at_unix_ms
                 FROM object_placements
                 GROUP BY provider
                 ORDER BY object_count DESC, provider ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([], |row| {
                Ok(ObjectPlacementProviderSummaryRecord {
                    provider: row.get(0)?,
                    object_count: row.get::<_, i64>(1)? as usize,
                    latest_updated_at_unix_ms: row
                        .get::<_, Option<i64>>(2)?
                        .map(|value| value as u64),
                })
            })
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn create_multipart_upload_session(
        &self,
        record: &MultipartUploadSessionRecord,
    ) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO multipart_upload_sessions (
                    upload_id, bucket, key, application_id, content_type, initiated_at_unix_ms, expires_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(upload_id) DO UPDATE SET
                    bucket = excluded.bucket,
                    key = excluded.key,
                    application_id = excluded.application_id,
                    content_type = excluded.content_type,
                    initiated_at_unix_ms = excluded.initiated_at_unix_ms,
                    expires_at_unix_ms = excluded.expires_at_unix_ms",
                params![
                    record.upload_id.as_str(),
                    record.bucket.as_str(),
                    record.key.as_str(),
                    record.application_id.as_deref(),
                    record.content_type.as_deref(),
                    record.initiated_at_unix_ms as i64,
                    record.expires_at_unix_ms as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn multipart_upload_session(
        &self,
        upload_id: &str,
    ) -> Result<Option<MultipartUploadSessionRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT upload_id, bucket, key, application_id, content_type, initiated_at_unix_ms, expires_at_unix_ms
                 FROM multipart_upload_sessions
                 WHERE upload_id = ?1",
                [upload_id],
                row_to_multipart_upload_session_record,
            )
            .optional()
            .map_err(MetadataError::Sqlite)
    }

    pub fn list_active_multipart_upload_sessions(
        &self,
        now_unix_ms: u64,
    ) -> Result<Vec<MultipartUploadSessionRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT upload_id, bucket, key, application_id, content_type, initiated_at_unix_ms, expires_at_unix_ms
                 FROM multipart_upload_sessions
                 WHERE expires_at_unix_ms > ?1
                 ORDER BY initiated_at_unix_ms ASC, upload_id ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([now_unix_ms as i64], row_to_multipart_upload_session_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    pub fn prune_expired_multipart_upload_sessions(
        &self,
        now_unix_ms: u64,
    ) -> Result<usize, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let deleted = connection
            .execute(
                "DELETE FROM multipart_upload_sessions WHERE expires_at_unix_ms <= ?1",
                [now_unix_ms as i64],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(deleted)
    }

    pub fn delete_multipart_upload_session(&self, upload_id: &str) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "DELETE FROM multipart_upload_sessions WHERE upload_id = ?1",
                [upload_id],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn upsert_multipart_upload_part(
        &self,
        record: &MultipartUploadPartRecord,
    ) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO multipart_upload_parts (
                    upload_id, part_number, etag, size_bytes, offset_bytes, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(upload_id, part_number) DO UPDATE SET
                    etag = excluded.etag,
                    size_bytes = excluded.size_bytes,
                    offset_bytes = excluded.offset_bytes,
                    updated_at_unix_ms = excluded.updated_at_unix_ms",
                params![
                    record.upload_id.as_str(),
                    i64::from(record.part_number),
                    record.etag.as_str(),
                    record.size_bytes as i64,
                    record.offset_bytes as i64,
                    record.updated_at_unix_ms as i64,
                ],
            )
            .map_err(MetadataError::Sqlite)?;
        Ok(())
    }

    pub fn list_multipart_upload_parts(
        &self,
        upload_id: &str,
    ) -> Result<Vec<MultipartUploadPartRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        let mut statement = connection
            .prepare(
                "SELECT upload_id, part_number, etag, size_bytes, offset_bytes, updated_at_unix_ms
                 FROM multipart_upload_parts
                 WHERE upload_id = ?1
                 ORDER BY part_number ASC",
            )
            .map_err(MetadataError::Sqlite)?;

        statement
            .query_map([upload_id], row_to_multipart_upload_part_record)
            .map_err(MetadataError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(MetadataError::Sqlite)
    }

    fn init_schema(&self) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                CREATE TABLE IF NOT EXISTS replication_jobs (
                    job_id INTEGER PRIMARY KEY,
                    target TEXT NOT NULL,
                    source_provider TEXT NULL,
                    operation TEXT NOT NULL,
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    etag TEXT NULL,
                    size INTEGER NULL,
                    content_type TEXT NULL,
                    status TEXT NOT NULL,
                    attempts INTEGER NOT NULL,
                    enqueued_at_unix_ms INTEGER NOT NULL,
                    next_attempt_at_unix_ms INTEGER NULL,
                    last_error TEXT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_replication_jobs_status_job_id
                    ON replication_jobs(status, job_id);
                CREATE TABLE IF NOT EXISTS replication_dead_letters (
                    original_job_id INTEGER PRIMARY KEY,
                    target TEXT NOT NULL,
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    attempts INTEGER NOT NULL,
                    dead_lettered_at_unix_ms INTEGER NOT NULL,
                    last_error TEXT NULL,
                    reason TEXT NOT NULL,
                    status TEXT NOT NULL,
                    replay_count INTEGER NOT NULL,
                    last_replayed_job_id INTEGER NULL,
                    last_replayed_at_unix_ms INTEGER NULL,
                    FOREIGN KEY(original_job_id) REFERENCES replication_jobs(job_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_replication_dead_letters_status_target_dead_lettered
                    ON replication_dead_letters(status, target, dead_lettered_at_unix_ms DESC, original_job_id DESC);
                CREATE TABLE IF NOT EXISTS object_placements (
                    provider TEXT NOT NULL,
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    updated_at_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY(bucket, key)
                );
                CREATE INDEX IF NOT EXISTS idx_object_placements_provider_bucket
                    ON object_placements(provider, bucket);
                CREATE TABLE IF NOT EXISTS logical_objects (
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    application_id TEXT NULL,
                    encrypted INTEGER NOT NULL,
                    encryption_profile_id TEXT NULL,
                    algorithm TEXT NULL,
                    key_id TEXT NULL,
                    key_source_kind TEXT NULL,
                    key_source_ref TEXT NULL,
                    chunk_plaintext_bytes INTEGER NULL,
                    plaintext_size INTEGER NOT NULL,
                    stored_size INTEGER NOT NULL,
                    logical_content_type TEXT NULL,
                    updated_at_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY(bucket, key)
                );
                CREATE INDEX IF NOT EXISTS idx_logical_objects_bucket_updated_at
                    ON logical_objects(bucket, updated_at_unix_ms);
                CREATE TABLE IF NOT EXISTS object_protection_plans (
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    sync_targets_csv TEXT NOT NULL,
                    fallback_read_order_csv TEXT NOT NULL,
                    updated_at_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY(bucket, key)
                );
                CREATE INDEX IF NOT EXISTS idx_object_protection_plans_updated_at
                    ON object_protection_plans(updated_at_unix_ms);
                CREATE TABLE IF NOT EXISTS gateway_write_ahead_log_state (
                    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
                    next_lsn INTEGER NOT NULL,
                    last_checkpoint_lsn INTEGER NULL,
                    last_replayed_lsn INTEGER NULL,
                    updated_at_unix_ms INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS multipart_upload_sessions (
                    upload_id TEXT PRIMARY KEY,
                    bucket TEXT NOT NULL,
                    key TEXT NOT NULL,
                    application_id TEXT NULL,
                    content_type TEXT NULL,
                    initiated_at_unix_ms INTEGER NOT NULL,
                    expires_at_unix_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_multipart_upload_sessions_expires_at
                    ON multipart_upload_sessions(expires_at_unix_ms);
                CREATE TABLE IF NOT EXISTS multipart_upload_parts (
                    upload_id TEXT NOT NULL,
                    part_number INTEGER NOT NULL,
                    etag TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    offset_bytes INTEGER NOT NULL,
                    updated_at_unix_ms INTEGER NOT NULL,
                    PRIMARY KEY(upload_id, part_number),
                    FOREIGN KEY(upload_id) REFERENCES multipart_upload_sessions(upload_id) ON DELETE CASCADE
                );
                INSERT OR IGNORE INTO gateway_write_ahead_log_state (
                    singleton_id, next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
                ) VALUES (1, 1, NULL, NULL, 0);",
            )
            .map_err(MetadataError::Sqlite)?;
        ensure_replication_jobs_column(&connection, "source_provider", "TEXT NULL")
            .and_then(|_| {
                ensure_replication_jobs_column(
                    &connection,
                    "next_attempt_at_unix_ms",
                    "INTEGER NULL",
                )
            })
            .and_then(|_| ensure_logical_objects_column(&connection, "application_id", "TEXT NULL"))
            .and_then(|_| {
                ensure_gateway_write_ahead_log_state_column(
                    &connection,
                    "last_replayed_lsn",
                    "INTEGER NULL",
                )
            })
    }
}

fn row_to_object_placement_record(
    row: &rusqlite::Row<'_>,
) -> Result<ObjectPlacementRecord, rusqlite::Error> {
    Ok(ObjectPlacementRecord {
        provider: row.get(0)?,
        bucket: row.get(1)?,
        key: row.get(2)?,
        updated_at_unix_ms: row.get::<_, i64>(3)? as u64,
    })
}

fn row_to_multipart_upload_session_record(
    row: &rusqlite::Row<'_>,
) -> Result<MultipartUploadSessionRecord, rusqlite::Error> {
    Ok(MultipartUploadSessionRecord {
        upload_id: row.get(0)?,
        bucket: row.get(1)?,
        key: row.get(2)?,
        application_id: row.get(3)?,
        content_type: row.get(4)?,
        initiated_at_unix_ms: row.get::<_, i64>(5)? as u64,
        expires_at_unix_ms: row.get::<_, i64>(6)? as u64,
    })
}

fn row_to_multipart_upload_part_record(
    row: &rusqlite::Row<'_>,
) -> Result<MultipartUploadPartRecord, rusqlite::Error> {
    Ok(MultipartUploadPartRecord {
        upload_id: row.get(0)?,
        part_number: row.get::<_, i64>(1)? as u32,
        etag: row.get(2)?,
        size_bytes: row.get::<_, i64>(3)? as u64,
        offset_bytes: row.get::<_, i64>(4)? as u64,
        updated_at_unix_ms: row.get::<_, i64>(5)? as u64,
    })
}

fn row_to_gateway_write_ahead_log_state_record(
    row: &rusqlite::Row<'_>,
) -> Result<GatewayWriteAheadLogStateRecord, rusqlite::Error> {
    Ok(GatewayWriteAheadLogStateRecord {
        next_lsn: row.get::<_, i64>(0)?.max(1) as u64,
        last_checkpoint_lsn: row.get::<_, Option<i64>>(1)?.map(|value| value as u64),
        last_replayed_lsn: row.get::<_, Option<i64>>(2)?.map(|value| value as u64),
        updated_at_unix_ms: row.get::<_, i64>(3)?.max(0) as u64,
    })
}

fn upsert_gateway_write_ahead_log_state(
    connection: &Connection,
    record: &GatewayWriteAheadLogStateRecord,
) -> Result<(), MetadataError> {
    connection
        .execute(
            "INSERT INTO gateway_write_ahead_log_state (
                singleton_id, next_lsn, last_checkpoint_lsn, last_replayed_lsn, updated_at_unix_ms
             ) VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(singleton_id) DO UPDATE SET
                next_lsn = excluded.next_lsn,
                last_checkpoint_lsn = excluded.last_checkpoint_lsn,
                last_replayed_lsn = excluded.last_replayed_lsn,
                updated_at_unix_ms = excluded.updated_at_unix_ms",
            params![
                record.next_lsn.max(1) as i64,
                record.last_checkpoint_lsn.map(|value| value as i64),
                record.last_replayed_lsn.map(|value| value as i64),
                record.updated_at_unix_ms as i64,
            ],
        )
        .map_err(MetadataError::Sqlite)?;
    Ok(())
}

fn ensure_gateway_write_ahead_log_state_column(
    connection: &Connection,
    column_name: &str,
    definition: &str,
) -> Result<(), MetadataError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(gateway_write_ahead_log_state)")
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(MetadataError::Sqlite)?;
    let existing = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    if existing.iter().any(|column| column == column_name) {
        return Ok(());
    }

    connection
        .execute(
            &format!(
                "ALTER TABLE gateway_write_ahead_log_state ADD COLUMN {column_name} {definition}"
            ),
            [],
        )
        .map_err(MetadataError::Sqlite)?;
    Ok(())
}

fn row_to_logical_object_record(
    row: &rusqlite::Row<'_>,
) -> Result<LogicalObjectRecord, rusqlite::Error> {
    Ok(LogicalObjectRecord {
        bucket: row.get(0)?,
        key: row.get(1)?,
        application_id: row.get(2)?,
        encrypted: row.get::<_, i64>(3)? != 0,
        encryption_profile_id: row.get(4)?,
        algorithm: row.get(5)?,
        key_id: row.get(6)?,
        key_source_kind: row.get(7)?,
        key_source_ref: row.get(8)?,
        chunk_plaintext_bytes: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
        plaintext_size: row.get::<_, i64>(10)? as u64,
        stored_size: row.get::<_, i64>(11)? as u64,
        logical_content_type: row.get(12)?,
        updated_at_unix_ms: row.get::<_, i64>(13)? as u64,
    })
}

fn row_to_object_protection_plan_record(
    row: &rusqlite::Row<'_>,
) -> Result<ObjectProtectionPlanRecord, rusqlite::Error> {
    Ok(ObjectProtectionPlanRecord {
        bucket: row.get(0)?,
        key: row.get(1)?,
        sync_targets_csv: row.get(2)?,
        fallback_read_order_csv: row.get(3)?,
        updated_at_unix_ms: row.get::<_, i64>(4)? as u64,
    })
}

fn sqlite_like_prefix_pattern(prefix: &str) -> String {
    let mut pattern = String::with_capacity(prefix.len() + 1);
    for ch in prefix.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn ensure_replication_jobs_column(
    connection: &Connection,
    column_name: &str,
    definition: &str,
) -> Result<(), MetadataError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(replication_jobs)")
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(MetadataError::Sqlite)?;
    let existing = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    if existing.iter().any(|column| column == column_name) {
        return Ok(());
    }

    connection
        .execute(
            &format!("ALTER TABLE replication_jobs ADD COLUMN {column_name} {definition}"),
            [],
        )
        .map_err(MetadataError::Sqlite)?;
    Ok(())
}

fn ensure_logical_objects_column(
    connection: &Connection,
    column_name: &str,
    definition: &str,
) -> Result<(), MetadataError> {
    let mut statement = connection
        .prepare("PRAGMA table_info(logical_objects)")
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(MetadataError::Sqlite)?;
    let existing = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    if existing.iter().any(|column| column == column_name) {
        return Ok(());
    }

    connection
        .execute(
            &format!("ALTER TABLE logical_objects ADD COLUMN {column_name} {definition}"),
            [],
        )
        .map_err(MetadataError::Sqlite)?;
    Ok(())
}

fn prune_history(
    connection: &Connection,
    retention: MetadataRetentionPolicy,
) -> Result<MetadataPruneResult, MetadataError> {
    let mut protected_job_ids = latest_job_ids(connection)?;
    protected_job_ids.extend(dead_letter_original_job_ids(connection)?);
    let deleted_completed_jobs = prune_status_history(
        connection,
        ReplicationStatus::Completed,
        retention.completed_history_limit,
        &protected_job_ids,
    )?;
    let deleted_failed_jobs = prune_status_history(
        connection,
        ReplicationStatus::Failed,
        retention.failed_history_limit,
        &protected_job_ids,
    )?;

    Ok(MetadataPruneResult {
        deleted_completed_jobs,
        deleted_failed_jobs,
    })
}

fn latest_job_ids(connection: &Connection) -> Result<HashSet<u64>, MetadataError> {
    let mut statement = connection
        .prepare(
            "SELECT MAX(job_id)
             FROM replication_jobs
             GROUP BY target, bucket, key",
        )
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(MetadataError::Sqlite)?;

    let latest = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?
        .into_iter()
        .map(|job_id| job_id as u64)
        .collect();

    Ok(latest)
}

fn dead_letter_original_job_ids(connection: &Connection) -> Result<HashSet<u64>, MetadataError> {
    let mut statement = connection
        .prepare("SELECT original_job_id FROM replication_dead_letters")
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(MetadataError::Sqlite)?;

    let job_ids = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?
        .into_iter()
        .map(|job_id| job_id as u64)
        .collect();

    Ok(job_ids)
}

fn prune_status_history(
    connection: &Connection,
    status: ReplicationStatus,
    history_limit: usize,
    latest_job_ids: &HashSet<u64>,
) -> Result<usize, MetadataError> {
    let mut statement = connection
        .prepare(
            "SELECT job_id
             FROM replication_jobs
             WHERE status = ?1
             ORDER BY job_id DESC",
        )
        .map_err(MetadataError::Sqlite)?;
    let rows = statement
        .query_map([status.as_str()], |row| row.get::<_, i64>(0))
        .map_err(MetadataError::Sqlite)?;
    let job_ids = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    let mut kept_history = 0usize;
    let mut deleted = 0usize;

    for job_id in job_ids {
        let job_id = job_id as u64;
        if latest_job_ids.contains(&job_id) {
            continue;
        }

        if kept_history < history_limit {
            kept_history += 1;
            continue;
        }

        connection
            .execute(
                "DELETE FROM replication_jobs WHERE job_id = ?1",
                [job_id as i64],
            )
            .map_err(MetadataError::Sqlite)?;
        deleted += 1;
    }

    Ok(deleted)
}

fn upsert_job(connection: &Connection, job: &ReplicationJob) -> Result<(), MetadataError> {
    connection
        .execute(
            "INSERT INTO replication_jobs (
                job_id, target, source_provider, operation, bucket, key, etag, size, content_type,
                status, attempts, enqueued_at_unix_ms, next_attempt_at_unix_ms, last_error
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(job_id) DO UPDATE SET
                target = excluded.target,
                source_provider = excluded.source_provider,
                operation = excluded.operation,
                bucket = excluded.bucket,
                key = excluded.key,
                etag = excluded.etag,
                size = excluded.size,
                content_type = excluded.content_type,
                status = excluded.status,
                attempts = excluded.attempts,
                enqueued_at_unix_ms = excluded.enqueued_at_unix_ms,
                next_attempt_at_unix_ms = excluded.next_attempt_at_unix_ms,
                last_error = excluded.last_error",
            params![
                job.job_id as i64,
                job.target,
                job.source_provider,
                job.operation.as_str(),
                job.object.bucket,
                job.object.key,
                job.object.etag,
                job.object.size.map(|value| value as i64),
                job.object.content_type,
                job.status.as_str(),
                i64::from(job.attempts),
                job.enqueued_at_unix_ms as i64,
                job.next_attempt_at_unix_ms.map(|value| value as i64),
                job.last_error,
            ],
        )
        .map_err(MetadataError::Sqlite)?;

    Ok(())
}

fn ensure_job_id_available(connection: &Connection, job_id: u64) -> Result<(), MetadataError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM replication_jobs WHERE job_id = ?1",
            [job_id as i64],
            |_| Ok(()),
        )
        .optional()
        .map_err(MetadataError::Sqlite)?
        .is_some();
    if exists {
        return Err(MetadataError::JobAlreadyExists(job_id));
    }
    Ok(())
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReplicationJob> {
    Ok(ReplicationJob {
        job_id: row.get::<_, i64>(0)? as u64,
        target: row.get(1)?,
        source_provider: row.get(2)?,
        operation: ReplicationOperation::parse(&row.get::<_, String>(3)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        object: ReplicationObjectRef {
            bucket: row.get(4)?,
            key: row.get(5)?,
            etag: row.get(6)?,
            size: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
            content_type: row.get(8)?,
        },
        status: ReplicationStatus::parse(&row.get::<_, String>(9)?).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        attempts: row.get::<_, i64>(10)? as u32,
        enqueued_at_unix_ms: row.get::<_, i64>(11)? as u128,
        next_attempt_at_unix_ms: row.get::<_, Option<i64>>(12)?.map(|value| value as u128),
        last_error: row.get(13)?,
    })
}

fn row_to_dead_letter_entry(
    row: &rusqlite::Row<'_>,
) -> Result<ReplicationDeadLetterEntry, MetadataError> {
    let operation_value = row.get::<_, String>(3).map_err(MetadataError::Sqlite)?;
    let operation = ReplicationOperation::parse(&operation_value)
        .map_err(|error| MetadataError::InvalidOperation(error.to_string()))?;
    let status_value = row.get::<_, String>(9).map_err(MetadataError::Sqlite)?;
    let status = ReplicationStatus::parse(&status_value)
        .map_err(|error| MetadataError::InvalidStatus(error.to_string()))?;
    let dlq_status_value = row.get::<_, String>(14).map_err(MetadataError::Sqlite)?;
    Ok(ReplicationDeadLetterEntry {
        original_job: ReplicationJob {
            job_id: row.get::<_, i64>(0).map_err(MetadataError::Sqlite)? as u64,
            target: row.get(1).map_err(MetadataError::Sqlite)?,
            source_provider: row.get(2).map_err(MetadataError::Sqlite)?,
            operation,
            object: ReplicationObjectRef {
                bucket: row.get(4).map_err(MetadataError::Sqlite)?,
                key: row.get(5).map_err(MetadataError::Sqlite)?,
                etag: row.get(6).map_err(MetadataError::Sqlite)?,
                size: row
                    .get::<_, Option<i64>>(7)
                    .map_err(MetadataError::Sqlite)?
                    .map(|value| value as u64),
                content_type: row.get(8).map_err(MetadataError::Sqlite)?,
            },
            status,
            attempts: row.get::<_, i64>(10).map_err(MetadataError::Sqlite)? as u32,
            enqueued_at_unix_ms: row.get::<_, i64>(11).map_err(MetadataError::Sqlite)? as u128,
            next_attempt_at_unix_ms: row
                .get::<_, Option<i64>>(12)
                .map_err(MetadataError::Sqlite)?
                .map(|value| value as u128),
            last_error: row.get(13).map_err(MetadataError::Sqlite)?,
        },
        status: ReplicationDeadLetterStatus::parse(&dlq_status_value)?,
        dead_lettered_at_unix_ms: row.get::<_, i64>(15).map_err(MetadataError::Sqlite)? as u64,
        reason: row.get(16).map_err(MetadataError::Sqlite)?,
        replay_count: row.get::<_, i64>(17).map_err(MetadataError::Sqlite)? as u32,
        last_replayed_job_id: row
            .get::<_, Option<i64>>(18)
            .map_err(MetadataError::Sqlite)?
            .map(|value| value as u64),
        last_replayed_at_unix_ms: row
            .get::<_, Option<i64>>(19)
            .map_err(MetadataError::Sqlite)?
            .map(|value| value as u64),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct LatestStatusAggregate {
    pending_count: usize,
    retry_scheduled_count: usize,
    completed_count: usize,
    failed_count: usize,
}

fn latest_status_aggregate(
    connection: &Connection,
) -> Result<LatestStatusAggregate, MetadataError> {
    let mut statement = connection
        .prepare(
            "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
             FROM replication_jobs jobs
             INNER JOIN (
                 SELECT target, bucket, key, MAX(job_id) AS max_job_id
                 FROM replication_jobs
                 GROUP BY target, bucket, key
             ) latest
             ON latest.max_job_id = jobs.job_id",
        )
        .map_err(MetadataError::Sqlite)?;
    let jobs = statement
        .query_map([], row_to_job)
        .map_err(MetadataError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    let mut aggregate = LatestStatusAggregate::default();
    for job in jobs {
        match job.status {
            ReplicationStatus::Pending => aggregate.pending_count += 1,
            ReplicationStatus::RetryScheduled => aggregate.retry_scheduled_count += 1,
            ReplicationStatus::Completed => aggregate.completed_count += 1,
            ReplicationStatus::Failed => aggregate.failed_count += 1,
        }
    }

    Ok(aggregate)
}

fn target_statuses(connection: &Connection) -> Result<Vec<MetadataTargetStatus>, MetadataError> {
    let mut statement = connection
        .prepare(
            "SELECT jobs.job_id, jobs.target, jobs.source_provider, jobs.operation, jobs.bucket, jobs.key, jobs.etag, jobs.size, jobs.content_type, jobs.status, jobs.attempts, jobs.enqueued_at_unix_ms, jobs.next_attempt_at_unix_ms, jobs.last_error
             FROM replication_jobs jobs
             INNER JOIN (
                 SELECT target, bucket, key, MAX(job_id) AS max_job_id
                 FROM replication_jobs
                 GROUP BY target, bucket, key
             ) latest
             ON latest.max_job_id = jobs.job_id
             ORDER BY jobs.target ASC, jobs.job_id DESC",
        )
        .map_err(MetadataError::Sqlite)?;
    let jobs = statement
        .query_map([], row_to_job)
        .map_err(MetadataError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MetadataError::Sqlite)?;

    let mut grouped = BTreeMap::<String, MetadataTargetStatus>::new();
    for job in jobs {
        let entry = grouped
            .entry(job.target.clone())
            .or_insert_with(|| MetadataTargetStatus {
                target: job.target.clone(),
                queued_count: 0,
                pending_count: 0,
                retry_scheduled_count: 0,
                completed_count: 0,
                failed_count: 0,
                latest_job: Some(job.clone()),
            });

        if entry.latest_job.is_none() {
            entry.latest_job = Some(job.clone());
        }

        match job.status {
            ReplicationStatus::Pending => {
                entry.queued_count += 1;
                entry.pending_count += 1;
            }
            ReplicationStatus::RetryScheduled => {
                entry.queued_count += 1;
                entry.retry_scheduled_count += 1;
            }
            ReplicationStatus::Completed => entry.completed_count += 1,
            ReplicationStatus::Failed => entry.failed_count += 1,
        }
    }

    Ok(grouped.into_values().collect())
}

#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("metadata sqlite error: {0}")]
    Sqlite(#[source] rusqlite::Error),
    #[error("metadata io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("replication job {0} was not found")]
    MissingJob(u64),
    #[error("replication job {0} already exists")]
    JobAlreadyExists(u64),
    #[error("replication job {0} is not currently failed")]
    JobNotFailed(u64),
    #[error(
        "replication job {requested_job_id} is no longer the latest state for its object; latest job is {latest_job_id}"
    )]
    JobNotLatest {
        requested_job_id: u64,
        latest_job_id: u64,
    },
    #[error("replication dead letter for job {0} was not found")]
    DeadLetterMissing(u64),
    #[error("replication dead letter for job {0} is not open")]
    DeadLetterNotOpen(u64),
    #[error("invalid replication dead-letter status: {0}")]
    InvalidDeadLetterStatus(String),
    #[error("invalid replication operation: {0}")]
    InvalidOperation(String),
    #[error("invalid replication status: {0}")]
    InvalidStatus(String),
    #[error("duplicate replay job id: {0}")]
    DuplicateReplayJobId(u64),
    #[error("insufficient replay job ids: required {required}, provided {provided}")]
    InsufficientReplayJobIds { required: usize, provided: usize },
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::Arc,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use replication_engine::{
        ReplicationJob, ReplicationObjectRef, ReplicationOperation, ReplicationStatus,
    };

    use super::{
        GatewayWriteAheadLogStateRecord, LogicalObjectRecord, MetadataError,
        MetadataRetentionPolicy, MetadataStore, MetadataStoreOptions, MultipartUploadPartRecord,
        MultipartUploadSessionRecord, ObjectPlacementRecord, ObjectProtectionPlanRecord,
        ReplicationDeadLetterStatus,
    };

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ccbg-metadata-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be valid")
                .as_nanos()
        ))
    }

    fn sample_job(job_id: u64) -> ReplicationJob {
        ReplicationJob {
            job_id,
            target: "onedrive".to_string(),
            source_provider: Some("unicom".to_string()),
            operation: ReplicationOperation::Put,
            object: ReplicationObjectRef {
                bucket: "bucket-a".to_string(),
                key: "hello.txt".to_string(),
                etag: Some("etag-1".to_string()),
                size: Some(12),
                content_type: Some("text/plain".to_string()),
            },
            status: ReplicationStatus::Pending,
            attempts: 0,
            enqueued_at_unix_ms: 1234,
            next_attempt_at_unix_ms: None,
            last_error: None,
        }
    }

    #[test]
    fn pending_jobs_round_trip_through_sqlite() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        store
            .enqueue_jobs(&[sample_job(1), sample_job(2)])
            .expect("jobs should persist");

        let jobs = store.load_pending_jobs(None).expect("jobs should load");
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].job_id, 1);
        assert_eq!(jobs[1].job_id, 2);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn status_updates_are_reflected_in_snapshot() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        store
            .enqueue_jobs(&[sample_job(7)])
            .expect("job should persist");
        store
            .mark_job_status(7, ReplicationStatus::Failed, 1, Some("write not supported"))
            .expect("job status should update");

        let snapshot = store.snapshot(10).expect("snapshot should load");
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.retry_scheduled_count, 0);
        assert_eq!(snapshot.failed_count, 1);
        assert_eq!(snapshot.recent_jobs[0].job_id, 7);
        assert_eq!(
            snapshot.recent_jobs[0].last_error.as_deref(),
            Some("write not supported")
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn failed_job_can_be_requeued_as_pending() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        store
            .enqueue_jobs(&[sample_job(8)])
            .expect("job should persist");
        store
            .mark_job_status(8, ReplicationStatus::Failed, 2, Some("temporary outage"))
            .expect("job should fail");

        let retried = store
            .retry_failed_job(8, 9_999)
            .expect("failed job should retry");

        assert_eq!(retried.job_id, 8);
        assert!(matches!(retried.status, ReplicationStatus::Pending));
        assert_eq!(retried.attempts, 0);
        assert_eq!(retried.enqueued_at_unix_ms, 9_999);
        assert!(retried.last_error.is_none());

        let snapshot = store.snapshot(10).expect("snapshot should load");
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.retry_scheduled_count, 0);
        assert_eq!(snapshot.failed_count, 0);
        assert_eq!(snapshot.recent_jobs[0].job_id, 8);
        assert!(matches!(
            snapshot.recent_jobs[0].status,
            ReplicationStatus::Pending
        ));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn retry_rejects_non_latest_failed_job() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut failed_job = sample_job(20);
        failed_job.status = ReplicationStatus::Failed;

        let mut newer_completed = sample_job(21);
        newer_completed.status = ReplicationStatus::Completed;

        store
            .enqueue_jobs(&[failed_job, newer_completed])
            .expect("jobs should persist");

        let error = store
            .retry_failed_job(20, 9_999)
            .expect_err("retry should reject stale failed job");
        assert!(matches!(
            error,
            MetadataError::JobNotLatest {
                requested_job_id: 20,
                latest_job_id: 21,
            }
        ));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn retry_failed_jobs_for_target_requeues_only_latest_failed_jobs() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut onedrive_failed = sample_job(30);
        onedrive_failed.object.key = "retry/only-latest.txt".to_string();
        onedrive_failed.status = ReplicationStatus::Failed;
        onedrive_failed.attempts = 2;
        onedrive_failed.last_error = Some("temporary outage".to_string());

        let mut onedrive_old_failed = sample_job(31);
        onedrive_old_failed.object.key = "retry/stale.txt".to_string();
        onedrive_old_failed.status = ReplicationStatus::Failed;
        onedrive_old_failed.attempts = 1;
        onedrive_old_failed.last_error = Some("old failure".to_string());

        let mut onedrive_newer_completed = sample_job(32);
        onedrive_newer_completed.object.key = "retry/stale.txt".to_string();
        onedrive_newer_completed.status = ReplicationStatus::Completed;

        let mut telecom_failed = sample_job(33);
        telecom_failed.target = "telecom".to_string();
        telecom_failed.object.key = "retry/telecom.txt".to_string();
        telecom_failed.status = ReplicationStatus::Failed;
        telecom_failed.attempts = 3;
        telecom_failed.last_error = Some("target offline".to_string());

        store
            .enqueue_jobs(&[
                onedrive_failed,
                onedrive_old_failed,
                onedrive_newer_completed,
                telecom_failed,
            ])
            .expect("jobs should persist");

        let retried = store
            .retry_failed_jobs_for_target("onedrive", 9_999)
            .expect("target batch retry should succeed");

        assert_eq!(retried.len(), 1);
        assert_eq!(retried[0].job_id, 30);
        assert!(matches!(retried[0].status, ReplicationStatus::Pending));
        assert_eq!(retried[0].attempts, 0);
        assert!(retried[0].last_error.is_none());
        assert_eq!(retried[0].enqueued_at_unix_ms, 9_999);

        let latest_onedrive = store
            .latest_job_for_object("onedrive", "bucket-a", "retry/only-latest.txt")
            .expect("retried onedrive job should load")
            .expect("retried onedrive job should exist");
        assert!(matches!(latest_onedrive.status, ReplicationStatus::Pending));

        let stale_onedrive = store
            .latest_job_for_object("onedrive", "bucket-a", "retry/stale.txt")
            .expect("stale object job should load")
            .expect("stale object job should exist");
        assert_eq!(stale_onedrive.job_id, 32);
        assert!(matches!(
            stale_onedrive.status,
            ReplicationStatus::Completed
        ));

        let latest_telecom = store
            .latest_job_for_object("telecom", "bucket-a", "retry/telecom.txt")
            .expect("telecom job should load")
            .expect("telecom job should exist");
        assert_eq!(latest_telecom.job_id, 33);
        assert!(matches!(latest_telecom.status, ReplicationStatus::Failed));

        let snapshot = store.snapshot(10).expect("snapshot should load");
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.completed_count, 1);
        assert_eq!(snapshot.failed_count, 1);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn latest_failed_jobs_only_return_current_failed_object_states() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut latest_failed = sample_job(40);
        latest_failed.object.key = "failed/latest.txt".to_string();
        latest_failed.status = ReplicationStatus::Failed;
        latest_failed.last_error = Some("latest failure".to_string());

        let mut stale_failed = sample_job(41);
        stale_failed.object.key = "failed/stale.txt".to_string();
        stale_failed.status = ReplicationStatus::Failed;

        let mut stale_completed = sample_job(42);
        stale_completed.object.key = "failed/stale.txt".to_string();
        stale_completed.status = ReplicationStatus::Completed;

        let mut telecom_failed = sample_job(43);
        telecom_failed.target = "telecom".to_string();
        telecom_failed.object.key = "failed/telecom.txt".to_string();
        telecom_failed.status = ReplicationStatus::Failed;

        let mut pending_job = sample_job(44);
        pending_job.object.key = "failed/pending.txt".to_string();
        pending_job.status = ReplicationStatus::Pending;

        store
            .enqueue_jobs(&[
                latest_failed,
                stale_failed,
                stale_completed,
                telecom_failed,
                pending_job,
            ])
            .expect("jobs should persist");

        let failed_jobs = store
            .latest_failed_jobs(None)
            .expect("latest failed jobs should load");
        assert_eq!(failed_jobs.len(), 2);
        assert_eq!(failed_jobs[0].job_id, 43);
        assert_eq!(failed_jobs[1].job_id, 40);

        let onedrive_only = store
            .latest_failed_jobs(Some("onedrive"))
            .expect("target-filtered failed jobs should load");
        assert_eq!(onedrive_only.len(), 1);
        assert_eq!(onedrive_only[0].job_id, 40);
        assert_eq!(onedrive_only[0].object.key, "failed/latest.txt");

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn latest_job_queries_return_most_recent_object_state() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut put_job = sample_job(10);
        put_job.status = ReplicationStatus::Completed;

        let mut delete_job = sample_job(11);
        delete_job.operation = ReplicationOperation::Delete;
        delete_job.status = ReplicationStatus::Pending;

        let mut other_object = sample_job(12);
        other_object.object.key = "notes/other.txt".to_string();
        other_object.status = ReplicationStatus::Completed;

        store
            .enqueue_jobs(&[put_job.clone(), delete_job.clone(), other_object.clone()])
            .expect("jobs should persist");

        let latest = store
            .latest_job_for_object("onedrive", "bucket-a", "hello.txt")
            .expect("latest job should load")
            .expect("object should have a latest job");
        assert_eq!(latest.job_id, 11);
        assert!(matches!(latest.operation, ReplicationOperation::Delete));
        assert!(matches!(latest.status, ReplicationStatus::Pending));

        let latest_by_bucket = store
            .latest_jobs_for_bucket("onedrive", "bucket-a")
            .expect("bucket jobs should load");
        assert_eq!(latest_by_bucket.len(), 2);
        assert_eq!(latest_by_bucket[0].object.key, "hello.txt");
        assert_eq!(latest_by_bucket[0].job_id, 11);
        assert_eq!(latest_by_bucket[1].object.key, "notes/other.txt");
        assert_eq!(latest_by_bucket[1].job_id, 12);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn dead_letter_mark_list_and_replay_preserve_audit() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut onedrive_job = sample_job(100);
        onedrive_job.target = "onedrive".to_string();
        onedrive_job.object.key = "dlq/one.txt".to_string();

        let mut telecom_job = sample_job(101);
        telecom_job.target = "telecom".to_string();
        telecom_job.object.key = "dlq/two.txt".to_string();

        store
            .enqueue_jobs(&[onedrive_job.clone(), telecom_job.clone()])
            .expect("jobs should persist");

        let entry = store
            .mark_job_dead_letter(
                100,
                3,
                Some("target throttled"),
                "max_attempts_exhausted",
                111_000,
            )
            .expect("job should enter dlq");
        assert_eq!(entry.original_job.job_id, 100);
        assert!(matches!(entry.status, ReplicationDeadLetterStatus::Open));
        assert_eq!(entry.original_job.attempts, 3);
        assert_eq!(
            entry.original_job.last_error.as_deref(),
            Some("target throttled")
        );

        store
            .mark_job_dead_letter(101, 1, Some("access denied"), "permanent_failure", 112_000)
            .expect("second job should enter dlq");

        let open_all = store
            .list_dead_letter_jobs(None, false, 10)
            .expect("dlq list should load");
        assert_eq!(open_all.len(), 2);
        assert_eq!(open_all[0].original_job.job_id, 101);
        assert_eq!(open_all[1].original_job.job_id, 100);

        let target_open = store
            .list_dead_letter_jobs(Some("onedrive"), false, 10)
            .expect("target dlq list should load");
        assert_eq!(target_open.len(), 1);
        assert_eq!(target_open[0].original_job.job_id, 100);

        let replayed = store
            .replay_dead_letter_job(100, 200, 222_000)
            .expect("open dlq should replay");
        assert_eq!(replayed.job_id, 200);
        assert!(matches!(replayed.status, ReplicationStatus::Pending));
        assert_eq!(replayed.attempts, 0);
        assert!(replayed.last_error.is_none());

        let open_after_replay = store
            .list_dead_letter_jobs(None, false, 10)
            .expect("open dlq list after replay should load");
        assert_eq!(open_after_replay.len(), 1);
        assert_eq!(open_after_replay[0].original_job.job_id, 101);

        let include_replayed = store
            .list_dead_letter_jobs(Some("onedrive"), true, 10)
            .expect("include replayed list should load");
        assert_eq!(include_replayed.len(), 1);
        assert!(matches!(
            include_replayed[0].status,
            ReplicationDeadLetterStatus::Replayed
        ));
        assert_eq!(include_replayed[0].replay_count, 1);
        assert_eq!(include_replayed[0].last_replayed_job_id, Some(200));
        assert_eq!(include_replayed[0].last_replayed_at_unix_ms, Some(222_000));

        let replay_error = store
            .replay_dead_letter_job(100, 201, 223_000)
            .expect_err("replayed dlq cannot replay again");
        assert!(matches!(
            replay_error,
            MetadataError::DeadLetterNotOpen(100)
        ));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn dead_letter_original_jobs_are_not_pruned_after_replay_or_newer_states() {
        let db_path = temp_db_path();
        let store = MetadataStore::open_with_options(
            &db_path,
            MetadataStoreOptions {
                retention: MetadataRetentionPolicy {
                    completed_history_limit: 0,
                    failed_history_limit: 0,
                },
            },
        )
        .expect("store should open");

        let mut failed_job = sample_job(300);
        failed_job.target = "telecom".to_string();
        failed_job.object.key = "dlq/prune-protected.txt".to_string();
        failed_job.status = ReplicationStatus::Failed;
        store
            .enqueue_jobs(&[failed_job])
            .expect("failed job should persist");
        store
            .mark_job_dead_letter(
                300,
                3,
                Some("target kept failing"),
                "max_attempts_exhausted",
                333_000,
            )
            .expect("job should enter dlq");

        let replayed = store
            .replay_dead_letter_job(300, 301, 334_000)
            .expect("dlq should replay");
        store
            .mark_job_status(replayed.job_id, ReplicationStatus::Completed, 1, None)
            .expect("replayed job completion should apply retention");

        let include_replayed = store
            .list_dead_letter_jobs(Some("telecom"), true, 10)
            .expect("dlq audit should survive retention");
        assert_eq!(include_replayed.len(), 1);
        assert_eq!(include_replayed[0].original_job.job_id, 300);
        assert!(matches!(
            include_replayed[0].status,
            ReplicationDeadLetterStatus::Replayed
        ));

        let original = store
            .latest_job_for_object("telecom", "bucket-a", "dlq/prune-protected.txt")
            .expect("latest job query should succeed")
            .expect("completed replay job should be latest");
        assert_eq!(original.job_id, 301);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn dead_letter_counts_do_not_require_loading_entries() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut telecom_job = sample_job(310);
        telecom_job.target = "telecom".to_string();
        telecom_job.object.key = "dlq/count-a.txt".to_string();
        let mut mobile_job = sample_job(311);
        mobile_job.target = "mobile".to_string();
        mobile_job.object.key = "dlq/count-b.txt".to_string();
        store
            .enqueue_jobs(&[telecom_job, mobile_job])
            .expect("jobs should persist");
        store
            .mark_job_dead_letter(310, 2, Some("fail a"), "max_attempts_exhausted", 1)
            .expect("telecom dlq should persist");
        store
            .mark_job_dead_letter(311, 2, Some("fail b"), "max_attempts_exhausted", 2)
            .expect("mobile dlq should persist");
        store
            .replay_dead_letter_job(310, 312, 3)
            .expect("telecom dlq should replay");

        assert_eq!(
            store
                .count_dead_letter_jobs(None, false)
                .expect("open dlq count should load"),
            1
        );
        assert_eq!(
            store
                .count_dead_letter_jobs(None, true)
                .expect("all dlq count should load"),
            2
        );
        assert_eq!(
            store
                .count_dead_letter_jobs(Some("telecom"), false)
                .expect("target open dlq count should load"),
            0
        );
        let target_counts = store
            .open_dead_letter_target_counts()
            .expect("target counts should load");
        assert_eq!(target_counts.get("mobile"), Some(&1));
        assert!(!target_counts.contains_key("telecom"));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn dead_letter_replay_rejects_existing_new_job_id() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut dlq_job = sample_job(320);
        dlq_job.object.key = "dlq/conflict.txt".to_string();
        let mut existing_job = sample_job(321);
        existing_job.object.key = "dlq/existing.txt".to_string();
        store
            .enqueue_jobs(&[dlq_job, existing_job])
            .expect("jobs should persist");
        store
            .mark_job_dead_letter(320, 2, Some("fail"), "max_attempts_exhausted", 1)
            .expect("dlq should persist");

        let error = store
            .replay_dead_letter_job(320, 321, 2)
            .expect_err("replay must not overwrite existing job id");
        assert!(matches!(error, MetadataError::JobAlreadyExists(321)));
        assert_eq!(
            store
                .count_dead_letter_jobs(None, false)
                .expect("open dlq count should load"),
            1
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn dead_letter_target_replay_is_atomic_before_mutating_entries() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut job_a = sample_job(330);
        job_a.target = "telecom".to_string();
        job_a.object.key = "dlq/atomic-a.txt".to_string();
        let mut job_b = sample_job(331);
        job_b.target = "telecom".to_string();
        job_b.object.key = "dlq/atomic-b.txt".to_string();
        store
            .enqueue_jobs(&[job_a, job_b])
            .expect("jobs should persist");
        store
            .mark_job_dead_letter(330, 2, Some("fail a"), "max_attempts_exhausted", 1)
            .expect("dlq a should persist");
        store
            .mark_job_dead_letter(331, 2, Some("fail b"), "max_attempts_exhausted", 2)
            .expect("dlq b should persist");

        let error = store
            .replay_dead_letter_jobs_for_target("telecom", &[400], 3, 10)
            .expect_err("batch replay should reject too few ids before mutating");
        assert!(matches!(
            error,
            MetadataError::InsufficientReplayJobIds {
                required: 2,
                provided: 1
            }
        ));
        assert_eq!(
            store
                .count_dead_letter_jobs(Some("telecom"), false)
                .expect("open dlq count should load"),
            2
        );

        let replayed = store
            .replay_dead_letter_jobs_for_target("telecom", &[400, 401], 4, 10)
            .expect("batch replay should succeed atomically");
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].0, 331);
        assert_eq!(replayed[0].1.job_id, 400);
        assert_eq!(replayed[1].0, 330);
        assert_eq!(replayed[1].1.job_id, 401);
        assert_eq!(
            store
                .count_dead_letter_jobs(Some("telecom"), false)
                .expect("open dlq count should load"),
            0
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn object_protection_plan_round_trips() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let record = ObjectProtectionPlanRecord {
            bucket: "root".to_string(),
            key: "docs/plan.txt".to_string(),
            sync_targets_csv: "onedrive,telecom".to_string(),
            fallback_read_order_csv: "telecom".to_string(),
            updated_at_unix_ms: 1_234,
        };

        store
            .upsert_object_protection_plan(&record)
            .expect("protection plan should persist");

        let loaded = store
            .object_protection_plan("root", "docs/plan.txt")
            .expect("protection plan should load")
            .expect("protection plan should exist");
        assert_eq!(loaded, record);

        store
            .delete_object_protection_plan("root", "docs/plan.txt")
            .expect("protection plan should delete");
        assert!(
            store
                .object_protection_plan("root", "docs/plan.txt")
                .expect("deleted protection plan lookup should succeed")
                .is_none()
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn retention_prunes_old_history_but_keeps_latest_and_pending_jobs() {
        let db_path = temp_db_path();
        let store = MetadataStore::open_with_options(
            &db_path,
            MetadataStoreOptions {
                retention: MetadataRetentionPolicy {
                    completed_history_limit: 0,
                    failed_history_limit: 0,
                },
            },
        )
        .expect("store should open");

        let mut completed_old = sample_job(1);
        completed_old.status = ReplicationStatus::Completed;

        let mut completed_latest = sample_job(2);
        completed_latest.status = ReplicationStatus::Completed;
        completed_latest.object.key = "object-a.txt".to_string();
        completed_old.object.key = "object-a.txt".to_string();

        let mut failed_old = sample_job(3);
        failed_old.status = ReplicationStatus::Failed;
        failed_old.operation = ReplicationOperation::Delete;
        failed_old.object.key = "object-b.txt".to_string();

        let mut failed_latest = sample_job(4);
        failed_latest.status = ReplicationStatus::Failed;
        failed_latest.operation = ReplicationOperation::Delete;
        failed_latest.object.key = "object-b.txt".to_string();

        let mut pending = sample_job(5);
        pending.object.key = "object-c.txt".to_string();

        store
            .enqueue_jobs(&[
                completed_old,
                completed_latest.clone(),
                failed_old,
                failed_latest.clone(),
                pending.clone(),
            ])
            .expect("jobs should persist");

        let prune = store.apply_retention().expect("retention should apply");
        assert_eq!(prune.deleted_completed_jobs, 1);
        assert_eq!(prune.deleted_failed_jobs, 1);

        let snapshot = store.snapshot(10).expect("snapshot should load");
        let remaining_ids: Vec<u64> = snapshot
            .recent_jobs
            .into_iter()
            .map(|job| job.job_id)
            .collect();
        assert_eq!(snapshot.pending_count, 1);
        assert_eq!(snapshot.retry_scheduled_count, 0);
        assert_eq!(snapshot.completed_count, 1);
        assert_eq!(snapshot.failed_count, 1);
        assert!(remaining_ids.contains(&completed_latest.job_id));
        assert!(remaining_ids.contains(&failed_latest.job_id));
        assert!(remaining_ids.contains(&pending.job_id));
        assert!(!remaining_ids.contains(&1));
        assert!(!remaining_ids.contains(&3));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn fallback_readable_buckets_only_include_latest_completed_puts() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut bucket_a_put = sample_job(20);
        bucket_a_put.object.bucket = "bucket-a".to_string();
        bucket_a_put.object.key = "memory/one.json".to_string();
        bucket_a_put.status = ReplicationStatus::Completed;

        let mut bucket_b_put = sample_job(21);
        bucket_b_put.object.bucket = "bucket-b".to_string();
        bucket_b_put.object.key = "memory/two.json".to_string();
        bucket_b_put.status = ReplicationStatus::Completed;

        let mut bucket_b_delete = sample_job(22);
        bucket_b_delete.object.bucket = "bucket-b".to_string();
        bucket_b_delete.object.key = "memory/two.json".to_string();
        bucket_b_delete.operation = ReplicationOperation::Delete;
        bucket_b_delete.status = ReplicationStatus::Pending;

        store
            .enqueue_jobs(&[bucket_a_put, bucket_b_put, bucket_b_delete])
            .expect("jobs should persist");

        let buckets = store
            .fallback_readable_buckets("onedrive")
            .expect("fallback buckets should load");
        assert_eq!(buckets, vec!["bucket-a".to_string()]);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn snapshot_includes_per_target_latest_status_counts() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut onedrive_completed = sample_job(30);
        onedrive_completed.status = ReplicationStatus::Completed;
        onedrive_completed.object.key = "done.txt".to_string();

        let mut onedrive_retry = sample_job(31);
        onedrive_retry.object.key = "retry.txt".to_string();
        onedrive_retry.status = ReplicationStatus::RetryScheduled;
        onedrive_retry.attempts = 1;
        onedrive_retry.next_attempt_at_unix_ms = Some(9_999);
        onedrive_retry.last_error = Some("temporary upstream outage".to_string());

        let mut telecom_failed = sample_job(32);
        telecom_failed.target = "telecom".to_string();
        telecom_failed.object.key = "failed.txt".to_string();
        telecom_failed.status = ReplicationStatus::Failed;
        telecom_failed.last_error = Some("permanent auth error".to_string());

        store
            .enqueue_jobs(&[
                onedrive_completed.clone(),
                onedrive_retry.clone(),
                telecom_failed.clone(),
            ])
            .expect("jobs should persist");

        let snapshot = store.snapshot(10).expect("snapshot should load");
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.retry_scheduled_count, 1);
        assert_eq!(snapshot.completed_count, 1);
        assert_eq!(snapshot.failed_count, 1);
        assert_eq!(snapshot.target_statuses.len(), 2);

        let onedrive = snapshot
            .target_statuses
            .iter()
            .find(|status| status.target == "onedrive")
            .expect("onedrive target status should exist");
        assert_eq!(onedrive.queued_count, 1);
        assert_eq!(onedrive.pending_count, 0);
        assert_eq!(onedrive.retry_scheduled_count, 1);
        assert_eq!(onedrive.completed_count, 1);
        assert_eq!(onedrive.failed_count, 0);
        assert_eq!(onedrive.latest_job.as_ref().map(|job| job.job_id), Some(31));

        let telecom = snapshot
            .target_statuses
            .iter()
            .find(|status| status.target == "telecom")
            .expect("telecom target status should exist");
        assert_eq!(telecom.queued_count, 0);
        assert_eq!(telecom.failed_count, 1);
        assert_eq!(
            telecom
                .latest_job
                .as_ref()
                .and_then(|job| job.last_error.clone()),
            Some("permanent auth error".to_string())
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn snapshot_counts_only_latest_job_per_object() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let mut failed_old = sample_job(40);
        failed_old.object.key = "same-object.txt".to_string();
        failed_old.status = ReplicationStatus::Failed;
        failed_old.last_error = Some("old failure".to_string());

        let mut completed_latest = sample_job(41);
        completed_latest.object.key = "same-object.txt".to_string();
        completed_latest.status = ReplicationStatus::Completed;

        store
            .enqueue_jobs(&[failed_old, completed_latest.clone()])
            .expect("jobs should persist");

        let snapshot = store.snapshot(10).expect("snapshot should load");
        assert_eq!(snapshot.pending_count, 0);
        assert_eq!(snapshot.retry_scheduled_count, 0);
        assert_eq!(snapshot.completed_count, 1);
        assert_eq!(snapshot.failed_count, 0);

        let onedrive = snapshot
            .target_statuses
            .iter()
            .find(|status| status.target == "onedrive")
            .expect("onedrive target status should exist");
        assert_eq!(onedrive.completed_count, 1);
        assert_eq!(onedrive.failed_count, 0);
        assert_eq!(onedrive.latest_job.as_ref().map(|job| job.job_id), Some(41));

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn object_placement_round_trip_and_delete() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        store
            .upsert_object_placement("telecom", "root", "docs/a.txt", 100)
            .expect("placement should persist");
        store
            .upsert_object_placement("unicom", "root", "docs/b.txt", 200)
            .expect("placement should persist");
        store
            .upsert_object_placement("unicom", "root", "docs/a.txt", 300)
            .expect("placement should update");

        let placement = store
            .object_placement("root", "docs/a.txt")
            .expect("placement query should succeed")
            .expect("placement should exist");
        assert_eq!(
            placement,
            ObjectPlacementRecord {
                provider: "unicom".to_string(),
                bucket: "root".to_string(),
                key: "docs/a.txt".to_string(),
                updated_at_unix_ms: 300,
            }
        );

        let placements = store
            .object_placements_for_bucket("root")
            .expect("bucket placement listing should succeed");
        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].key, "docs/a.txt");
        assert_eq!(placements[1].key, "docs/b.txt");

        store
            .delete_object_placement("root", "docs/a.txt")
            .expect("placement delete should succeed");
        assert!(
            store
                .object_placement("root", "docs/a.txt")
                .expect("placement query should succeed")
                .is_none()
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn object_placement_provider_summary_and_samples() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        store
            .upsert_object_placement("telecom", "root", "docs/a.txt", 100)
            .expect("telecom placement should persist");
        store
            .upsert_object_placement("telecom", "family", "shared/b.txt", 300)
            .expect("telecom family placement should persist");
        store
            .upsert_object_placement("unicom", "root", "docs/c.txt", 200)
            .expect("unicom placement should persist");

        let summaries = store
            .object_placement_provider_summaries()
            .expect("provider summaries should load");
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].provider, "telecom");
        assert_eq!(summaries[0].object_count, 2);
        assert_eq!(summaries[0].latest_updated_at_unix_ms, Some(300));
        assert_eq!(summaries[1].provider, "unicom");
        assert_eq!(summaries[1].object_count, 1);

        let telecom_samples = store
            .object_placements_for_provider("telecom", 8)
            .expect("telecom samples should load");
        assert_eq!(telecom_samples.len(), 2);
        assert_eq!(telecom_samples[0].bucket, "family");
        assert_eq!(telecom_samples[0].key, "shared/b.txt");
        assert_eq!(telecom_samples[1].bucket, "root");
        assert_eq!(telecom_samples[1].key, "docs/a.txt");

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn logical_object_round_trip_and_delete() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        let record = LogicalObjectRecord {
            bucket: "root".to_string(),
            key: "encrypted/a.txt".to_string(),
            application_id: Some("media-app".to_string()),
            encrypted: true,
            encryption_profile_id: Some("router-default".to_string()),
            algorithm: Some("chacha20_poly1305".to_string()),
            key_id: Some("kek-2026-01".to_string()),
            key_source_kind: Some("gateway_managed".to_string()),
            key_source_ref: Some("managed-key-1".to_string()),
            chunk_plaintext_bytes: Some(65_536),
            plaintext_size: 11,
            stored_size: 127,
            logical_content_type: Some("text/plain".to_string()),
            updated_at_unix_ms: 100,
        };

        store
            .upsert_logical_object(&record)
            .expect("logical object should persist");

        let loaded = store
            .logical_object("root", "encrypted/a.txt")
            .expect("logical object should load")
            .expect("logical object should exist");
        assert_eq!(loaded, record);

        let bucket_records = store
            .logical_objects_for_bucket("root")
            .expect("logical bucket query should succeed");
        assert_eq!(bucket_records, vec![record.clone()]);

        store
            .delete_logical_object("root", "encrypted/a.txt")
            .expect("logical object should delete");
        assert!(
            store
                .logical_object("root", "encrypted/a.txt")
                .expect("logical object query should succeed")
                .is_none()
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn backup_export_and_replace_round_trip() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let placement = ObjectPlacementRecord {
            provider: "telecom".to_string(),
            bucket: "root".to_string(),
            key: "docs/placed.txt".to_string(),
            updated_at_unix_ms: 101,
        };
        let logical = LogicalObjectRecord {
            bucket: "root".to_string(),
            key: "docs/placed.txt".to_string(),
            application_id: Some("backup-app".to_string()),
            encrypted: true,
            encryption_profile_id: Some("enc-a".to_string()),
            algorithm: Some("chacha20_poly1305".to_string()),
            key_id: Some("kek-a".to_string()),
            key_source_kind: Some("gateway_managed".to_string()),
            key_source_ref: Some("managed-a".to_string()),
            chunk_plaintext_bytes: Some(131_072),
            plaintext_size: 42,
            stored_size: 128,
            logical_content_type: Some("text/plain".to_string()),
            updated_at_unix_ms: 102,
        };
        let plan = ObjectProtectionPlanRecord {
            bucket: "root".to_string(),
            key: "docs/placed.txt".to_string(),
            sync_targets_csv: "mobile,unicom".to_string(),
            fallback_read_order_csv: "unicom,mobile".to_string(),
            updated_at_unix_ms: 103,
        };
        let mut pending = sample_job(88);
        pending.target = "mobile".to_string();
        pending.object.bucket = "root".to_string();
        pending.object.key = "docs/placed.txt".to_string();
        pending.status = ReplicationStatus::RetryScheduled;
        pending.next_attempt_at_unix_ms = Some(12_345);
        let wal_state = GatewayWriteAheadLogStateRecord {
            next_lsn: 42,
            last_checkpoint_lsn: Some(19),
            last_replayed_lsn: Some(21),
            updated_at_unix_ms: 104,
        };

        store
            .replace_backup_state(
                std::slice::from_ref(&placement),
                std::slice::from_ref(&logical),
                std::slice::from_ref(&plan),
                std::slice::from_ref(&pending),
                &wal_state,
            )
            .expect("backup replacement should succeed");

        assert_eq!(
            store
                .all_object_placements()
                .expect("placements should load after replace"),
            vec![placement]
        );
        assert_eq!(
            store
                .all_logical_objects()
                .expect("logical objects should load after replace"),
            vec![logical]
        );
        assert_eq!(
            store
                .all_object_protection_plans()
                .expect("protection plans should load after replace"),
            vec![plan]
        );
        assert_eq!(
            store
                .load_pending_jobs(None)
                .expect("pending jobs should load after replace"),
            vec![pending]
        );
        assert_eq!(
            store
                .gateway_write_ahead_log_state()
                .expect("wal state should load after replace"),
            wal_state
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn gateway_write_ahead_log_state_allocates_lsn_and_tracks_checkpoint() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        assert_eq!(
            store
                .gateway_write_ahead_log_state()
                .expect("initial wal state should load"),
            GatewayWriteAheadLogStateRecord {
                next_lsn: 1,
                last_checkpoint_lsn: None,
                last_replayed_lsn: None,
                updated_at_unix_ms: 0,
            }
        );

        let lsn = store
            .allocate_gateway_write_ahead_log_lsn(101)
            .expect("lsn allocation should succeed");
        assert_eq!(lsn, 1);
        assert_eq!(
            store
                .gateway_write_ahead_log_state()
                .expect("wal state should load after lsn allocation"),
            GatewayWriteAheadLogStateRecord {
                next_lsn: 2,
                last_checkpoint_lsn: None,
                last_replayed_lsn: None,
                updated_at_unix_ms: 101,
            }
        );

        store
            .update_gateway_write_ahead_log_checkpoint_lsn(1, 202)
            .expect("checkpoint lsn update should succeed");
        assert_eq!(
            store
                .gateway_write_ahead_log_state()
                .expect("wal state should load after checkpoint update"),
            GatewayWriteAheadLogStateRecord {
                next_lsn: 2,
                last_checkpoint_lsn: Some(1),
                last_replayed_lsn: Some(1),
                updated_at_unix_ms: 202,
            }
        );

        store
            .update_gateway_write_ahead_log_replayed_lsn(3, 303)
            .expect("replayed lsn update should succeed");
        assert_eq!(
            store
                .gateway_write_ahead_log_state()
                .expect("wal state should load after replayed lsn update"),
            GatewayWriteAheadLogStateRecord {
                next_lsn: 4,
                last_checkpoint_lsn: Some(1),
                last_replayed_lsn: Some(3),
                updated_at_unix_ms: 303,
            }
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn list_object_placements_filters_prefix_and_orders_by_latest_update() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        store
            .upsert_object_placement("telecom", "root", "docs/a.txt", 100)
            .expect("telecom root placement should persist");
        store
            .upsert_object_placement("telecom", "family", "shared/100%/b.txt", 400)
            .expect("telecom family placement should persist");
        store
            .upsert_object_placement("unicom", "root", "docs/c.txt", 300)
            .expect("unicom placement should persist");
        store
            .upsert_object_placement("mobile", "root", "logs/100%/d.txt", 200)
            .expect("mobile placement should persist");

        let all = store
            .list_object_placements(None, None, None, 10)
            .expect("full placement listing should succeed");
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].key, "shared/100%/b.txt");
        assert_eq!(all[1].key, "docs/c.txt");
        assert_eq!(all[2].key, "logs/100%/d.txt");
        assert_eq!(all[3].key, "docs/a.txt");

        let telecom_only = store
            .list_object_placements(Some("telecom"), None, None, 10)
            .expect("provider-filtered placement listing should succeed");
        assert_eq!(telecom_only.len(), 2);
        assert_eq!(telecom_only[0].bucket, "family");
        assert_eq!(telecom_only[1].bucket, "root");

        let docs_in_root = store
            .list_object_placements(None, Some("root"), Some("docs/"), 10)
            .expect("bucket + prefix filtered placement listing should succeed");
        assert_eq!(docs_in_root.len(), 2);
        assert_eq!(docs_in_root[0].provider, "unicom");
        assert_eq!(docs_in_root[0].key, "docs/c.txt");
        assert_eq!(docs_in_root[1].provider, "telecom");
        assert_eq!(docs_in_root[1].key, "docs/a.txt");

        let percent_prefix = store
            .list_object_placements(None, None, Some("shared/100%/"), 10)
            .expect("escaped prefix filter should succeed");
        assert_eq!(percent_prefix.len(), 1);
        assert_eq!(percent_prefix[0].key, "shared/100%/b.txt");

        let limited = store
            .list_object_placements(None, None, None, 2)
            .expect("limited placement listing should succeed");
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].updated_at_unix_ms, 400);
        assert_eq!(limited[1].updated_at_unix_ms, 300);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_session_create_and_get_round_trip() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        let session = MultipartUploadSessionRecord {
            upload_id: "upload-1".to_string(),
            bucket: "root".to_string(),
            key: "video/part.bin".to_string(),
            application_id: Some("app-1".to_string()),
            content_type: Some("application/octet-stream".to_string()),
            initiated_at_unix_ms: 1_700_000_000_000,
            expires_at_unix_ms: 1_700_000_360_000,
        };
        store
            .create_multipart_upload_session(&session)
            .expect("session should persist");

        let loaded = store
            .multipart_upload_session("upload-1")
            .expect("session should load")
            .expect("session should exist");
        assert_eq!(loaded, session);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_list_parts_returns_sorted_part_numbers() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        store
            .create_multipart_upload_session(&MultipartUploadSessionRecord {
                upload_id: "upload-2".to_string(),
                bucket: "root".to_string(),
                key: "video/a.bin".to_string(),
                application_id: None,
                content_type: None,
                initiated_at_unix_ms: 10,
                expires_at_unix_ms: 1000,
            })
            .expect("session should persist");

        for part_number in [5, 1, 3] {
            store
                .upsert_multipart_upload_part(&MultipartUploadPartRecord {
                    upload_id: "upload-2".to_string(),
                    part_number,
                    etag: format!("etag-{part_number}"),
                    size_bytes: u64::from(part_number),
                    offset_bytes: u64::from(part_number) * 100,
                    updated_at_unix_ms: 99,
                })
                .expect("part should persist");
        }

        let parts = store
            .list_multipart_upload_parts("upload-2")
            .expect("parts should list");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].part_number, 1);
        assert_eq!(parts[1].part_number, 3);
        assert_eq!(parts[2].part_number, 5);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_concurrent_upserts_and_duplicate_part_number_keep_latest_value() {
        let db_path = temp_db_path();
        let store = Arc::new(MetadataStore::open(&db_path).expect("store should open"));
        store
            .create_multipart_upload_session(&MultipartUploadSessionRecord {
                upload_id: "upload-3".to_string(),
                bucket: "root".to_string(),
                key: "video/b.bin".to_string(),
                application_id: None,
                content_type: None,
                initiated_at_unix_ms: 10,
                expires_at_unix_ms: 1000,
            })
            .expect("session should persist");

        let mut workers = Vec::new();
        for i in 1..=6u32 {
            let store = Arc::clone(&store);
            workers.push(thread::spawn(move || {
                store
                    .upsert_multipart_upload_part(&MultipartUploadPartRecord {
                        upload_id: "upload-3".to_string(),
                        part_number: i,
                        etag: format!("etag-{i}"),
                        size_bytes: 100 + u64::from(i),
                        offset_bytes: 500 + u64::from(i),
                        updated_at_unix_ms: 200 + u64::from(i),
                    })
                    .expect("part upsert should succeed");
            }));
        }
        for worker in workers {
            worker.join().expect("worker should finish");
        }

        store
            .upsert_multipart_upload_part(&MultipartUploadPartRecord {
                upload_id: "upload-3".to_string(),
                part_number: 3,
                etag: "etag-3-replaced".to_string(),
                size_bytes: 999,
                offset_bytes: 888,
                updated_at_unix_ms: 777,
            })
            .expect("duplicate part upsert should replace prior value");

        let parts = store
            .list_multipart_upload_parts("upload-3")
            .expect("parts should list");
        assert_eq!(parts.len(), 6);
        assert_eq!(
            parts
                .iter()
                .map(|part| part.part_number)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        let replaced = parts
            .iter()
            .find(|part| part.part_number == 3)
            .expect("replaced part should exist");
        assert_eq!(replaced.updated_at_unix_ms, 777);
        assert_eq!(replaced.etag, "etag-3-replaced");
        assert_eq!(replaced.size_bytes, 999);
        assert_eq!(replaced.offset_bytes, 888);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_state_recovers_after_reopen() {
        let db_path = temp_db_path();
        {
            let store = MetadataStore::open(&db_path).expect("store should open");
            store
                .create_multipart_upload_session(&MultipartUploadSessionRecord {
                    upload_id: "upload-4".to_string(),
                    bucket: "root".to_string(),
                    key: "video/c.bin".to_string(),
                    application_id: Some("app-x".to_string()),
                    content_type: Some("video/mp4".to_string()),
                    initiated_at_unix_ms: 10,
                    expires_at_unix_ms: 1000,
                })
                .expect("session should persist");
            store
                .upsert_multipart_upload_part(&MultipartUploadPartRecord {
                    upload_id: "upload-4".to_string(),
                    part_number: 2,
                    etag: "etag-2".to_string(),
                    size_bytes: 1024,
                    offset_bytes: 0,
                    updated_at_unix_ms: 22,
                })
                .expect("part should persist");
        }

        let reopened = MetadataStore::open(&db_path).expect("store should reopen");
        assert!(
            reopened
                .multipart_upload_session("upload-4")
                .expect("session should load")
                .is_some()
        );
        let parts = reopened
            .list_multipart_upload_parts("upload-4")
            .expect("parts should load");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].part_number, 2);

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_delete_session_cascades_parts() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");
        store
            .create_multipart_upload_session(&MultipartUploadSessionRecord {
                upload_id: "upload-5".to_string(),
                bucket: "root".to_string(),
                key: "video/d.bin".to_string(),
                application_id: None,
                content_type: None,
                initiated_at_unix_ms: 10,
                expires_at_unix_ms: 1000,
            })
            .expect("session should persist");
        store
            .upsert_multipart_upload_part(&MultipartUploadPartRecord {
                upload_id: "upload-5".to_string(),
                part_number: 1,
                etag: "etag-1".to_string(),
                size_bytes: 64,
                offset_bytes: 0,
                updated_at_unix_ms: 11,
            })
            .expect("part should persist");

        store
            .delete_multipart_upload_session("upload-5")
            .expect("session should delete");

        assert!(
            store
                .multipart_upload_session("upload-5")
                .expect("session lookup should succeed")
                .is_none()
        );
        assert!(
            store
                .list_multipart_upload_parts("upload-5")
                .expect("parts list should succeed")
                .is_empty()
        );

        fs::remove_file(db_path).ok();
    }

    #[test]
    fn multipart_prune_expired_or_list_active_sessions() {
        let db_path = temp_db_path();
        let store = MetadataStore::open(&db_path).expect("store should open");

        store
            .create_multipart_upload_session(&MultipartUploadSessionRecord {
                upload_id: "upload-active".to_string(),
                bucket: "root".to_string(),
                key: "video/active.bin".to_string(),
                application_id: None,
                content_type: None,
                initiated_at_unix_ms: 100,
                expires_at_unix_ms: 1_000,
            })
            .expect("active session should persist");
        store
            .create_multipart_upload_session(&MultipartUploadSessionRecord {
                upload_id: "upload-expired".to_string(),
                bucket: "root".to_string(),
                key: "video/expired.bin".to_string(),
                application_id: None,
                content_type: None,
                initiated_at_unix_ms: 100,
                expires_at_unix_ms: 500,
            })
            .expect("expired session should persist");

        let active = store
            .list_active_multipart_upload_sessions(600)
            .expect("active list should succeed");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].upload_id, "upload-active");

        let pruned = store
            .prune_expired_multipart_upload_sessions(600)
            .expect("prune should succeed");
        assert_eq!(pruned, 1);
        assert!(
            store
                .multipart_upload_session("upload-expired")
                .expect("session lookup should succeed")
                .is_none()
        );

        fs::remove_file(db_path).ok();
    }
}
