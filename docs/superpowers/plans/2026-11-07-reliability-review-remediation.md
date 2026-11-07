# CCBG Reliability Review Remediation Implementation Plan

> **For agentic workers:** Use subagent-driven-development with the explicit model and file ownership below. This plan is the task checklist; the associated design document defines architecture. Implementers must not choose a different architecture.

**Goal:** Fix the 14 reliability findings using test-first, independently reviewable changes.

**Architecture:** Keep rusqlite and existing BlobBackend. Durable target/object versions, persist-before-publish ordering, and process-local serialization govern replication. Recoverable replacement protects bytes/metadata; generation snapshots and conservative probe transitions protect credentials. Browser, prefix, crypto, and smoke fixes remain independent modules.

**Tech Stack:** Rust 2024, Tokio, Axum, rusqlite, serde_json, existing WAL and provider mocks, Python/Bash, Node.

## Global constraints

- Design: `docs/superpowers/specs/2026-11-07-reliability-review-remediation-design.md`.
- Preserve BlobBackend and synchronous rusqlite; no SQLx, new distributed service, global error hierarchy, or unrelated extraction of gatewayd.
- Only one gateway process may operate a metadata database. Per-key locking is process-local.
- Low-tier models implement frozen contracts. Architecture/interface questions return to the controller; do not silently improvise.
- Coding: SenseNova Lite, DeepSeek Flash, Agnes Flash, or the other user-approved coding routes. Review: only GLM-5.3 or Kimi-K3.
- Never send secrets, user configuration, credential files, or live provider responses in model packages. Source/test excerpts are permitted.
- Every behavior change requires a test-only change, a recorded expected failure, a minimal production change, and passing focused/regression tests.
- A command accepts one Cargo test filter. Run separate commands for separate filters; never mistake a rejected command or zero tests for a red/green result.
- No provider mutations, deployments, pushes, or remote publication are part of this execution.
- Coding workers do not commit/push or switch the shared branch. The controller integrates explicitly owned files and records evidence.
- Use isolated worktrees for parallel edits to overlapping files. In the shared workspace, only disjoint file owners may run concurrently.

## Execution ledger and ownership

The persistent status ledger is `docs/reliability-remediation-todo.md`. Record task/model, source baseline, red/green commands and results, review result, and integration state there. A test-passing implementation is not marked accepted until an allowed review model passes it.

Initial parallel lanes:

| Lane | Ownership | Tasks | Model |
|---|---|---|---|
| crypto | `crates/object-crypto/src/lib.rs` | T09 | SenseNova Lite |
| smoke | `deploy/lxc/_smoke_test.sh`, `scripts/test-lxc-smoke-harness.py` | T10 | Agnes Flash or DeepSeek Flash |
| browser | `crates/browser-cdp/src/lib.rs`, `crates/blob-core/src/browser_flow.rs`, affected browser-flow catalogs | T08 | DeepSeek Flash |
| mobile | `crates/provider-mobile/src/lib.rs` | T05 | DeepSeek Flash |
| prefix | Unicom, Telecom, OneDrive adapter files; Mobile prefix change waits for mobile owner | T07 | SenseNova Lite |

Gateway/metadata lanes are serialized: T01 → T02/T03 integration → T04 → T06 → T11. Do not concurrently edit gatewayd main.rs or metadata-store lib.rs. Prefix/Mobile share one file, so Mobile prefix integration waits. Browser/prefix do not share blob-core: prefix is fixed locally in each adapter and introduces no shared public helper.

## T01 — Durable replication contracts

**Owner files:** replication-engine lib.rs; metadata-store lib.rs; all workspace `ReplicationJob` literals for mechanical compatibility only.

**Frozen contracts:**

- Add `#[serde(default)] pub object_version: u64` to existing `ReplicationJob`.
- Add `replication_object_versions` with `(target,bucket,key)` primary key, `current_version`, `updated_at_unix_ms`.
- Add `object_version INTEGER NOT NULL DEFAULT 0` to replication_jobs and index target/bucket/key/version/job_id.
- Add `replication_job_claims(job_id INTEGER PRIMARY KEY, claimed_at_unix_ms INTEGER NOT NULL)`; preserve existing public ReplicationStatus strings.
- Backfill legacy job versions deterministically from job_id once. Seed current versions by maximum persisted version for each tuple; retain tombstones through retention and restore/export.
- `enqueue_jobs_with_versions(&self, jobs: &[ReplicationJob]) -> Result<Vec<ReplicationJob>, MetadataError>` increments tuple versions and inserts rows in one transaction. Retry/save of an existing attempt never increments logical version.
- Checked replay creates a new job_id with the same object_version only after validating the current version transactionally.
- Claims are exclusive per attempt. Startup clears abandoned claims. Completion/retry/failure removes the claim transactionally.

