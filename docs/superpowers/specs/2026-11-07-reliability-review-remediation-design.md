# CCBG Reliability Review Remediation Design

> Status: architecture frozen after review and approval on 2026-11-07.
>
> This design covers all findings from the repository-wide reliability review. It is deliberately written against the current repository layout. Coding agents must not invent new architecture, move existing modules, or broaden scope without an explicit architecture change.

## 1. Goal and invariants

The goal is to remove stale-operation, remote/local inconsistency, credential-state, provider-translation, browser-flow, stream-integrity, and deployment-test failures while preserving the existing CCBG model:

- one configured primary provider;
- primary write success followed by asynchronous replication to sync targets;
- SQLite metadata as durable control state;
- in-memory replication queues as runtime scheduling aids;
- provider crates implementing the existing `blob_core::BlobBackend` boundary.

The implementation must preserve these invariants:

1. Replication operations are ordered per `(target, bucket, key)`; an older remote operation cannot finish after a newer operation and remain the final remote state.
2. SQLite persistence precedes publication to an in-memory queue. A lost wake-up is recoverable by startup or periodic pending-job recovery.
3. A DLQ replay is executable only when its original operation is still the current operation for its target/object. Superseded entries remain auditable and are never silently converted into a current operation.
4. Remote bytes and logical/encryption metadata are either committed as one recoverable replacement or exposed as an explicit partial-commit/orphan state. Restoring old metadata alone is forbidden after an overwrite.
5. Only explicit Mobile NotFound is translated to an absent object. Timeout, transport, authentication, server, and decode failures propagate.
6. A request-side authentication failure is tagged with the credential generation used by that request. A stale generation cannot mutate the current lease.
7. A lease `requires_reauth=true` is sticky until the same credential generation receives a positive authentication verification. An inconclusive/network probe cannot clear it.
8. Data-plane `/healthz` is a local process/listener liveness check. Provider readiness is reported by the metrics service `/readyz`.
9. S3 prefix matching preserves the requested prefix string, including a trailing `/`; `prefix=docs/` does not match `docs-archive`, while ordinary S3 `prefix=docs` retains ordinary string-prefix semantics.
10. Browser request checkpoints are established before the operation that emits the request; runtime values are encoded as JavaScript values, never raw string fragments.
11. Encryption emits a successful envelope only when the actual plaintext byte count equals the declared size.
12. The LXC smoke harness exits nonzero if any scenario fails.

## 2. Architecture decisions

### 2.1 Replication: durable outbox plus per-key execution serialization

The existing `MetadataStore` in `crates/metadata-store/src/lib.rs` remains synchronous rusqlite. No SQLx or new distributed lock is introduced.

Each replication job gets an explicit `object_version`, scoped by `(target, bucket, key)`. `job_id` remains the unique execution/attempt identifier. A SQLite table records the current version for each target/object and increments it in the same transaction that inserts the jobs. Existing rows are backfilled conservatively from their historical `job_id`; new rows always use the durable version table.

`ReplicationEngine` continues to own the in-memory queue, but its queue is no longer the source of truth. Gateway request code constructs jobs without publishing them, calls `MetadataStore::enqueue_jobs` successfully, then publishes the persisted jobs. Startup and a bounded periodic scan re-notify pending and recovered in-progress rows.

Workers use a fixed-size striped async lock table keyed by `(target,bucket,key)`. The lock is held across the latest-version check and the remote provider mutation. After the mutation, the worker updates status with an optimistic version check. If a process crashes after remote success and before the status update, startup converts `in_progress` back to pending; the operation is retried idempotently. A newer version always follows the older operation under the same key lock.

This is a single-gateway-process guarantee. Running multiple gateway processes against one metadata database is not a supported deployment model and remains out of scope.

### 2.2 DLQ replay

DLQ replay checks the current version within the same SQLite transaction that inserts a replay job. A stale entry returns a typed superseded result, marks the DLQ entry as superseded for operator visibility, and creates no executable job. A current entry receives a new execution `job_id` but retains the same logical object version. Batch replay selects at most one eligible current entry per `(target,bucket,key)`.

