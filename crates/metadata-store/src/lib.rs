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

    pub fn upsert_logical_object(&self, record: &LogicalObjectRecord) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute(
                "INSERT INTO logical_objects (
                    bucket, key, encrypted, encryption_profile_id, algorithm, key_id,
                    key_source_kind, key_source_ref, chunk_plaintext_bytes, plaintext_size,
                    stored_size, logical_content_type, updated_at_unix_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(bucket, key) DO UPDATE SET
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

    pub fn logical_object(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<Option<LogicalObjectRecord>, MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .query_row(
                "SELECT bucket, key, encrypted, encryption_profile_id, algorithm, key_id,
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
                "SELECT bucket, key, encrypted, encryption_profile_id, algorithm, key_id,
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

    fn init_schema(&self) -> Result<(), MetadataError> {
        let connection = self.connection.lock().expect("metadata store poisoned");
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS replication_jobs (
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
                    ON object_protection_plans(updated_at_unix_ms);",
            )
            .map_err(MetadataError::Sqlite)?;
        ensure_replication_jobs_column(&connection, "source_provider", "TEXT NULL").and_then(|_| {
            ensure_replication_jobs_column(&connection, "next_attempt_at_unix_ms", "INTEGER NULL")
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

fn row_to_logical_object_record(
    row: &rusqlite::Row<'_>,
) -> Result<LogicalObjectRecord, rusqlite::Error> {
    Ok(LogicalObjectRecord {
        bucket: row.get(0)?,
        key: row.get(1)?,
        encrypted: row.get::<_, i64>(2)? != 0,
        encryption_profile_id: row.get(3)?,
        algorithm: row.get(4)?,
        key_id: row.get(5)?,
        key_source_kind: row.get(6)?,
        key_source_ref: row.get(7)?,
        chunk_plaintext_bytes: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
        plaintext_size: row.get::<_, i64>(9)? as u64,
        stored_size: row.get::<_, i64>(10)? as u64,
        logical_content_type: row.get(11)?,
        updated_at_unix_ms: row.get::<_, i64>(12)? as u64,
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

fn prune_history(
    connection: &Connection,
    retention: MetadataRetentionPolicy,
) -> Result<MetadataPruneResult, MetadataError> {
    let latest_job_ids = latest_job_ids(connection)?;
    let deleted_completed_jobs = prune_status_history(
        connection,
        ReplicationStatus::Completed,
        retention.completed_history_limit,
        &latest_job_ids,
    )?;
    let deleted_failed_jobs = prune_status_history(
        connection,
        ReplicationStatus::Failed,
        retention.failed_history_limit,
        &latest_job_ids,
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
    #[error("replication job {0} is not currently failed")]
    JobNotFailed(u64),
    #[error(
        "replication job {requested_job_id} is no longer the latest state for its object; latest job is {latest_job_id}"
    )]
    JobNotLatest {
        requested_job_id: u64,
        latest_job_id: u64,
    },
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use replication_engine::{
        ReplicationJob, ReplicationObjectRef, ReplicationOperation, ReplicationStatus,
    };

    use super::{
        LogicalObjectRecord, MetadataError, MetadataRetentionPolicy, MetadataStore,
        MetadataStoreOptions, ObjectPlacementRecord, ObjectProtectionPlanRecord,
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
}
