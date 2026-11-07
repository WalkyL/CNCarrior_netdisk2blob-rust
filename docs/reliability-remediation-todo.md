# Reliability remediation — execution ledger

Architecture owner: Kimi-K3 (verified direct Responses completion), calibrated to actual rusqlite/workspace APIs; architecture approved by user on 2026-11-07.

- Design: [frozen spec](superpowers/specs/2026-11-07-reliability-review-remediation-design.md).
- TDD tasks: [implementation plan](superpowers/plans/2026-11-07-reliability-review-remediation.md).
- Source baseline before implementation: `175e5d1`.
- Coding only: user-approved low-tier models; independent code review only GLM-5.3 or Kimi-K3.
- No remote deployment/publication or real carrier mutations authorized as part of local acceptance.

## Tasks

| ID | Deliverable | Dependencies | State |
|---|---|---|---|
| T01 | Durable versions/claims/schema and backup compatibility | architecture | complete |
| T02 | DB-first enqueue/retry, striped worker locks, recovery | T01 | complete |
| T03 | Checked DLQ replay and API outcomes | T01, T02 | complete |
| T04 | Recoverable overwrite/staging/WAL | T01–T03 | complete |
| T05a | Mobile family error propagation | architecture | complete |
| T05b | Mobile non-destructive staged overwrite | T05a | complete |
| T06 | Request backend generation, sticky reauth, local liveness | gateway owner released | complete |
| T07 | Four-provider exact list prefix preservation | Mobile file waits T05 | complete |
| T08a | Action-scoped CDP checkpoints | architecture | complete |
| T08b | Explicit JavaScript value binding/catalog migration | T08a | complete |
| T09 | Encryption declared-size validation | architecture | complete |
| T10 | Smoke harness failure propagation/regression | architecture | complete |
| T11 | Integration acceptance and allowed-model reviews | completed tasks | complete |

## Coverage map

Review findings 1/3 → T01/T02; 2 → T03; 4 → T04; 5/6 → T05a/T05b; 7/8/9 → T06; 10 → T07; 11/12 → T08a/T08b; 13 → T09; 14 → T10.

## Evidence log

### Documentation phase

- Spec and detailed TDD plan written. Self-check corrected invalid multiple-filter Cargo commands, imaginary fixtures/paths, public status ambiguity, and the Mobile/prefix shared-file ownership conflict.
- Existing baseline evidence from review: core six crates 141 tests passed; gateway failure-feedback/readiness seven tests passed; smoke real matrix 5/5 and Admin batch-state harness passed.
- Broad gateway `credential` run exceeded 120 seconds at a Telecom credentials test; not a full successful baseline.
- Architecture/model calls: Kimi subagent failed with client `annotations` serialization; documented direct Responses call completed successfully. Do not repeat the broken subagent path or treat the failed call as design evidence.

### Per-task report format

Each task appends: model/route, owned files, RED command and failing assertion, GREEN command and counts, independent reviewer/model/result, integration status and remaining concerns. A task is accepted only after green tests and allowed-model review; implementation-only progress remains unaccepted.

### T07 — Four-provider exact list prefix preservation

- Route: direct implementation (model TEST-DIFF was worthless, re-implemented starts_with on a local array; PROD-DIFF second hunks did not match).
- Owned files: `crates/provider-unicom/src/lib.rs`, `crates/provider-telecom/src/lib.rs`, `crates/provider-onedrive/src/lib.rs`.
- Change: stop normalizing `request.prefix` (the `docs/` trailing slash was being trimmed to `docs`); pass the raw prefix to both the directory-walk predicate and the leaf `object_key.starts_with`. For Unicom, `directory_may_contain_prefix` now receives the raw prefix.
- Tests: added `list_objects_prefix_is_segment_boundary_aware` in each crate, seeding a `docs-archive` sibling directory (via mock fixtures / real put_object) and asserting: `docs/` returns only `docs/*`; `docs` (raw substring) also returns `docs-archive/*`; `docs-archive/` returns only `docs-archive/*`; partial file prefix still works.
- GREEN: `cargo test -p provider-unicom --lib` = 29 passed; `-p provider-telecom --lib` = 21 passed (one fixture ordering assertion updated: `root_entry_count=3` → `4`); `-p provider-onedrive --lib` = 51 passed.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T07/T05/T10 package; no finding specifically against the prefix adapters (findings were on the LXC harness and provider-mobile). Prefix preservation confirmed compliant.
- Integration: committed `95006a7`. Remaining concern: none functional; Mobile list still uses its own path and waits on T05.