### 2.3 Overwrite recovery

The gateway uses a staged replacement protocol. New bytes are written to a reserved temporary provider key and verified before the destination is changed. Providers with a reliable native rename/move path use that path to replace the destination. Where an atomic provider replacement is unavailable, the gateway retains a recoverable backup of the previous object using bounded spool/provider backup capacity; if that guarantee cannot be met, the overwrite is rejected before destructive mutation. A failure after remote mutation records enough WAL/metadata state for restart reconciliation and never restores old encryption metadata over unknown new bytes.

The Mobile provider uses its existing native rename/move primitives to implement the same staged replacement rule rather than deleting the destination before uploading its replacement.

### 2.4 Credential and health state

`ConfiguredBackend` or an equivalent returned backend snapshot carries the credential generation captured when the backend was built. Read/write paths that record provider authentication failures pass that generation to the lease updater. The updater compares request generation, lease generation, and current credential generation before mutating state.

Lease updates distinguish `Verified`, `AuthFailure`, and `InconclusiveFailure`. Only `Verified` for the current generation clears `requires_reauth`; an inconclusive failure updates diagnostics while retaining the flag.

The data-plane `/healthz` handler returns a local success response without calling `backend.health()`. The existing metrics `/readyz` continues to perform provider health/readiness evaluation behind its control-plane authentication middleware.

## 3. Data structures and persistence contract

### 3.1 Replication data

Add to the existing `ReplicationJob` in `crates/replication-engine/src/lib.rs`:

```rust
pub object_version: u64;
```

Add a durable current-version table to `MetadataStore::init_schema`:

```sql
CREATE TABLE IF NOT EXISTS replication_object_versions (
    target TEXT NOT NULL,
    bucket TEXT NOT NULL,
    key TEXT NOT NULL,
    current_version INTEGER NOT NULL,
    updated_at_unix_ms INTEGER NOT NULL,
    PRIMARY KEY (target, bucket, key)
);
```

Add `object_version INTEGER NOT NULL DEFAULT 0` to `replication_jobs`. Add an index on `(target, bucket, key, object_version)`.

The store must provide synchronous APIs matching existing rusqlite style:

- `enqueue_jobs_with_versions(&self, jobs: &[ReplicationJob]) -> Result<Vec<ReplicationJob>, MetadataError>`: one transaction, increment each target/object version and insert jobs.
- `latest_job_for_object(...)` must continue to return the latest logical version, not merely the largest replay attempt id.
- `try_claim_job(job_id, object_version) -> Result<bool, MetadataError>`: atomically transition `pending`/due `retry_scheduled` to `in_progress`.
- `mark_job_completed_if_current(...) -> Result<bool, MetadataError>`.
- `mark_job_superseded(...) -> Result<(), MetadataError>`.
- `recover_in_progress_jobs(...)`: transition `in_progress` rows back to pending at startup.
- `replay_dead_letter_job_checked(...)` and batch equivalent: perform current-version validation in the same transaction and return a typed `ReplayOutcome`.

Persist claims in a new `replication_job_claims(job_id INTEGER PRIMARY KEY, claimed_at_unix_ms INTEGER NOT NULL)` table, not new public job status strings. Existing `ReplicationStatus` values remain unchanged. Claim insertion checks that the job is pending/due and still current; completion deletes the claim in the same transaction as the status change. Startup deletes abandoned claims before restoring pending jobs. Superseded jobs use existing `Failed` status with a stable `superseded` diagnostic and are excluded from retry/replay by the current-version guard. Keep existing snapshots and old databases readable. All public Rust struct literal call sites must be updated in the same integration change that adds `object_version`; a partially compiling schema commit is not an acceptance milestone.

### 3.2 Credential request snapshot

Do not expose token hashes or plaintext credential material. Add an internal snapshot:

```rust
#[derive(Clone)]
struct ProviderBackendSnapshot {
    provider: ProviderId,
    credential_generation: u64,
    backend: DynBackend,
}
```

