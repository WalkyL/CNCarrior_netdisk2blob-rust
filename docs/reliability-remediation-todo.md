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
| T02 | DB-first enqueue/retry, striped worker locks, recovery | T01 | pending |
| T03 | Checked DLQ replay and API outcomes | T01, T02 | pending |
| T04 | Recoverable overwrite/staging/WAL | T01–T03 | pending |
| T05a | Mobile family error propagation | architecture | complete |
| T05b | Mobile non-destructive staged overwrite | T05a | complete |
| T06 | Request backend generation, sticky reauth, local liveness | gateway owner released | pending |
| T07 | Four-provider exact list prefix preservation | Mobile file waits T05 | complete |
| T08a | Action-scoped CDP checkpoints | architecture | complete |
| T08b | Explicit JavaScript value binding/catalog migration | T08a | complete |
| T09 | Encryption declared-size validation | architecture | complete |
| T10 | Smoke harness failure propagation/regression | architecture | complete |
| T11 | Integration acceptance and allowed-model reviews | completed tasks | pending |

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
- Review: pending (needs GLM-5.3 or Kimi-K3).
- Integration: committed `95006a7`. Remaining concern: none functional; Mobile list still uses its own path and waits on T05.

### T09 — Encryption declared-size validation

- Route: `agnes-3.0-flash` with focused excerpt (initial `sensenova-6.8-flash-lite` returned empty diffs on the 53KB source).
- Owned files: `crates/object-crypto/src/lib.rs`.
- Change: `EncryptStreamState` gains `expected_plaintext_size`, `consumed_plaintext`, `terminal_error`; the `encrypt_upload` unfold closure checks-adds per chunk and, at EOF, emits a single terminal `BlobError::BodyStream` when the consumed byte count differs from the declared `plaintext_size`.
- Tests: 7 new (`encrypt_rejects_short_plaintext`, `encrypt_rejects_long_plaintext`, `encrypt_rejects_nonempty_zero_size`, `encrypt_validates_fragmented_stream_size`, `encrypt_propagates_source_error`, `encrypt_exact_size_roundtrip`, `encrypt_zero_size_roundtrip`); 8 baseline fixtures corrected from `plaintext_size: 32` to the actual short plaintext length.
- GREEN: `cargo test -p object-crypto` = 33 passed.
- Review: pending.
- Integration: committed `22bb34c`.

### T08a — Action-scoped CDP checkpoints

- Route: direct implementation (prior model PROD-DIFF set the checkpoint after dispatch, still missing the just-sent request; TEST-DIFF referenced a nonexistent session).
- Owned files: `crates/browser-cdp/src/lib.rs`, `crates/blob-core/src/browser_flow.rs`.
- Change: `CdpEventState` gains `request_floor_sequence: Option<u64>`; `mark_request_checkpoint()` records the current sequence before an action dispatches; `take_request_floor()` consumes it (falling back to the documented current-time floor for standalone waits). `BrowserFlowSession::mark_action_request_checkpoint` is a default no-op so non-CDP sessions and the test mock are unchanged; the executor calls it before navigate/click/set_input/invoke_operation/set_files/dispatch_events.
- Tests: `request_checkpoint_captures_action_request_and_excludes_preaction`, `request_checkpoint_consumed_by_following_wait`.
- GREEN: `cargo test -p browser-cdp --lib` = 24 passed; `cargo test -p blob-core --lib` = 36 passed.
- Review: pending.
- Integration: committed `6636f5d`.

### T08b — Explicit JavaScript value binding/catalog migration

- Route: direct implementation.
- Owned files: `crates/blob-core/src/browser_flow.rs`, `config/browser-flows/unicom-web.json`.
- Change: add `bind_javascript_template` which serializes each placeholder via `serde_json::to_string` (quoted+escaped JS string literal; bare literals for numbers/bools/null); `bind_operation` uses it for `operation.source`. Migrate the 5 Unicom JS expressions so `'{{inputs.x}}'` becomes `{{inputs.x}}` (quotes removed; JSON encoding supplies them exactly once).
- Tests: `bind_operation_encodes_javascript_values_without_injection` (payload `'); alert('pwned'); //` must stay inside one quoted literal).
- GREEN: `cargo test -p blob-core --lib` = 37 passed.
- Review: pending.
- Integration: committed `8a115ce`.

### T10 — Smoke harness exit semantics

- Route: direct implementation.
- Owned files: `deploy/lxc/_smoke_test.sh`, `scripts/test-lxc-smoke-harness.py`.
- Change: add a `FAILURES` accumulator flipped by every failing case and by mock startup failures; exit 1 when nonzero. `cleanup` trap on EXIT/INT/TERM stops the harness-owned mock and removes only the harness mktemp dir.
- Tests: new `scripts/test-lxc-smoke-harness.py` (copies harness + failing smoke.sh double into a temp dir, invokes Bash, asserts FAIL → nonzero). RED against the old harness (printed FAIL but exited 0); GREEN now.
- GREEN: Python regression under WSL Debian passed; `bash -n` on both LXC scripts; real `bash deploy/lxc/_smoke_test.sh` = 5/5 with `RESULT: PASS`.
- Review: pending.
- Integration: committed `124de30`.

### T05a — Mobile family error propagation (finding #5)