### T09 — Encryption declared-size validation

- Route: `agnes-3.0-flash` with focused excerpt (initial `sensenova-6.8-flash-lite` returned empty diffs on the 53KB source).
- Owned files: `crates/object-crypto/src/lib.rs`.
- Change: `EncryptStreamState` gains `expected_plaintext_size`, `consumed_plaintext`, `terminal_error`, `terminated`; the `encrypt_upload` unfold closure checks-adds per chunk and, at EOF, emits a single terminal `BlobError::BodyStream` when the consumed byte count differs from the declared `plaintext_size`. The `terminated` flag guarantees the stream yields `None` after the single terminal/source error instead of re-emitting it on every subsequent poll.
- Tests: 9 new (`encrypt_rejects_short_plaintext`, `encrypt_rejects_long_plaintext`, `encrypt_rejects_nonempty_zero_size`, `encrypt_validates_fragmented_stream_size`, `encrypt_propagates_source_error`, `encrypt_exact_size_roundtrip`, `encrypt_zero_size_roundtrip`, `encrypt_terminal_length_error_emitted_exactly_once`, `encrypt_source_error_emitted_exactly_once`); 8 baseline fixtures corrected from `plaintext_size: 32` to the actual short plaintext length.
- GREEN: `cargo test -p object-crypto` = 35 passed.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T09 package and requested changes: the stream re-emitted the terminal length/source error on every subsequent poll (confirmed livelock — 62 errors in 64 polls in RED). Fixed with a `terminated` flag; two new RED tests (`encrypt_terminal_length_error_emitted_exactly_once`, `encrypt_source_error_emitted_exactly_once`) now pass; full suite 35 passed.
- Integration: committed `22bb34c` + review-fix `1fe7ed6`.

### T08a — Action-scoped CDP checkpoints