The snapshot is created when selecting a backend. `note_provider_auth_failure_on_request` accepts the snapshot generation. It ignores failures from generations older than the current credential record and only mutates a lease that is still bound to the same generation.

### 3.3 Probe transition

Use an internal result classification:

```rust
enum CredentialVerificationOutcome {
    Verified,
    AuthenticationFailure,
    InconclusiveFailure,
}
```

`provider_credential_lease_summary_from_health` must preserve the distinction. `InconclusiveFailure` cannot turn an existing `requires_reauth=true` into false.

### 3.4 Replacement/orphan state

Use existing gateway WAL structures in `crates/gatewayd/src/main.rs` as the recovery journal. Extend the record only with additive fields needed to identify staged key, previous remote identity, new logical metadata, and replacement phase. If a durable orphan table is needed, add it through `MetadataStore::init_schema`; do not silently delete a remote object when its metadata transaction fails.

### 3.5 Browser values

Keep generic flow binding in `crates/blob-core/src/browser_flow.rs`. Add a distinct JavaScript-value binding path backed by `serde_json` serialization. Catalog operations must identify JavaScript value placeholders explicitly; existing plain text placeholders retain their old semantics.

### 3.6 Encryption stream state

Extend the existing internal `EncryptStreamState` in `crates/object-crypto/src/lib.rs` with:

```rust
expected_plaintext_bytes: u64,
consumed_plaintext_bytes: u64,
```

`PlaintextChunker` must reject chunks that would exceed the expected size and reject EOF before the expected size is consumed. Zero-size objects remain valid.

## 4. Functional module specifications

### Module R1: Durable replication outbox and ordering

**Files:** `crates/metadata-store/src/lib.rs`, `crates/replication-engine/src/lib.rs`, targeted replication portions of `crates/gatewayd/src/main.rs`.

**Success:** DB transaction commits versioned jobs; only then are jobs published. Worker claims one job, locks its target/object shard, rechecks current version, performs the remote operation, then commits completion conditionally.

**Failures:** DB failure means no queue publication. Queue notification loss is recovered by startup/periodic scan. Remote failure follows existing retry/dead-letter classification. Crash after remote success is safe because retry is idempotent and newer versions supersede old jobs.

**Concurrency:** same target/object serialized; different objects remain parallel.

**Tests:** old PUT/new DELETE, old DELETE/new PUT, queue publication after persistence failure, duplicate queue notification, crash recovery from `in_progress`, and version-conditional completion.

### Module R2: DLQ replay validity

**Files:** `crates/metadata-store/src/lib.rs`, DLQ handlers in `crates/gatewayd/src/main.rs`.

**Success:** only the current object version is replayed; stale entries become auditable superseded results. Batch replay is deduplicated by target/object.

**Errors:** replay races are transactionally rejected; replayed entries remain inspectable; no old operation is enqueued.

**Tests:** stale DELETE after newer PUT, multiple DLQ entries for one object, concurrent replay, batch replay ordering, and legacy records with default version.

### Module R3: Recoverable gateway overwrite

**Files:** PUT/recovery/WAL portions of `crates/gatewayd/src/main.rs`, additive metadata structures in `crates/metadata-store/src/lib.rs` if required.

**Success:** staged object is complete and verified before replacement. Metadata records the replacement phase. Recovery either finishes the new metadata/remote swap or restores the previous remote object before restoring previous metadata.

**Errors:** no silent old-metadata restoration after an un-restored overwrite. If recovery capacity is unavailable, reject before destructive overwrite.

**Tests:** failure after provider PUT, failure at each metadata persistence step, encrypted-to-encrypted replacement with different key, plaintext/encrypted transition, restart reconciliation, cleanup failure and orphan visibility.

### Module R4: Mobile family error semantics

**Files:** `crates/provider-mobile/src/lib.rs`; only gateway mapping tests if needed.

**Success:** explicit family NotFound remains NotFound; all other family discovery errors propagate unchanged.

**Tests:** 404, 401, 500, timeout, decode failure; DELETE must preserve local metadata for non-NotFound failures.

### Module R5: Mobile staged overwrite

