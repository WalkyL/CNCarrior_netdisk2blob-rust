# Object Management Batch Actions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Add safe multi-select batch delete and batch move to the CCBG Admin object browser without changing provider protocols.

**Architecture:** Keep the existing single embedded Admin HTML asset. Add typed Admin API contracts, process-local selection/plan caches, a persisted idempotency ledger, and a shared async object-mutation lock in gatewayd. Batch execution calls a refactored single-object action core so it preserves current placement, logical-object, protection-plan, WAL, replication, rollback, and warning behavior.

**Tech Stack:** Rust 2024, Axum 0.8, Tokio, Serde, SQLite metadata store, embedded Admin HTML/JavaScript, Node standard-library test harness.

## Global Constraints

- Work on the current branch; do not create a worktree.
- Do not modify provider crates or BlobBackend.
- Do not deploy to .43, .49, or any remote host.
- Support only batch delete and batch move. Do not add batch copy, batch rename, recursive folder actions, or unbounded server-side search.
- Limit a batch to 100 objects. Selection and plan TTLs are 5 minutes. Retain at most 32 active selection snapshots and 64 ledger entries for 24 hours.
- A move target is destination bucket plus destination prefix; preserve each source basename.
- Gateway-observed move conflicts block the entire batch before its first provider mutation.
- Use a Tokio async mutex for the mutation lock. Never hold a std::sync::Mutex guard across await.
- Existing single-object delete/move semantics are authoritative. Batch code must call one structured single-action core rather than copy remote mutation, metadata, WAL, replication, or rollback logic.
- WAL preparation, metadata, and replication-enqueue errors retain existing rollback behavior. WAL commit-finalization errors are warnings and do not roll back an otherwise successful object action.
- Keep all runtime frontend code in crates/gatewayd/assets/admin/index.html. Do not introduce a browser asset that package/runtime template discovery would not serve.
- Broad cargo fmt --all -- --check has a committed baseline failure. Do not reformat unrelated code or claim that broad formatter gate is green; use git diff --check and focused compilation/tests.
- Preserve old control-plane data through serde defaults and never log credentials, cookies, provider responses, or API keys.

## File Map

- Modify: crates/admin-api/src/lib.rs
  - Batch routes, DTOs, route contracts, and contract tests.
- Modify: crates/gatewayd/src/main.rs
  - Batch runtime state, control-plane ledger/revision fields, authenticated principal, mutation lock, action-core extraction, handlers, and Rust tests.
- Modify: crates/gatewayd/assets/admin/index.html
  - Object list multi-select, management flow, preview/confirmation/results, and pure state block.
- Create: scripts/test-admin-object-batch-state.mjs
  - Node-only test harness for the pure embedded Admin state functions.
- Modify: docs/object-actions-api-reference.md
  - Public batch route, response, error, idempotency, and recovery contract.
- Modify: docs/object-actions-and-history.md
  - Operator workflow, warning, recovery, and placement-cleanup guidance.

---

### Task 1: Add Typed Admin API Contracts

**Files:**
- Modify: crates/admin-api/src/lib.rs:10-76, 580-726, 850-1020
- Test: crates/admin-api/src/lib.rs tests module

**Interfaces:**
- Produces: ROUTE_OBJECT_BATCH_PREVIEW = /api/object-actions/batch/preview
- Produces: ROUTE_OBJECT_BATCH_EXECUTE = /api/object-actions/batch
- Produces: ObjectBatchAction, ObjectBatchObjectInput, ObjectBatchPreviewInput, ObjectBatchExecuteInput
- Produces: ObjectBatchItemStatus, ObjectBatchItemPayload, ObjectBatchPreviewPayload, ObjectBatchExecutePayload, ObjectBatchErrorPayload, ObjectBatchRecoveryPayload
- Consumed by: gatewayd and embedded Admin Web.

- [ ] **Step 1: Write the failing route and serde contract test**

Add object_batch_contracts_are_registered_and_serde_stable to admin-api tests.