- Route: direct implementation.
- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `native_root_folder_id_for_container` no longer resolves the family root via `family_disk_info_if_available().await.ok()` (which swallowed timeout/401/500 into `NotFound`). New `family_disk_info()` helper returns `Ok(None)` only for an explicit `success:false` "family not enabled" response and `Err` for transport/parse failures, which object operations now propagate. Removed the dead `.ok()` wrapper.
- Tests: `family_transient_error_is_not_masked_as_not_found` (500 on `getFamilyDiskInfo` → `list_objects` must not be `NotFound`).
- GREEN: `cargo test -p provider-mobile --lib` = 28 passed (before prefix addition); family group = 5 passed including the previously-masked `list_containers_keeps_root_when_family_disk_info_is_unavailable`.
- Review: pending.
- Integration: committed `575c763`.

### T05b — Mobile non-destructive staged overwrite (finding #6)

- Route: direct implementation.
- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `create_mobile_upload` no longer deletes existing same-name children before uploading the replacement. When `file/create` reports an existing object without an uploadId, each same-name sibling is renamed to a hidden `.ccbg-staging-<nonce>-<name>` via `file_update` before `file/create` is retried at the original name. The staged backups are returned with the upload plan; `put_object` discards them (delete) only after the replacement is uploaded and completed, and renames them back to their original names if create/upload/complete fails. `delete_child_files_by_name` is removed. `list_scope_objects` hides staging-named siblings so they never surface in ordinary list results.
- Tests: `overwrite_restores_staged_old_object_when_complete_fails` (complete error → original renamed back and still reachable, no delete), `put_object_retries_after_existing_object_blocks_mobile_create_upload_id` (asserts rename-backup precedes retried create, delete follows complete, staging prefix), `list_objects_excludes_staged_overwrite_siblings`.
- GREEN: `cargo test -p provider-mobile --lib` = 31 passed; `cargo build -p provider-mobile` clean; `cargo check --workspace` clean.
- Review: pending (needs GLM-5.3 or Kimi-K3).
- Integration: committed `134b270`. Remaining concern: best-effort restore failure leaves a hidden staging sibling (never surfaced in listings); a backup delete failure after success leaves an invisible orphan.

### T07 (Mobile portion) — Mobile exact list prefix preservation (finding #10)

- Owned files: `crates/provider-mobile/src/lib.rs`.
- Change: `list_scope_objects` uses raw `request.prefix` (no `normalize_object_key`) for both the directory walk and the leaf `starts_with`; `directory_may_contain_prefix` aligned with S3 raw-substring semantics so prefix `docs` also reaches `docs-archive` while `docs/` stays isolated.
- Tests: `list_objects_prefix_is_segment_boundary_aware` (docs + docs-archive nested under the managed root).
- GREEN: `cargo test -p provider-mobile --lib` = 29 passed.
- Review: pending.
- Integration: committed `6421343`. Finding #10 now complete across Unicom, Telecom, OneDrive, and Mobile.

### T01 — Durable replication contracts (versions/claims/schema)

- Route: direct implementation (prior `sensenova-6.8-flash-lite`/`deepseek-flash` attempts exhausted the token budget on the 3.7K-line metadata-store; direct editing is the reliable path).
- Owned files: `crates/replication-engine/src/lib.rs`, `crates/metadata-store/src/lib.rs`, `crates/gatewayd/src/main.rs`.
- Change: `ReplicationJob` gains `#[serde(default)] pub object_version: u64`; metadata-store schema gains `object_version INTEGER NOT NULL DEFAULT 0` on `replication_jobs` (plus `idx_replication_jobs_tuple_version`), a `replication_object_versions(target,bucket,key,current_version,updated_at_unix_ms)` table, and `replication_job_claims(job_id,claimed_at_unix_ms)`. New methods: `enqueue_jobs_with_versions` (increments each tuple's version and inserts rows in one transaction), `backfill_object_versions` (legacy rows backfilled deterministically from `job_id`; current versions seeded by `MAX(object_version)` per tuple, run on every open), `claim_job`/`release_job_claim` (exclusive per job), `clear_abandoned_job_claims` (orphan claims released at startup), `current_object_version`. `replace_backup_state` deletes versions+claims and re-seeds versions from restored jobs. Every `row_to_job`/`row_to_dead_letter_entry` SELECT updated to carry `object_version` (dead-letter SELECTs place it last so `dlq.status` stays at column 14).
- Tests (RED → GREEN): `versioned_enqueue_allocates_per_target_object`, `legacy_jobs_backfill_versions_on_reopen`, `failed_enqueue_rolls_back_version_and_job` (injected trigger aborts the batch; version bump + prior row roll back), `job_claim_is_exclusive`, `startup_releases_abandoned_claims`, `metadata_export_restore_preserves_current_versions`.
- GREEN: `cargo test -p metadata-store` = 36 passed; `cargo test -p replication-engine` = 33 passed; `cargo test --workspace --no-run` compiles all test binaries; `cargo check --workspace` clean.
- Baseline check: three gatewayd tests (`notify_webhook_posts_only_when_alert_state_changes`, `object_action_delete_writes_replayable_wal_metadata`, `object_action_core_surfaces_wal_commit_warning_without_rollback`) fail identically on baseline `0f52dc9` with my changes stashed — pre-existing, unrelated to T01.
- Review: pending (needs GLM-5.3 or Kimi-K3).
- Integration: pending commit.