**Files:** `crates/provider-mobile/src/lib.rs`.

**Success:** upload replacement to a unique temporary sibling, verify completion, move/rename old destination to backup, move new object into destination, then remove backup. Every failure path attempts restoration.

**Tests:** create failure, part failure, complete failure, backup move failure, final move failure, cleanup failure, and successful replacement.

### Module R6: Credential generation and conservative probe state

**Files:** credential/backend selection and lease functions in `crates/gatewayd/src/main.rs`.

**Success:** old backend errors are ignored after a newer credential generation is saved; current-generation auth errors fence; network errors do not unfence; positive current-generation verification clears the fence.

**Tests:** delayed old request after save/reset, current request 401, probe timeout after 401, successful verification after 401, fallback-provider read failure, and generation persistence/restart.

### Module R7: Local liveness and provider readiness

**Files:** `/healthz` handler and route tests in `crates/gatewayd/src/main.rs`; `deploy/lxc/smoke.sh` only if output wording needs alignment.

**Success:** data-plane healthz responds locally without upstream network. Metrics readyz remains provider-aware and authenticated.

**Tests:** delayed provider health, provider auth expiry, missing primary, empty primary list, local endpoint response bound.

### Module R8: S3 prefix preservation

**Files:** `crates/provider-unicom/src/lib.rs`, `provider-telecom`, `provider-mobile`, `provider-onedrive`; shared helper only if it can be added to `blob-core` without changing provider semantics.

**Success:** trailing slash is retained through provider list filtering; filter occurs before limit and continuation projection. `docs/` excludes `docs-archive`; `docs` keeps S3 string-prefix behavior.

**Tests:** all four providers’ pure prefix cases, repeated separators according to each provider’s existing key contract, limit, and continuation.

### Module R9: CDP request correlation

**Files:** `crates/browser-cdp/src/lib.rs`, `crates/blob-core/src/browser_flow.rs`, affected flow catalogs.

**Success:** checkpoint is created before the operation step; request wait includes requests emitted synchronously or asynchronously by that operation and excludes earlier requests.

**Tests:** immediate request, delayed request, unrelated earlier request, multiple matching requests, timeout, and affected Unicom operation flows.

### Module R10: JavaScript-safe browser values

**Files:** `crates/blob-core/src/browser_flow.rs`, affected `config/browser-flows/*.json`, tests in `blob-core`.

**Success:** runtime names are encoded as JS values or function arguments. Existing catalog fields without JS binding remain compatible.

**Tests:** apostrophe, backslash, quote, Unicode, U+2028, `</script>`, and node syntax/runtime checks.

### Module R11: Encryption declared-size validation

**Files:** `crates/object-crypto/src/lib.rs`.

**Success:** actual and declared plaintext sizes must match before successful stream completion; errors propagate through ObjectBody.

**Tests:** short, exact, long, zero, fragmented chunks, and source stream error.

### Module R12: Smoke harness failure propagation

**Files:** `deploy/lxc/_smoke_test.sh`.

**Success:** every failed case sets a global failure flag; cleanup runs via trap; final exit is nonzero if any case fails.

**Tests:** deliberate failing smoke script, healthy/degraded/dead/strict matrix, mock startup failure, and cleanup after interruption.

## 5. TDD and task order

Every coding task follows: add a focused failing test, run it and record the failure, implement the smallest specified change, rerun the focused test, run the module suite, then commit.

### Phase 0: contract tests and schema design

1. Add metadata-store tests for version allocation, DB-first insertion, claim/completion, in-progress recovery, and DLQ superseded replay. Run `cargo test -p metadata-store` and verify new tests fail before implementation.
2. Add replication-engine tests proving queue construction does not publish before the gateway persists jobs, and that duplicate notifications are harmless.
3. Add gateway pure tests for credential transitions and local healthz behavior.

### Phase 1: independent lanes