~~~rust
#[test]
fn object_batch_contracts_are_registered_and_serde_stable() {
    let routes = route_contracts();
    assert!(routes.iter().any(|route| {
        route.id == "object_batch_preview"
            && route.method == AdminApiMethod::Post
            && route.path == ROUTE_OBJECT_BATCH_PREVIEW
            && route.request == Some(AdminDtoKind::ObjectBatchPreviewInput)
            && route.response == AdminDtoKind::ObjectBatchPreviewPayload
    }));
    assert!(routes.iter().any(|route| {
        route.id == "object_batch_execute"
            && route.method == AdminApiMethod::Post
            && route.path == ROUTE_OBJECT_BATCH_EXECUTE
            && route.request == Some(AdminDtoKind::ObjectBatchExecuteInput)
            && route.response == AdminDtoKind::ObjectBatchExecutePayload
    }));
}
~~~

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
cargo test -p admin-api object_batch_contracts_are_registered_and_serde_stable -- --exact
~~~

Expected: compilation failure because the routes and DTOs do not exist.

- [ ] **Step 3: Implement the minimal public contract**

Add route constants and AdminDtoKind entries. Use the following request types.

~~~rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectBatchAction {
    Delete,
    Move,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectBatchObjectInput {
    pub bucket: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectBatchPreviewInput {
    pub action: ObjectBatchAction,
    pub selection_id: String,
    pub objects: Vec<ObjectBatchObjectInput>,
    #[serde(default)]
    pub destination_bucket: Option<String>,
    #[serde(default)]
    pub destination_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectBatchExecuteInput {
    pub plan_id: String,
    pub batch_id: String,
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}
~~~

Define a snake_case ObjectBatchItemStatus covering Ready, AlreadyMissing, NoOp, Completed, Failed, StaleConflict, NotStarted, Conflict, and Unverifiable. Define ObjectBatchItemPayload with ordinal, source, read_source, home_provider, optional destination, status, optional reason_code, optional message, and warnings. Define success, error, and recovery payloads to match the approved specification.

Register both POST routes with Operator surface. Extend route contract presence and DTO-kind tests.

- [ ] **Step 4: Verify GREEN**

Run:

~~~powershell
cargo test -p admin-api object_batch_contracts_are_registered_and_serde_stable -- --exact
cargo test -p admin-api route_contracts_reference_exported_dto_kinds -- --exact
~~~

Expected: both pass.

- [ ] **Step 5: Commit**

~~~powershell
git add crates/admin-api/src/lib.rs
git commit -m "feat(admin-api): add batch object action contracts"
~~~

### Task 2: Add Gateway State, Principal, and Shared Mutation Lock

**Files:**
- Modify: crates/gatewayd/src/main.rs:24, 240-270, 1111-1147, 7040-7080, 7690-7725, 7927-7966, 17592-17640, 37076-37186
- Test: crates/gatewayd/src/main.rs tests module

**Interfaces:**
- Produces: AppState.object_mutation_lock as Arc<TokioMutex<()>>
- Produces: AppState.object_batch_runtime as Arc<Mutex<ObjectBatchRuntimeState>>
- Produces: ControlPlaneState.object_batch_ledger and object_topology_generation with serde defaults.
- Produces: AdminAuthenticatedPrincipal inserted by require_admin_access.
- Produces: topology_fingerprint and with_object_mutation_lock helpers.
- Consumed by: object actions, topology update, reconcile execution, stale-placement cleanup, and batch handlers.

- [ ] **Step 1: Write failing state and lock tests**

~~~rust
#[tokio::test]
async fn object_batch_topology_update_waits_for_mutation_lock() {
    let state = test_state();
    let guard = state.object_mutation_lock.lock().await;
    let state_for_update = state.clone();
    let task = tokio::spawn(async move {
        update_topology(
            State(state_for_update),
            Json(TopologyUpdateInput {
                primary_provider: ProviderId::Stub,
                sync_targets: Vec::new(),
                fallback_read_order: Vec::new(),
                high_speed_providers: Vec::new(),
                write_targets: vec![ProviderId::Stub],
                object_placement_mode: ObjectPlacementMode::PreferPrimary,
            }),
        )
        .await
    });
    assert!(timeout(Duration::from_millis(50), task).await.is_err());
    drop(guard);
}

#[test]
fn legacy_control_plane_defaults_batch_state() {
    let decoded: ControlPlaneState = serde_json::from_str(
        r#"{"topology":{"primary_provider":"stub","sync_targets":[],"fallback_read_order":[],"onedrive_enabled":true,"replication_mode":"async_backup"}}"#,
    )
    .expect("legacy control plane should decode");
    assert!(decoded.object_batch_ledger.is_empty());
    assert_eq!(decoded.object_topology_generation, 0);
}
~~~

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
cargo test -p gatewayd object_batch_topology_update_waits_for_mutation_lock -- --exact
cargo test -p gatewayd legacy_control_plane_defaults_batch_state -- --exact
~~~

Expected: compilation failure because these fields and helpers do not exist.

- [ ] **Step 3: Implement state and access propagation**

Add:

~~~rust
#[derive(Clone)]
struct AdminAuthenticatedPrincipal(String);

#[derive(Default)]
struct ObjectBatchRuntimeState {
    selections: BTreeMap<String, ObjectBatchSelectionSnapshot>,
    plans: BTreeMap<String, ObjectBatchPlanRecord>,
}
~~~

Add the async object_mutation_lock and short-held object_batch_runtime map to AppState. Add serde-defaulted object_batch_ledger and object_topology_generation to ControlPlaneState, initialize them in production state construction and test_state, and prune to documented limits.

Change require_admin_access to accept a mutable request and insert a server-derived principal before next.run:
- browser session: admin:<username>
- control API key: admin-api-key

Implement with_object_mutation_lock using timeout(Duration::from_secs(5), state.object_mutation_lock.lock()). Wrap single object action, reconcile execution, stale-placement cleanup, and update_topology. Increment object_topology_generation only on a successfully persisted topology update and expose the opaque fingerprint as rev:<generation>.

- [ ] **Step 4: Verify GREEN**

Run:

~~~powershell
cargo test -p gatewayd object_batch_topology_update_waits_for_mutation_lock -- --exact
cargo test -p gatewayd legacy_control_plane_defaults_batch_state -- --exact
cargo test -p gatewayd topology_updates_can_disable_fallback_reads -- --exact
~~~

Expected: all pass.

- [ ] **Step 5: Commit**

~~~powershell
git add crates/gatewayd/src/main.rs
git commit -m "feat(gatewayd): add object batch runtime guards"
~~~

### Task 3: Extract a Shared Single-Action Core

**Files:**
- Modify: crates/gatewayd/src/main.rs:15299-16459, 19895-19965, 21053-21235
- Test: crates/gatewayd/src/main.rs tests module

**Interfaces:**
- Produces: ObjectActionCoreOutcome { warnings: Vec<String> }
- Produces: execute_object_action_core(state, input)
- Produces: mark_gateway_write_ahead_log_committed_or_warn returning Option<String>
- Consumed by: existing run_object_action and batch executor.

- [ ] **Step 1: Write failing warning and rollback tests**

~~~rust
#[tokio::test]
async fn object_action_core_surfaces_wal_commit_warning_without_rollback() {
    let state = test_state();
    let input = seeded_move_input();
    seed_object_for_move(&state, &input).await;
    set_test_wal_commit_failures(1);

    let outcome = execute_object_action_core(&state, &input)
        .await
        .expect("move remains successful");
    assert!(outcome.warnings.iter().any(|warning| warning.contains("WAL")));
    assert_object_exists_on_home(&state, "family", "archive/source.txt").await;
}
~~~

Add object_action_core_rolls_back_after_metadata_failure using the existing move fault-injection helper and asserting source/destination metadata restoration.

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
cargo test -p gatewayd object_action_core_surfaces_wal_commit_warning_without_rollback -- --exact
cargo test -p gatewayd object_action_core_rolls_back_after_metadata_failure -- --exact
~~~

Expected: compilation failure because the core and warning outcome do not exist.

- [ ] **Step 3: Extract the core without semantic drift**

Move the match body from run_object_action into execute_object_action_core. Keep run_object_action responsible for mutation lock acquisition, before/after snapshots, audit extraction, history recording, and the existing 204 response.

Keep delete, rename, copy, and move behavior intact. The batch path uses only Delete and Move.

Change mark_gateway_write_ahead_log_committed_or_warn to return:
- None for no WAL or successful finalization;
- Some sanitized warning after it updates existing runtime WAL failure fields.

Append finalization warnings to ObjectActionCoreOutcome. Do not turn them into errors or roll back a completed provider action. Keep rollback for WAL preparation, metadata persistence, and replication enqueue errors.

- [ ] **Step 4: Verify GREEN**

Run:

~~~powershell
cargo test -p gatewayd object_action_core_surfaces_wal_commit_warning_without_rollback -- --exact
cargo test -p gatewayd object_actions_move_failure_reports_remote_rollback_status -- --exact
cargo test -p gatewayd object_actions_move_replication_enqueue_failure_reports_remote_rollback_status -- --exact
cargo test -p gatewayd object_actions_api_rename_copy_and_move_against_primary_backend -- --exact
~~~

Expected: new warning test and existing action regressions pass.

- [ ] **Step 5: Commit**

~~~powershell
git add crates/gatewayd/src/main.rs
git commit -m "refactor(gatewayd): share object action execution core"
~~~

### Task 4: Create Selection Snapshots and Batch Preview

**Files:**
- Modify: crates/gatewayd/src/main.rs:6262-6465, 7160-7310, 15244-15290, 21244-21376
- Test: crates/gatewayd/src/main.rs tests module

**Interfaces:**
- Consumes: ObjectBatchPreviewInput and ObjectBatchPreviewPayload.
- Produces: selection_id and selection_expires_at_unix_ms on ObjectBrowserObjectsPayload.
- Produces: preview_object_batch at ROUTE_OBJECT_BATCH_PREVIEW.
- Produces: ObjectBatchSelectionSnapshot, ObjectBatchPlanRecord, and ObjectBatchPreflightResult.
- Consumed by: Admin list and batch executor.

- [ ] **Step 1: Write failing preview tests**

~~~rust
#[tokio::test]
async fn object_batch_preview_rejects_destination_conflict_without_mutation() {
    let state = test_state();
    let selection = seed_object_browser_selection(
        &state,
        "root",
        vec![("docs/a.txt", b"a")],
    )
    .await;
    seed_home_object(&state, "family", "archive/a.txt", b"existing").await;

    let error = preview_object_batch(
        State(state.clone()),
        Json(ObjectBatchPreviewInput {
            action: ObjectBatchAction::Move,
            selection_id: selection.selection_id,
            objects: vec![ObjectBatchObjectInput {
                bucket: "root".to_string(),
                key: "docs/a.txt".to_string(),
            }],
            destination_bucket: Some("family".to_string()),
            destination_prefix: Some("archive/".to_string()),
        }),
    )
    .await
    .expect_err("existing destination must block preview");

    assert_eq!(error.code, "destination_exists");
    assert_object_exists_on_home(&state, "root", "docs/a.txt").await;
    assert_object_exists_on_home(&state, "family", "archive/a.txt").await;
}
~~~

Also add exact tests for duplicate generated destination keys, no-op move, stale selection, changed identity, target bucket absence, and source/home mismatch.

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
cargo test -p gatewayd object_batch_preview_rejects_destination_conflict_without_mutation -- --exact
cargo test -p gatewayd object_batch_preview_noop_is_not_a_destination_conflict -- --exact
~~~

Expected: compilation failure because preview and selection types do not exist.

- [ ] **Step 3: Implement preflight**

Extend ObjectBrowserObjectsPayload with selection_id and selection_expires_at_unix_ms. When list_object_browser_objects returns its existing fallback-aware list, create a short-lived runtime snapshot with exact bucket, prefix, source/fallback provider, ObjectInfo identity, and topology fingerprint.

Implement canonical validation that rejects empty values, leading/trailing slash, double slash, backslash, control characters, dot/dot-dot segments, and empty basenames. Generate destination key from canonical prefix plus basename.

Preview must:
- ensure every requested object belongs to the active selection;
- resolve home provider from persisted placement, otherwise current primary;
- validate source identity;
- head destination bucket and check generic write/delete capability;
- detect destination object and gateway metadata conflicts;
- classify no-op before destination-exists checks;
- return a plan only when the whole batch is executable/no-op.

Store action, selection id, normalized objects, destinations, identities, topology fingerprint, preview items, and expiry in ObjectBatchPlanRecord. Register the preview route.

- [ ] **Step 4: Verify GREEN**

Run:

~~~powershell
cargo test -p gatewayd object_batch_preview_rejects_destination_conflict_without_mutation -- --exact
cargo test -p gatewayd object_batch_preview_noop_is_not_a_destination_conflict -- --exact
cargo test -p gatewayd object_browser_lists_current_read_source -- --exact
~~~

Expected: preview is read-only and all tests pass.

- [ ] **Step 5: Commit**

~~~powershell
git add crates/gatewayd/src/main.rs
git commit -m "feat(gatewayd): add batch object action preview"
~~~

### Task 5: Implement Batch Execution, Ledger, and Recovery

**Files:**
- Modify: crates/gatewayd/src/main.rs:6497-6583, 7160-7310, 15299-16459, 17592-17640, 24750-24995
- Test: crates/gatewayd/src/main.rs tests module

**Interfaces:**
- Consumes: ObjectBatchPlanRecord, ObjectBatchExecuteInput, AdminAuthenticatedPrincipal, execute_object_action_core.
- Produces: execute_object_batch at ROUTE_OBJECT_BATCH_EXECUTE.
- Produces: ObjectBatchLedgerRecord, ObjectBatchLedgerState, and ObjectBatchApiError.
- Consumed by: Admin workflow and persisted control-plane reload path.

- [ ] **Step 1: Write failing execution tests**

~~~rust
#[tokio::test]
async fn object_batch_execute_replays_same_idempotency_key() {
    let state = test_state();
    let plan = create_move_plan_for_test(&state, "root", "docs/a.txt", "family", "archive/");
    let principal = AdminAuthenticatedPrincipal("admin:admin".to_string());
    let input = ObjectBatchExecuteInput {
        plan_id: plan.plan_id.clone(),
        batch_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        operator: Some("ops".to_string()),
        ticket: None,
        notes: None,
    };

    let first = execute_object_batch_request(&state, principal.clone(), input.clone()).await;
    let second = execute_object_batch_request(&state, principal, input).await;

    assert_eq!(first, second);
    assert_eq!(provider_move_call_count(&state), 1);
}
~~~

Add tests for zero-write final-preflight conflict, execution-time failure producing NotStarted rows, same UUID/different fingerprint, rejected before first provider mutation, InProgress recovery after restart, and Interrupted recovery after a persisted provider marker.

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
cargo test -p gatewayd object_batch_execute_replays_same_idempotency_key -- --exact
cargo test -p gatewayd object_batch_execute_stops_remaining_items_after_failure -- --exact
cargo test -p gatewayd object_batch_recovery_contract_is_not_replayed -- --exact
~~~

Expected: compilation failure because the executor and ledger do not exist.

- [ ] **Step 3: Implement execution state machine**

Add ObjectBatchApiError with status, code, message, action, items, and optional Retry-After response header.

Implement execute_object_batch:
- require the Idempotency-Key header;
- require it to equal batch_id and pass the canonical lowercase UUID v4 parser;
- take AdminAuthenticatedPrincipal from middleware extension;
- delegate to execute_object_batch_request.

Implement the request flow:
1. inspect an existing ledger before waiting;
2. obtain the mutation lock with the five-second timeout;
3. inspect the ledger again after the lock;
4. validate plan TTL, topology fingerprint, source identity, target buckets, metadata conflicts, and action availability;
5. persist InProgress only after all zero-write checks pass;
6. persist provider_call_started_at immediately before first provider mutation;
7. call execute_object_action_core in stable source bucket/key order;
8. persist each item result and warnings;
9. persist one Completed response/history summary.

A core error after earlier successes marks that row Failed, all remaining rows NotStarted, and returns a truthful 200 result. Native provider move NotImplemented is a per-item provider_unsupported failure, not a destination conflict.

For zero-write failure after reservation and before provider marker, persist Rejected with its exact HTTP response. During control-plane load:
- InProgress without marker becomes Rejected batch_not_started;
- InProgress with marker becomes Interrupted batch_recovery_required.

Interrupted recovery returns saved_results and unresolved_items, never replays work, and does not use the ordinary result count invariant.

- [ ] **Step 4: Verify GREEN**

Run:

~~~powershell
cargo test -p gatewayd object_batch_execute_replays_same_idempotency_key -- --exact
cargo test -p gatewayd object_batch_execute_stops_remaining_items_after_failure -- --exact
cargo test -p gatewayd object_batch_recovery_contract_is_not_replayed -- --exact
cargo test -p gatewayd object_batch_topology_update_waits_for_mutation_lock -- --exact
cargo test -p gatewayd object_actions_move_failure_reports_remote_rollback_status -- --exact
cargo test -p gatewayd delete_stale_object_placement_bulk_deletes_selected_stale_rows_only -- --exact
~~~

Expected: all pass.

- [ ] **Step 5: Commit**

~~~powershell
git add crates/gatewayd/src/main.rs
git commit -m "feat(gatewayd): execute batch object actions safely"
~~~

### Task 6: Build the Embedded Admin Workflow

**Files:**
- Modify: crates/gatewayd/assets/admin/index.html:1609-1793, 5171-5178, 11192-11416, 11939-12110, 20543-20606
- Create: scripts/test-admin-object-batch-state.mjs
- Test: crates/gatewayd/src/main.rs admin page contract test
- Test: scripts/test-admin-object-batch-state.mjs

**Interfaces:**
- Consumes: selection_id, preview/execute APIs, typed payloads, and error payloads.
- Produces: multi-select rows, current-result toolbar, management panel, move preview, delete confirmation, per-item results.
- Produces: globalThis.ccbgObjectBatchState in index.html.
- Consumed by: Node harness and browser runtime.

- [ ] **Step 1: Write the failing Node harness**

Create scripts/test-admin-object-batch-state.mjs. Read index.html, extract the block between the exact comments object-batch-state:start and object-batch-state:end, and evaluate it with node:vm.

Assert deterministic selection, reset on selection replacement, current-result-only select-all, basename-preserving destinations, duplicate destination detection, stable execution UUID for one plan, and reset after result.

~~~javascript
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const html = fs.readFileSync("crates/gatewayd/assets/admin/index.html", "utf8");
const match = html.match(/\/\* object-batch-state:start \*\/([\s\S]*?)\/\* object-batch-state:end \*\//);
assert.ok(match, "batch state block exists");
const sandbox = { globalThis: {} };
vm.runInNewContext(match[1], sandbox);
const api = sandbox.globalThis.ccbgObjectBatchState;
const state = api.create();
api.replaceList(state, { selectionId: "s1", keys: ["docs/a.txt", "docs/b.txt"] });
api.selectAll(state);
assert.deepEqual(api.selectedKeys(state), ["docs/a.txt", "docs/b.txt"]);
~~~

- [ ] **Step 2: Verify RED**

Run:

~~~powershell
node scripts/test-admin-object-batch-state.mjs
~~~

Expected: failure because the harness and marked state block do not exist.

- [ ] **Step 3: Implement list-first management**

Keep all browser code in index.html. Add the marked pure state block and use it from objectBrowserState.

Extend renderObjectBrowserResults with:
- checkbox column;
- Manage button per row;
- stable checkbox/table dimensions;
- selection rendered from state rather than inferred from DOM.

Add a toolbar above results, shown only when selected_keys is nonempty:
- selected count;
- select current result;
- move;
- delete;
- clear selection.

Add one management panel below results:
- source summary;
- destination bucket/prefix for move;
- preview table;
- per-item result table;
- optional operator label, ticket, notes;
- no provider selector.

Move calls preview first, renders conflicts without executing, saves plan_id, creates one crypto.randomUUID-compatible idempotency UUID, executes with matching header/body UUID, renders warnings/results, clears state, and reloads the current object list.

Delete calls preview first and opens exactly one in-app confirmation panel only after preview success. Do not use window.confirm. State the cloud-object plus metadata consequence.

Keep rename/copy under collapsed Advanced Object Actions. Remove move from the old manual action selector.

- [ ] **Step 4: Add failing HTML contract assertions**

Extend admin_page_exposes_object_actions_panel:

~~~rust
assert!(html.contains("id=\"object-browser-select-all\""));
assert!(html.contains("id=\"object-batch-move\""));
assert!(html.contains("id=\"object-batch-delete\""));
assert!(html.contains("id=\"object-batch-confirm-delete\""));
assert!(html.contains("/api/object-actions/batch/preview"));
assert!(html.contains("/api/object-actions/batch"));
assert!(html.contains("object-batch-state:start"));
assert!(html.contains("删除云盘对象及网关元数据"));
~~~

Also assert Placement cleanup remains explicitly distinct from real cloud-object deletion.

- [ ] **Step 5: Verify GREEN**

Run:

~~~powershell
node scripts/test-admin-object-batch-state.mjs
cargo test -p gatewayd admin_page_exposes_object_actions_panel -- --exact
~~~

Expected: both pass and no additional served browser asset is required.

- [ ] **Step 6: Commit**

~~~powershell
git add crates/gatewayd/assets/admin/index.html scripts/test-admin-object-batch-state.mjs crates/gatewayd/src/main.rs
git commit -m "feat(admin): manage selected cloud objects"
~~~

### Task 7: Update Documentation and Run Integrated Verification

**Files:**
- Modify: docs/object-actions-api-reference.md
- Modify: docs/object-actions-and-history.md
- Modify: docs/superpowers/specs/2026-09-03-object-management-batch-actions-design.md only for an implementation-proven contract mismatch
- Test: admin-api, gatewayd, Node harness, git diff check

**Interfaces:**
- Consumes: final route/DTO names and actual error codes.
- Produces: operator-facing API and recovery documentation.

- [ ] **Step 1: Write the failing documentation assertion**

Add a narrow assertion that final route constants and the closed batch reason-code list appear in the API reference. Include preview then execute, target conflict zero-write behavior, 5-minute expiry, UUID idempotency, Rejected/Interrupted recovery, provider_unsupported stage, WAL warning, and Placement-cleanup distinction.

- [ ] **Step 2: Verify RED**

Run the new focused documentation assertion. Expected: failure until the documents describe the batch workflow.

- [ ] **Step 3: Update operator documents**

In docs/object-actions-api-reference.md, add exact preview/execute requests and payloads, error codes, HTTP status behavior, idempotency/recovery, and generic-versus-native provider capability behavior.

In docs/object-actions-and-history.md, explain current-result selection scope, target bucket/prefix move behavior, one deletion confirmation, real deletion versus Placement cleanup, partial failures, NotStarted rows, WAL warnings, and Interrupted recovery. State that operator is a label and authenticated principal is server-derived.

- [ ] **Step 4: Verify the integrated change**

Run:

~~~powershell
cargo test -p admin-api
cargo test -p gatewayd object_batch -- --nocapture
cargo test -p gatewayd admin_page_exposes_object_actions_panel -- --exact
node scripts/test-admin-object-batch-state.mjs
cargo check -p gatewayd
git diff --check
~~~

Expected: all pass. Do not run deployment commands or claim the broad cargo fmt baseline is clean.

- [ ] **Step 5: Run focused action regressions**

Run:

~~~powershell
cargo test -p gatewayd object_actions_api_rename_copy_and_move_against_primary_backend -- --exact
cargo test -p gatewayd object_actions_move_failure_reports_remote_rollback_status -- --exact
cargo test -p gatewayd object_action_core_surfaces_wal_commit_warning_without_rollback -- --exact
cargo test -p gatewayd object_batch_execute_replays_same_idempotency_key -- --exact
cargo test -p gatewayd object_batch_preview_rejects_destination_conflict_without_mutation -- --exact
~~~

Expected: all pass without remote host access.

- [ ] **Step 6: Commit**

~~~powershell
git add docs/object-actions-api-reference.md docs/object-actions-and-history.md docs/superpowers/specs/2026-09-03-object-management-batch-actions-design.md
git commit -m "docs: document batch object management"
~~~

## Plan Self-Review

### Spec Coverage

- Multi-select, list-first UI, deletion confirmation, and simplicity: Task 6.
- Identity, basename moves, no-op, target buckets, and collision preflight: Task 4.
- Lock scope, topology fingerprint, idempotency, and recovery: Tasks 2 and 5.
- Single-object metadata, WAL, replication, rollback, and warning reuse: Task 3.
- Public contracts and route ledger: Task 1.
- Operator documentation and integrated verification: Task 7.

No approved requirement lacks an owning task.

### Placeholder Scan

Every task names files, interfaces, failing tests, implementation behavior, verification commands, and a commit boundary. The plan has no deferred or unspecified work markers.

### Type Consistency

- Task 1 defines ObjectBatchPreviewInput and ObjectBatchExecuteInput; Tasks 4 through 6 consume them.
- Task 2 defines AdminAuthenticatedPrincipal and ledger state; Task 5 consumes them.
- Task 3 defines execute_object_action_core; Task 5 consumes it.
- Task 1 defines ObjectBatchItemPayload; Tasks 4, 5, and 6 use it for preview, result, history, and UI.