- [ ] RED: add `versioned_enqueue_allocates_per_target_object`, `legacy_jobs_backfill_versions_on_reopen`, `failed_enqueue_rolls_back_version_and_job`, `job_claim_is_exclusive`, `startup_releases_abandoned_claims`, `metadata_export_restore_preserves_current_versions`. Use existing MetadataStore::open(":memory:")/temporary-file fixtures and real SQL; no empty test bodies.
- [ ] Run `cargo test -p metadata-store` and `cargo test -p replication-engine`; capture failing test names and expected assertion failures before production edits.
- [ ] GREEN: implement additive schema, row mappings, serialization, version/claim methods; update every literal in one compiling integration.
- [ ] Run both crate suites, then `cargo check --workspace`; all must pass before gateway behaviour depends on the new API.
- [ ] Allowed model reviews migration, retention, backup round-trip, and version allocation before acceptance.

## T02 — Persist first and serialize replication

**Owner files:** replication-engine lib.rs; gatewayd main.rs; metadata-store APIs from T01.

**Contracts:**

- Split construction from publication. New `prepare_put`/`prepare_delete` methods accept the same arguments as existing enqueue methods and return unpublished jobs; explicit publication accepts only persisted/versioned jobs.
- Use 64 Tokio mutex stripes in AppState, hashed from full target/bucket/key. A worker acquires its stripe before checking current version and holds it through remote operation and status persistence. SQLite/std mutexes are never held across await.
- Skip obsolete attempts before remote I/O. A later operation waits behind an already executing old operation; generation checks after remote I/O cannot replace this lock.
- DB-first order also applies to retry/dead-letter and admin/reconcile/copy paths. Startup and bounded periodic recovery can republish missing pending attempts; claim guards make duplicate hints harmless.

- [ ] RED: deterministic barriers reproduce `older_put_then_new_delete_finishes_deleted`, `older_delete_then_new_put_finishes_new`, `enqueue_store_failure_publishes_no_job`, `duplicate_hint_executes_once`, `committed_pending_job_recovers_without_notification`.
- [ ] Run `cargo test -p gatewayd replication -- --test-threads=1` and `cargo test -p replication-engine`; verify actual failures, not network timeouts.
- [ ] GREEN: wire prepared jobs → store transaction → publish, striped locks, claims and recovery. Keep source errors/retry policy unchanged.
- [ ] Run metadata/engine suites and gateway replication tests. No worker may observe an unpersisted job.
- [ ] Review ordering proof and crash window evidence with GLM-5.3/Kimi-K3.

## T03 — Reject obsolete DLQ replay

**Owner files:** metadata-store replay methods; gatewayd DLQ endpoints/tests.

- [ ] RED: `dlq_old_delete_after_new_put_is_rejected`, `dlq_batch_selects_current_version_once`, `dlq_replay_retains_logical_version`, `concurrent_dlq_replay_has_one_winner`.
- [ ] Run `cargo test -p metadata-store dead_letter` and `cargo test -p gatewayd replication_dlq -- --test-threads=1`.
- [ ] GREEN: validate current durable version inside replay transaction; stale single replay returns a typed conflict, creates no executable row, and retains audit. Batch skips stale tuples and returns auditable skipped outcomes; add no provider side effects.
- [ ] Replay attempt gets a new ID and retains original object_version; task snapshots and fallback gates use logical version plus attempt ID.
- [ ] Run both suites and review stale DELETE/new PUT example including retention and already-replayed cases.

## T04 — Recoverable overwrite

**Owner files:** gatewayd PUT/WAL/rollback helpers; additive metadata recovery records if needed. Depends on T01–T03.

**Required decision boundaries:** do not silently assume rename overwrites atomically. For a provider without confirmed safe replacement, preserve original bytes in bounded spool/backup before any destructive step; reject preflight when backup/reservation is unavailable. Never delete a successfully overwritten object and claim that old bytes were restored. Keep pending replacement journal before mutation and reconcile it before normal startup service. Reserve staging keys under the managed namespace and hide them from normal lists.

- [ ] RED: use existing gateway injected-store-failure gates to fail protection-plan save, logical save, replication enqueue, remote replacement, restoration, and cleanup. Assert old bytes+metadata or explicit recoverable partial commit; test changing encryption key/profile and plaintext↔encrypted overwrites.
- [ ] Run `cargo test -p gatewayd put_rollback -- --test-threads=1` and `cargo test -p gatewayd gateway_write_ahead_log -- --test-threads=1`.
- [ ] GREEN: stage and verify replacement, persist recoverable journal, replace/restore using confirmed provider capabilities, then finalize matching metadata atomically. Keep recovery material until final commit and record cleanup failure explicitly.
- [ ] Add reopen/restart recovery tests for each persisted phase. Ensure bounded disk admission rejects before touching the original.
- [ ] Run focused tests and metadata backup/restore suite; have an allowed reviewer validate actual bytes, not only restored rows.