4. Object crypto: add short/exact/long/zero stream tests, then implement size state. Run `cargo test -p object-crypto`.
5. Browser CDP/flow: add checkpoint race tests, then safe JS binding tests and catalog updates. Run `cargo test -p browser-cdp -p blob-core` plus node syntax checks.
6. Prefix: add provider unit tests for the four provider adapters, then preserve trailing slash and filtering order. Run each provider crate test suite.
7. Mobile: add family error and staged-overwrite tests using provider-local mock HTTP responses, then implement. Run `cargo test -p provider-mobile`.
8. Smoke: add a deliberate-failure harness scenario/test seam, then implement failure accumulation and cleanup. Run `bash deploy/lxc/_smoke_test.sh` and assert its exit code.

### Phase 2: shared gateway lanes

9. Replication/outbox/DLQ: implement metadata schema and APIs, then engine queue publication/recovery, then gateway call-site ordering and worker locks. Run metadata/replication suites and targeted gateway replication tests.
10. Gateway overwrite recovery: add injected provider/store failure tests, then implement staged/WAL recovery. Run targeted gateway PUT tests and metadata tests.
11. Credentials/health: add delayed backend and generation race tests, then implement snapshot propagation, sticky probe transitions, and local `/healthz`. Run targeted gateway tests and metrics tests.

### Phase 3: integration and acceptance

12. Add cross-module tests for PUT → persisted replication → worker recovery, stale DLQ replay, Mobile delete failure, credential fencing, and delayed provider health.
13. Run all workspace unit tests, shell syntax checks, smoke matrix, Admin frontend harness, and `git diff --check`.
14. Have an allowed review model inspect each grouped diff and the final integration diff. Resolve findings before deployment. No real provider mutation is part of local acceptance.

## 6. Parallel execution plan

Parallel work is permitted only when the file/interface boundary is disjoint:

- Lane A: `object-crypto` — `agnes-3.0-flash` or `sensenova-6.8-flash-lite`.
- Lane B: `browser-cdp` + `blob-core` + flow catalog — `deepseek-flash`.
- Lane C: provider prefix files — `sensenova-6.8-flash-lite`.
- Lane D: `provider-mobile` — `deepseek-flash`.
- Lane E: `deploy/lxc/_smoke_test.sh` — `agnes-3.0-flash`.

The following lanes must not edit `crates/gatewayd/src/main.rs` or `crates/metadata-store/src/lib.rs` concurrently:

- Lane F: replication/outbox/DLQ — `deepseek-v4.1-flash` through the configured `.44` endpoint, or another permitted low-tier coding endpoint if that route is unavailable.
- Lane G: gateway overwrite recovery — `deepseek-flash`.
- Lane H: credentials/health — `sensenova-6.8-flash-lite`.

Merge order is A–E in parallel, then F, then G, then H, with focused tests after each merge. If a lane needs a shared contract, stop and return to the main architecture owner; do not let a coding model redefine the contract.

Code review models are restricted to `glm-5.3` or `kimi-k3`. Use one grouped review after A–E and one final review after F–H/integration. Review requests must contain only the relevant diff and spec section, and must verify exact source paths and tests.

## 7. Rollout, migration, and rollback

- All SQLite changes are additive and handled through the existing `MetadataStore::init_schema` compatibility path.
- Existing replication rows receive a deterministic legacy version; no existing job or DLQ audit row is deleted.
- On startup, old pending and in-progress-compatible rows are recovered before workers begin.
- New staged keys use a reserved gateway-managed namespace and are excluded from normal object listings.
- A feature flag is not required for pure correctness fixes. If an unsafe provider cannot satisfy staged replacement, fail closed for that overwrite rather than silently using the old destructive behavior.
- Commits are split by module and each commit has its own test evidence. Deployment is not performed until the final review and local acceptance pass.

## 8. Non-goals and residual risks

- No multi-process distributed replication lock.
- No new provider trait redesign.
- No automatic credential refresh for carrier providers.
- No real carrier write/delete recovery drill in local tests.
- Provider-specific remote APIs may still be non-transactional; staged replacement and explicit partial-commit state reduce but cannot eliminate upstream failure windows.
- The existing gatewayd `main.rs` remains large; extracting unrelated modules is out of scope.