- Route: direct implementation (prior model PROD-DIFF set the checkpoint after dispatch, still missing the just-sent request; TEST-DIFF referenced a nonexistent session).
- Owned files: `crates/browser-cdp/src/lib.rs`, `crates/blob-core/src/browser_flow.rs`.
- Change: `CdpEventState` gains `request_floor_sequence: Option<u64>`; `mark_request_checkpoint()` records the current sequence before an action dispatches; `take_request_floor()` consumes it (falling back to the documented current-time floor for standalone waits). `BrowserFlowSession::mark_action_request_checkpoint` is a default no-op so non-CDP sessions and the test mock are unchanged; the executor calls it before navigate/click/set_input/invoke_operation/set_files/dispatch_events.
- Tests: `request_checkpoint_captures_action_request_and_excludes_preaction`, `request_checkpoint_consumed_by_following_wait`, `consecutive_action_wait_pairs_use_own_checkpoints`.
- GREEN: `cargo test -p browser-cdp --lib` = 25 passed; `cargo test -p blob-core --lib` = 38 passed.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T08 package and requested changes. Verified findings: sequence-floor math (#2) was a false positive (`next_request_sequence` is incremented then assigned, so the floor equals the last buffered sequence); "other scripts JSON-encoded" (#6/#7/#9) false positive because `BrowserFlowOperationKind` has a single `Javascript` variant and all other catalog sources route through `bind_string_template`; within-call floor retention (#3) is correct because the wait holds `start_sequence` for its whole loop. Addressed the legitimate gaps: added `consecutive_action_wait_pairs_use_own_checkpoints` (each pair gets its own floor, no leak) and `bind_javascript_values_cover_js_string_escape_set` (apostrophe/quote/backslash/CR-LF/U+2028/`</script>` round-trip as one JSON-encoded literal). A true end-to-end mock-CDP-transport test is not added because browser-cdp has no mock transport (only `CdpEventState` unit tests); recorded as a limitation.
- Integration: committed `6636f5d` + review-fix `70a6e5f`.

### T08b — Explicit JavaScript value binding/catalog migration

- Route: direct implementation.
- Owned files: `crates/blob-core/src/browser_flow.rs`, `config/browser-flows/unicom-web.json`.
- Change: add `bind_javascript_template` which serializes each placeholder via `serde_json::to_string` (quoted+escaped JS string literal; bare literals for numbers/bools/null); `bind_operation` uses it for `operation.source`. Migrate the 5 Unicom JS expressions so `'{{inputs.x}}'` becomes `{{inputs.x}}` (quotes removed; JSON encoding supplies them exactly once).
- Tests: `bind_operation_encodes_javascript_values_without_injection` (payload `'); alert('pwned'); //` must stay inside one quoted literal), `bind_javascript_values_cover_js_string_escape_set` (apostrophe, double quote, backslash, CR/LF, U+2028, `</script>`).
- GREEN: `cargo test -p blob-core --lib` = 38 passed.
- Review: GLM-5.3 reviewed with T08a (see T08a). The escape-set coverage test was added in response to the requested-changes review.
- Integration: committed `8a115ce` + review-fix `70a6e5f`.

### T10 — Smoke harness exit semantics

- Route: direct implementation.
- Owned files: `deploy/lxc/_smoke_test.sh`, `scripts/test-lxc-smoke-harness.py`.
- Change: add a `FAILURES` accumulator flipped by every failing case and by mock startup failures; exit 1 when nonzero. `cleanup` trap on EXIT/INT/TERM stops the harness-owned mock and removes only the harness mktemp dir. `readyz` probe sends the `x-api-key` header only when a control API key is configured (an empty header was a regression vs the original conditional header). The dead-process case skips the smoke run if mock startup fails.
- Tests: new `scripts/test-lxc-smoke-harness.py` (copies harness + failing smoke.sh double into a temp dir, invokes Bash, asserts FAIL → nonzero; plus a deterministic success double selected by endpoint/strict flag). RED against the old harness (printed FAIL but exited 0); GREEN now.
- GREEN: Python regression under WSL Debian passed (both failing-double and success-double scenarios); `bash -n` on both LXC scripts; real `bash deploy/lxc/_smoke_test.sh` = 5/5 with `RESULT: PASS`.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T07/T05/T10 package. Addressed: success-double scenario added to the Python regression (was missing), conditional `x-api-key` header restored in smoke.sh (confirmed regression vs original), dead-case startup-failure guard added. Reviewer's 503-classification finding was a false positive (503 falls to the `unexpected` branch and fails). The PATH-hardcode finding is a documented WSL-specific workaround retained as a limitation.
- Integration: committed `124de30` + review-fix `ad90681`.

### T05a — Mobile family error propagation (finding #5)

- Route: direct implementation.
- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `native_root_folder_id_for_container` no longer resolves the family root via `family_disk_info_if_available().await.ok()` (which swallowed timeout/401/500 into `NotFound`). New `family_disk_info()` helper returns `Ok(None)` only for the explicit `success:false` code `5010` ("family not enabled") and `Err` for transport failures AND for any other business-level `success=false` (transient upstream rejection), which object operations now propagate. Removed the dead `.ok()` wrapper.
- Tests: `family_transient_error_is_not_masked_as_not_found` (500 on `getFamilyDiskInfo` → `list_objects` must not be `NotFound`), `family_business_rejection_is_not_masked_as_not_found` (success=false code `5030` must propagate, not be masked as absence).
- GREEN: `cargo test -p provider-mobile --lib` = 32 passed; family group covers the transport-500 and business-5030 rejection cases plus the 5010 disabled case.
- Review: GLM-5.3 reviewed with T07/T05/T10. The reviewer flagged that any `success=false` was masked as absence; fixed by matching only the `5010` code and propagating other business rejections. The new `family_business_rejection_is_not_masked_as_not_found` test locks this in.
- Integration: committed `575c763` + review-fix `ad90681`.

### T05b — Mobile non-destructive staged overwrite (finding #6)

- Route: direct implementation.
- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `create_mobile_upload` no longer deletes existing same-name children before uploading the replacement. When `file/create` reports an existing object without an uploadId, each same-name sibling is renamed to a hidden `.ccbg-staging-<nonce>-<name>` via `file_update` before `file/create` is retried at the original name. The staged backups are returned with the upload plan; `put_object` discards them (delete) only after the replacement is uploaded and completed, and renames them back to their original names if create/upload/complete fails. `delete_child_files_by_name` is removed. `list_scope_objects` hides staging-named siblings so they never surface in ordinary list results.
- Tests: `overwrite_restores_staged_old_object_when_complete_fails` (complete error → original renamed back and still reachable, no delete), `put_object_retries_after_existing_object_blocks_mobile_create_upload_id` (asserts rename-backup precedes retried create, delete follows complete, staging prefix), `list_objects_excludes_staged_overwrite_siblings`.
- GREEN: `cargo test -p provider-mobile --lib` = 32 passed; `cargo build -p provider-mobile` clean; `cargo check --workspace` clean.
- Review: GLM-5.3 reviewed with T07/T05/T10. The reviewer flagged that a staged-backup delete failure on the success path flipped a successful upload into an error; fixed by making `discard_staged_backups` best-effort on the success path (matching `restore_staged_backups`), so cleanup failure never masks the replacement upload's success.
- Integration: committed `134b270` + review-fix `ad90681`. Remaining concern: best-effort restore failure leaves a hidden staging sibling (never surfaced in listings); a backup delete failure after success leaves an invisible orphan (now logged, not fatal).

### T07 (Mobile portion) — Mobile exact list prefix preservation (finding #10)

- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `list_scope_objects` uses raw `request.prefix` (no `normalize_object_key`) for both the directory walk and the leaf `starts_with`; `directory_may_contain_prefix` aligned with S3 raw-substring semantics so prefix `docs` also reaches `docs-archive` while `docs/` stays isolated.
- Tests: `list_objects_prefix_is_segment_boundary_aware` (docs + docs-archive nested under the managed root).
- GREEN: `cargo test -p provider-mobile --lib` = 29 passed (prefix); 32 full suite after T05 fixes.
- Review: GLM-5.3 reviewed with T07/T05/T10; no finding against the Mobile prefix preservation (the reviewer's prefix finding was about the LXC PATH and provider-mobile family/cleanup). Prefix preservation confirmed compliant.
- Integration: committed `6421343` + review-fix `ad90681`. Finding #10 now complete across Unicom, Telecom, OneDrive, and Mobile.

### T06 — Credential generation, sticky reauth, local liveness (findings #7/#8/#9)

- Route: direct implementation.
- Owned files: `crates/gatewayd/src/main.rs`.
- Change: `ConfiguredBackend` gains `credential_generation` captured when the backend is built/registered; `current_primary_backend` returns it, `ReadSource` carries it, `select_home_write_provider`/`eligible_write_candidates_for_object` carry it, and `note_provider_auth_failure_on_request` takes `request_generation` and ignores failures from a generation older than the current credential record. Probe classification adds `CredentialVerificationOutcome::{Verified, AuthenticationFailure, InconclusiveFailure}`; `provider_credential_lease_summary_from_health` returns it (Degraded → InconclusiveFailure). `maybe_probe_provider_credential_lease` preserves a sticky `requires_reauth=true` on InconclusiveFailure and only clears it on Verified for the current generation. Data-plane `/healthz` now returns a local Healthy payload without calling `backend.health()`; metrics `/readyz` and extended `/healthz` remain provider-aware and authenticated.
- Tests (RED → GREEN): `network_probe_preserves_reauth` (network-error probe must not clear a sticky `requires_reauth=true`), `verified_probe_clears_current_reauth`, `stale_probe_generation_does_not_mutate_lease` (credential save fired inside `health()` while the probe is in flight; stale result discarded), `stale_request_generation_does_not_fence_new_credentials`, `current_request_generation_fences`, `fallback_read_carries_selected_backend_generation`, `local_healthz_does_not_call_backend`. Existing `note_provider_auth_failure_*` tests updated for the new signature.
- GREEN: `cargo test -p gatewayd probe` = 24 passed; `cargo test -p gatewayd generation` = 6 passed; `cargo test -p gatewayd healthz` = 1 passed; full `cargo test -p gatewayd` = 426 passed, 1 failed (`notify_webhook_posts_only_when_alert_state_changes` — one of the three documented pre-existing baseline failures, reproduced on `0f52dc9`; unrelated to T06).
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T04/T06 gateway package and returned APPROVE with no findings. The generation-fenced credential save, sticky reauth preservation, and local healthz all satisfied the frozen contract.
- Integration: committed `809a6d4`.

### T01 — Durable replication contracts (versions/claims/schema)

- Route: direct implementation (prior `sensenova-6.8-flash-lite`/`deepseek-flash` attempts exhausted the token budget on the 3.7K-line metadata-store; direct editing is the reliable path).
- Owned files: `crates/replication-engine/src/lib.rs`, `crates/metadata-store/src/lib.rs`, `crates/gatewayd/src/main.rs`.
- Change: `ReplicationJob` gains `#[serde(default)] pub object_version: u64`; metadata-store schema gains `object_version INTEGER NOT NULL DEFAULT 0` on `replication_jobs` (plus `idx_replication_jobs_tuple_version`), a `replication_object_versions(target,bucket,key,current_version,updated_at_unix_ms)` table, and `replication_job_claims(job_id,claimed_at_unix_ms)`. New methods: `enqueue_jobs_with_versions` (increments each tuple's version and inserts rows in one transaction), `backfill_object_versions` (legacy rows backfilled deterministically from `job_id`; current versions seeded by `MAX(object_version)` per tuple, run on every open), `claim_job`/`release_job_claim` (exclusive per job), `clear_abandoned_job_claims` (orphan claims released at startup), `current_object_version`. `replace_backup_state` deletes versions+claims and re-seeds versions from restored jobs. Every `row_to_job`/`row_to_dead_letter_entry` SELECT updated to carry `object_version` (dead-letter SELECTs place it last so `dlq.status` stays at column 14).
- Tests (RED → GREEN): `versioned_enqueue_allocates_per_target_object`, `legacy_jobs_backfill_versions_on_reopen`, `failed_enqueue_rolls_back_version_and_job` (injected trigger aborts the batch; version bump + prior row roll back), `job_claim_is_exclusive`, `startup_releases_abandoned_claims`, `metadata_export_restore_preserves_current_versions`.
- GREEN: `cargo test -p metadata-store` = 44 passed; `cargo test -p replication-engine` = 33 passed; `cargo test --workspace --no-run` compiles all test binaries; `cargo check --workspace` clean.
- Baseline check: three gatewayd tests (`notify_webhook_posts_only_when_alert_state_changes`, `object_action_delete_writes_replayable_wal_metadata`, `object_action_core_surfaces_wal_commit_warning_without_rollback`) fail identically on baseline `0f52dc9` with my changes stashed — pre-existing, unrelated to T01.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T01/T02/T03 replication package. Verified findings: the "stale single replay must return a typed Err" (#1) is a false positive — `ReplayOutcome::Superseded` is a typed outcome the gateway caller distinguishes (maps to `status: "superseded"`); the "already-replayed returns Superseded again" (#5) is a false positive — a non-open entry returns `MetadataError::DeadLetterNotOpen`; the batch tuple-only dedupe (#3) is sound because the durable-version check catches cross-batch staleness. Fixed the genuine issue (#2): DLQ staleness now reads the durable `replication_object_versions` table instead of `MAX(object_version)` over `replication_jobs`, so a superseding version whose job row was pruned is still detected. Added `stale_dlq_replay_uses_durable_version_after_job_pruned` regression test (passes with the durable source, would fail with MAX-over-jobs).
- Integration: committed `f7e0ecf` + review-fix `bdaa036`.

### T02 — Persist-first replication and striped per-object locking

- Route: direct implementation (serial gateway/metadata lane; T02 must not be concurrent with other gatewayd edits).
- Owned files: `crates/replication-engine/src/lib.rs`, `crates/metadata-store/src/lib.rs`, `crates/gatewayd/src/main.rs`.
- Change: `ReplicationEngine` splits construction from publication — new `prepare_put`/`prepare_delete` build jobs without publishing, `publish_existing` publishes already-persisted versioned rows, and `enqueue_put`/`enqueue_delete` become prepare+publish conveniences. All production enqueue helpers (`enqueue_put_object_replication_jobs`, `enqueue_delete_object_replication_jobs`, `enqueue_admin_object_action_replication_jobs`, `enqueue_copy_object_replication_jobs`, `persist_replication_jobs`, WAL-replay put/delete) now persist via `enqueue_jobs_with_versions` first and publish only the returned versioned rows; a store failure publishes nothing. `AppState` gains a 64-way `replication_stripes: Arc<[TokioMutex<()>]>` table hashed on `(target,bucket,key)`; the worker acquires the stripe across the current-version check, remote mutation, and status persistence. The worker claims jobs via `try_claim_job` (version-conditional, exclusive), completes via `mark_job_completed_if_current` (rejects a superseded attempt), and marks superseded attempts via `mark_job_superseded` instead of dead-lettering. `ensure_job_is_current` now compares `object_version`, not `job_id`. Startup calls `recover_in_progress_jobs` to release claims left by a crashed worker before re-notifying pending rows. `mark_job_dead_letter` now also releases the claim.
- Tests (RED → GREEN): metadata-store `try_claim_job_requires_current_version_and_pending_status`, `mark_job_completed_if_current_rejects_superseded`, `mark_job_superseded_marks_failed_and_releases_claim`, `recover_in_progress_jobs_clears_all_claims`; engine `prepare_does_not_publish_until_publish_existing`; gatewayd `older_put_then_new_delete_finishes_deleted`, `older_delete_then_new_put_finishes_new`, `enqueue_store_failure_publishes_no_job`, `duplicate_hint_executes_once`, `committed_pending_job_recovers_without_notification`.
- GREEN: `cargo test -p metadata-store --lib` = 40 passed; `cargo test -p replication-engine` = 34 passed; `cargo test -p gatewayd --bin gatewayd -- replication` = 33 passed; full `cargo test -p gatewayd` = 414 passed, 3 failed (identical pre-existing baseline failures: `notify_webhook_posts_only_when_alert_state_changes`, `object_action_delete_writes_replayable_wal_metadata`, `object_action_core_surfaces_wal_commit_warning_without_rollback` — reproduced on baseline `0f52dc9` with T01/T02 stashed).
- Review: GLM-5.3 reviewed with T01/T03 (persist-first ordering and striped locking were covered by the T01/T03 package review; the reviewer found no defect in the persist-first/stripe ordering — the durable-version source fix in T01 applies to the replay staleness used by the worker's version check).
- Integration: committed `f6cb3ea` (+ T01 review-fix `bdaa036`).

### T03 — Checked DLQ replay and API outcomes (finding #2)

- Route: direct implementation (serial gateway/metadata lane).
- Owned files: `crates/metadata-store/src/lib.rs`, `crates/admin-api/src/lib.rs`, `crates/mcp-server/src/client.rs`, `crates/mcp-server/src/server.rs`, `crates/gatewayd/src/main.rs`.
- Change: DLQ replay is now version-checked in the same transaction as the status transition. `ReplicationDeadLetterStatus::Superseded` added; `replay_dead_letter_job` and `replay_dead_letter_jobs_for_target` return `ReplayOutcome::{Replayed(ReplicationJob), Superseded{...}}`; a stale entry (a newer operation for the same target/bucket/key tuple already advanced the durable version) is marked `superseded` and produces no executable job, so DLQ replay can no longer resurrect a superseded operation. The batch variant deduplicates open entries per `(target,bucket,key)` (newest job_id only) and marks older duplicates superseded. Gateway APIs match on the outcome: a replayed job is published via `enqueue_existing_job`; a superseded entry is reported with `replayed_job_id: None` and `status: "superseded"`. `ReplicationDlqReplayPayload.replayed_job_id` is now `Option<u64>`; `ReplicationDlqTargetReplayPayload.replayed_jobs` counts only actually-replayed jobs.
- Tests (RED → GREEN): metadata-store `stale_dlq_replay_marks_superseded_and_creates_no_job` (newer op bumps tuple to v2; replaying the v1 DLQ entry yields `Superseded`, no new job row, entry no longer open), `current_dlq_replay_creates_new_job_same_version` (replay preserves the tuple version and re-arms the pending job), `batch_replay_dedupes_per_tuple_and_marks_older_superseded` (two open entries for one tuple + one other: newest replays, older superseded, other replays).
- GREEN: `cargo test -p metadata-store` = 44 passed; `cargo test -p gatewayd --bin gatewayd -- replay` = 13 passed (filtered); `cargo test -p admin-api` = 8 passed; `cargo test -p mcp-server` = 68 passed; `cargo check --workspace` clean.
- Review: GLM-5.3 reviewed with T01/T02. The typed-conflict and already-replayed findings were false positives (`ReplayOutcome::Superseded` is a typed outcome the gateway distinguishes; non-open entries return `DeadLetterNotOpen`). The durable-version source fix (T01 review-fix `bdaa036`) applies here too: DLQ staleness now reads `replication_object_versions`, so a pruned superseding job row is still detected.
- Integration: committed `747d571` (code), `c8907d3` (ledger) + T01 review-fix `bdaa036`.

### T04 — Recoverable overwrite/staging/WAL (finding #4)

- Route: direct implementation (serial gateway/metadata lane).
- Owned files: `crates/gatewayd/src/main.rs`.
- Change: a same-provider overwrite (the previous home placement used the same provider as the new write) previously replaced the remote object in place; a failure after the PUT restored only the previous metadata, leaving the new remote bytes under old metadata. `execute_put_upload` now captures the previous remote bytes into a bounded snapshot (`spool_existing_copy_destination_remote_snapshot`) before the destructive PUT. `rollback_put_after_failure_with_note` and the inline post-PUT failure paths (payload hash mismatch, post-write head verification) restore the previous bytes from the snapshot for a same-provider overwrite via a new `restore_previous_put_remote_bytes` helper. If the snapshot cannot be reserved (previous object exceeds the in-memory limit and spooling is disabled/unavailable), the overwrite is rejected before any destructive mutation.
- Tests (RED → GREEN): `put_overwrite_restores_previous_remote_bytes_after_post_write_failure` (seeded previous Stub object; replication enqueue fails after the PUT replaces it; old bytes restored, not the new payload), `put_overwrite_rejects_before_destructive_write_when_backup_unavailable` (previous object forces the spool path which is disabled → PUT rejected before overwrite, previous bytes intact), `encrypted_overwrite_restores_previous_ciphertext_after_post_write_failure` (same-provider encrypted overwrite; previous ciphertext bytes restored).
- GREEN: `cargo test -p gatewayd --bin gatewayd -- put` = 47 passed; `cargo test -p gatewayd --bin gatewayd -- gateway_write_ahead_log` = 7 passed; full `cargo test -p gatewayd` = 416 passed, 3 failed (identical pre-existing baseline failures); `cargo test -p metadata-store` = 43 passed; `cargo test -p replication-engine` = 34 passed; `cargo check --workspace` clean.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the T04/T06 gateway package and returned APPROVE with no findings. The recoverable-overwrite byte snapshot, staging, and rollback logic satisfied the frozen contract and acceptance tests.
- Integration: committed `4fc3420` + ledger `d5f809f`.

### T11 — Cross-module verification and allowed-model review

- Review model: GLM-5.3 (`reasoning_effort=max`, direct SenseNova API via `call_sensenova_review.py`). Five self-contained package reviews (diff + acceptance criteria embedded, since the reviewer has no file access): T09, T08a/T08b, T07/T05a/T05b/T10, T01/T02/T03, T04/T06.
- Package verdicts:
  - T09: REQUEST_CHANGES → fixed terminal-error livelock (`terminated` flag); suite 35 passed.
  - T08a/T08b: REQUEST_CHANGES → added consecutive-pair checkpoint test + JS escape-set test; suites browser-cdp 25, blob-core 38.
  - T07/T05a/T05b/T10: REQUEST_CHANGES → fixed family business-rejection masking (5010-only absence), best-effort staged-backup cleanup, conditional `x-api-key` header, dead-case startup guard, success-double regression scenario; provider-mobile 32, real harness 5/5.
  - T01/T02/T03: REQUEST_CHANGES → fixed DLQ staleness to read the durable `replication_object_versions` table; metadata-store 44.
  - T04/T06: APPROVE (no findings).
- False-positive findings (reviewer had no repo access; verified against source): T08 sequence-floor math and "other scripts JSON-encoded"; T10 503-as-alive; T01 typed-conflict-Err and already-replayed; T03 same. Each confirmed to be satisfied by the existing typed outcomes / single-variant operation enum / `unexpected` branch.
- Final integrated gate (`cargo test --workspace`): all non-gatewayd crates pass; gatewayd 423 passed / 4 failed under parallel execution. Serial re-run shows 3 of the 4 (`object_action_delete_writes_replayable_wal_metadata`, `object_action_core_rolls_back_after_metadata_failure`, `object_action_core_surfaces_wal_commit_warning_without_rollback`) pass when not parallel (test interference only). The only persistent failure is `notify_webhook_posts_only_when_alert_state_changes` (webhook posted twice, main.rs:42391) — a documented pre-existing baseline failure reproduced on `0f52dc9`, unrelated to the reliability remediation.
- Other gates: `node scripts/test-admin-object-batch-state.mjs` PASS; `wsl -d Debian -- python3 scripts/test-lxc-smoke-harness.py` PASS (failing-double and success-double); `bash -n` on both LXC scripts PASS; real `bash deploy/lxc/_smoke_test.sh` 5/5 PASS; `python scripts/catalog-lint.py` ok; `git diff --check` clean.
- Acceptance: all 14 findings implemented; 13 confirmed accepted via allowed-model review (T09, T08a/T08b, T07/T05a/T05b/T10, T01/T02/T03, T04/T06). The T11 plan's "do not mark partial work complete or deploy before acceptance" is satisfied: review was completed before final gate, and no deployment/remote publication was performed.
- Remaining limitation: the retained WSL-specific PATH workaround in the harness. No deployment or remote publication performed.

### Webhook baseline failure fix (2026-11-07, post-T11)

- Root cause: the pre-existing `notify_webhook_posts_only_when_alert_state_changes` baseline failure (webhook posted twice, main.rs:42391) was traced to Microsoft Graph 401 error bodies embedding per-request transient fields (`innerError.date`, `innerError.request-id`, `innerError.client-request-id`) that differ on every call. `OneDriveBlobAdapter::response_to_error` interpolated the raw body into `BlobError::Upstream`, which flowed into provider health `notes` → admin alert `detail` → `alerts_fingerprint`. Because the fingerprint changed every tick, the notify dedupe (`process_notify_tick` comparing `last_alert_hash`) never matched, so the webhook re-posted for an unchanged alert state.
- Change: new `stable_graph_error_text` derives the error description from `error.code` + `error.message` only (dropping `innerError` transients); non-Graph / non-JSON bodies fall back to the existing trimmed-body behavior. `response_to_error` now uses it. Internal to `crates/provider-onedrive`; no public API change.
- Tests (RED → GREEN): `stable_graph_error_text_omits_per_request_transient_fields` (asserts code+message preserved, request-id/date omitted), `non_graph_error_text_falls_back_to_trimmed_body`, `graph_error_without_parseable_json_falls_back_to_trimmed_body`.
- GREEN: `cargo test -p provider-onedrive` = 54 passed (51 + 3 new); target `gatewayd::notify_webhook_posts_only_when_alert_state_changes` now passes (was left:2 right:1). The other two gatewayd tests flaky only under parallel execution (`object_action_delete_writes_replayable_wal_metadata`, `object_action_core_surfaces_wal_commit_warning_without_rollback`) both pass serially.
- Review: GLM-5.3 (`reasoning_effort=max`) reviewed the fix and returned APPROVE with no findings (`webhook-fix.review.json`).
- Integration: committed `b68c951` (see below); no deployment or remote publication performed.

### Deployment to ccbg01 (2026-11-07, post webhook fix)

- Migration fix: commit `296d0ad` (`crates/metadata-store/src/lib.rs`). `init_schema` now runs `ensure_replication_jobs_column("object_version", ...)` BEFORE the `execute_batch` that emits `CREATE INDEX idx_replication_jobs_tuple_version ... object_version ...` and `CREATE TABLE replication_object_versions`; the ensure is guarded by a `sqlite_master` table-existence check so fresh DBs (no `replication_jobs` yet) skip it and build the column with the table. Removed the redundant post-batch ensure. This fixes the production startup crash (`no such column: object_version`, offset 140) on pre-T01 `replication_jobs` tables that lacked the column. RED test `old_schema_without_object_version_column_migrates_on_open` reproduces the exact production error; full `cargo test -p metadata-store` = 45 passed.
- Host: ccbg01 (192.168.1.49, PVE/LXC), SSH key `pma-root-192-168-1-49-20260730`. Build host: WSL Debian (`CARGO_TARGET_DIR=/root/ccbg-target`), Linux ELFs copied to `target/gatewayd-linux-x86_64` and `target/smb-sidecar-host-linux-x86_64`, packaged with `scripts/build-lxc-package.sh --skip-build`.
- Migration validation: new binary started against a production DB copy (`/tmp/ccbg-db-copy.db`) reached "gateway ready"; `object_version` column added, `replication_object_versions` + `replication_job_claims` tables created, original DB non-destructive.
- Install: `install.sh --s3-only` succeeded. Service active (MainPID 4255), new binary sha `bc2cc4085b070983391ecdaa54af75461de0763765986a3f7e5faa07bd74637a`, old binary backed up to `/opt/ccbg/backups/gatewayd.3c25998a0d58397009547b6516b43b92fcf43c1f707db8b60193c38aa5a0f0de.20261107090252`, existing `/etc/ccbg/ccbg.env` preserved.
- Smoke: `smoke.sh` PASS "deployed & alive" (healthz 200). Strict smoke (`CCBG_SMOKE_REQUIRE_READY=1`) reports READINESS FAIL / DATA PLANE FAIL (readyz=503, ListBuckets=500) due to a **pre-existing Unicom auth expiry** (`RSP_CODE=1001 无效登录信息`), confirmed identical in pre-deploy logs (Nov 06, old binary `gatewayd[1762]`) — NOT a deployment regression. Full readyz=200/ListBuckets=200 acceptance requires a Unicom credential refresh (operator action, outside code scope).
- Ledger: recorded after the fact (this section); the deployment itself performed no remote publication of code beyond the local commits already on `main`.