## T05 — Mobile correctness

**Owner:** provider-mobile lib.rs. Family propagation is an independent first subtask; staged replacement follows with the same owner. Do not edit gatewayd in parallel.

- [ ] RED: `family_timeout_is_not_not_found`, `family_auth_failure_is_not_not_found`, `family_malformed_response_propagates`, `missing_family_root_is_not_found`. Call real adapter/root-resolution/DELETE with the existing mock HTTP server and assert the returned BlobError class and whether delete was attempted.
- [ ] Run `cargo test -p provider-mobile family`; record failures caused by `.await.ok()` suppression.
- [ ] GREEN: object operations propagate disk-info errors; optional health/discovery can continue omitting unavailable family scope. Absence of a configured family root remains explicit NotFound; an HTTP error is not assumed to mean object absence.
- [ ] RED: simulate existing-object create response followed by failure at upload/create/complete/backup move/final rename/cleanup. Assert original remote bytes remain accessible; include streaming and spool branches.
- [ ] Run `cargo test -p provider-mobile overwrite`; capture old destructive overwrite failure.
- [ ] GREEN: upload temporary sibling, complete and validate, back up destination before swapping, restore on failure. If confirmed primitives cannot preserve the old object, reject overwrite preflight instead of delete-before-upload. Do not leave temporary names in ordinary list results.
- [ ] Run `cargo test -p provider-mobile`; review full mock request trace and restoration cases.

## T06 — Credential state and local liveness

**Owner:** gatewayd main.rs. Serialize after shared gateway integration.

- [x] RED: `stale_request_generation_does_not_fence_new_credentials`, `current_request_generation_fences`, `network_probe_preserves_reauth`, `verified_probe_clears_current_reauth`, `fallback_auth_error_uses_selected_backend_generation`, `local_healthz_does_not_call_backend`.
- [x] Run `cargo test -p gatewayd credential -- --test-threads=1`, `cargo test -p gatewayd reauth -- --test-threads=1`, and `cargo test -p gatewayd healthz -- --test-threads=1`. Use local fake backends, not carrier endpoints; report existing unrelated network-dependent test timeouts separately.
- [x] GREEN: snapshot generation alongside actual registry backend construction/lookup; carry it through resolved read and write contexts. Guard lease read-modify-write and credential save using the same provider-state synchronization so check→persist cannot race.
- [x] Use Verified/AuthFailure/Inconclusive classifications. Inconclusive preserves previous reauth; only current-generation positive auth clears it. Do not classify generic Degraded as positive without evidence of successful authentication.
- [x] Make data-plane healthz local 200. Keep metrics extended healthz and readyz provider-aware and protected by existing control API key middleware.
- [x] Run focused tests and metrics/readiness tests. Review save→registry swap→late failure schedules and generation-guard atomicity.

## T07 — Exact S3 prefix semantics

**Owner:** each provider's existing list implementation. Unicom/Telecom/OneDrive can run before Mobile owner releases its file; Mobile portion waits.

- [ ] RED: per adapter, list two keys `docs/a.txt` and `docs-archive/a.txt` with prefix `docs/`, then assert only `docs/a.txt`; with prefix `docs`, assert both. Add limit=1, empty prefix, non-ASCII, and trailing slash cases through actual list paths/mock pages.
- [ ] Run `cargo test -p provider-unicom prefix`, `cargo test -p provider-telecom prefix`, `cargo test -p provider-onedrive prefix`, then `cargo test -p provider-mobile prefix` after ownership release.
- [ ] GREEN: preserve the actual requested prefix through matching; do not reuse object-path normalization that removes trailing/repeated separators. Filter before per-provider limit. Preserve normal S3 starts-with semantics, not directory-boundary semantics for `docs`.
- [ ] Run changed provider suites; integrate Mobile last. Review continuation/limit and hidden staging namespace interaction with T04/T05.

## T08 — Browser request correlation and safe JS values

**Owner:** browser-cdp lib.rs; blob-core browser_flow.rs; affected browser-flow JSON files.

- [ ] RED: checkpoint before immediate action request captures it; pre-action matching request is excluded; delayed request and two consecutive operation/wait pairs use their own checkpoints. Use the existing CDP/mock transport, no real browser login.
- [ ] Run `cargo test -p browser-cdp` and `cargo test -p blob-core`; verify post-action checkpoint tests fail.
- [ ] GREEN: establish checkpoint in the action method before dispatch; following wait consumes that action checkpoint. Do not reset the floor when a completed action's request is already buffered. Standalone waits retain documented current-time behavior.
- [ ] RED: bind apostrophe, double quote, backslash, CR/LF, Unicode/U+2028 and `</script>` into the actual Unicom expressions; JSON-encoded value must round-trip without executable injection.
- [ ] Run focused binding tests. If Node is used, feed generated script through stdin using `node --check` for syntax and a subprocess execution asserting the evaluated string; never leave a generated-script placeholder command.
- [ ] GREEN: explicit JavaScript value placeholder encoding in the existing binder; remove surrounding quotes from migrated catalog expressions so a value is serialized exactly once. Plain text, selectors, URLs and other scripts retain existing substitution unless explicitly opted in.
- [ ] Run both suites and parse every changed JSON catalog. Review checkpoint state isolation and complete affected placeholder coverage.

## T09 — Encrypted plaintext length

**Owner:** object-crypto lib.rs only. Frozen envelope format stays CCBGENC1; no new dependencies/types in the public API.

- [ ] RED: add `encrypt_rejects_short_plaintext`, `encrypt_rejects_long_plaintext`, `encrypt_rejects_nonempty_zero_size`, `encrypt_validates_fragmented_stream_size`, `encrypt_propagates_source_error`. Drain the public encrypted ObjectBody and assert errors rather than testing private counters. Preserve exact/zero roundtrips.
- [ ] Run `cargo test -p object-crypto encrypt_rejects` and capture current successful drain where an error is expected.
- [ ] GREEN: extend existing EncryptStreamState with expected/consumed sizes and a terminal error state. Before encrypting each chunk check checked-add/remaining length; at EOF compare consumed with expected. An error is emitted once, then stream ends. Never silently clamp or truncate bytes.
- [ ] Run `cargo test -p object-crypto`; keep all 26 baseline tests passing and add fragmented, overflow/truncation, source-error checks.
- [ ] Review stream termination, authenticated metadata and envelope compatibility.

## T10 — Smoke harness exit semantics

**Owner:** deploy/lxc/_smoke_test.sh plus new scripts/test-lxc-smoke-harness.py. The test script uses Python stdlib/tempfile only and never edits production smoke.sh.

- [ ] RED: new Python regression test copies the existing harness and an intentionally failing smoke.sh into a temporary directory, invokes Bash, asserts that FAIL output produces nonzero status. Ensure this assertion fails with the current harness. Also exercise a deterministic success double selected by endpoint/strict flag.
- [ ] Run `wsl -d Debian -- python3 /mnt/d/workspaces/ccbg/scripts/test-lxc-smoke-harness.py`; capture expected failure before changing the harness.
- [ ] GREEN: accumulate failures in every scenario, including explicit dead-process case; abort/count mock startup failures; use EXIT/INT/TERM traps to stop the harness-owned mock and delete only its mktemp directory. Keep default degraded/strict behavior unchanged.
- [ ] Run the Python regression test, `bash -n` on both LXC scripts, and real `bash deploy/lxc/_smoke_test.sh` under Linux PATH. Real matrix must be 5/5 and injection must fail reliably. A successful double is not a substitute for the real matrix.
- [ ] Review cleanup, signal exit status, no false positives on startup failure, and final exit propagation.

## T11 — Cross-module verification and review

- [ ] Tests cover persisted PUT → replication → restart, stale DLQ DELETE after newer PUT, Mobile auth error preserving metadata, old credential request ignored, local liveness during slow upstream, encrypted overwrite recovery.
- [ ] Run `cargo test --workspace` with enough wall time; report any network-dependent hangs explicitly. Focused pure tests must not depend on real carriers.
- [ ] Run `node scripts/test-admin-object-batch-state.mjs`, LXC matrix/regression tests, JSON catalog parsing and `git diff --check`.
- [ ] GLM-5.3/Kimi-K3 reviews each complete task package for spec compliance and defects. Low-tier models cannot sign off code review.
- [ ] After code-review fixes, run only changed and affected tests, then final integrated gate. Record actual model, completion status and limitations.
- [ ] Update persistent ledger with acceptance or unresolved findings for all 14 review issues. Do not mark partial work complete or deploy before acceptance.

## Completion/commit policy

Documentation is reviewable before coding. Local task commits may be made by the controller after tests and review; no force changes or published history rewrites. Deployment and remote publication remain separate from this authorized local implementation. Preserve task worktrees until integrated and do not delete unexpected files.
