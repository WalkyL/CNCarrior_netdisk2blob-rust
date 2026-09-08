import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const html = fs.readFileSync("crates/gatewayd/assets/admin/index.html", "utf8");
const match = html.match(/\/\* object-batch-state:start \*\/([\s\S]*?)\/\* object-batch-state:end \*\//);
assert.ok(match, "batch state block exists");

const sandbox = { globalThis: {} };
vm.runInNewContext(match[1], sandbox);
const api = sandbox.globalThis.ccbgObjectBatchState;
assert.ok(api, "batch state API is exposed");

const state = api.create();
assert.equal(state.executing, false, "batch execution starts idle");
api.replaceList(state, {
  selectionId: "s1",
  bucket: "root",
  keys: ["docs/b.txt", "docs/a.txt", "images/logo.png"],
});
api.setSelected(state, "docs/a.txt", true);
api.setSelected(state, "docs/b.txt", true);
assert.deepEqual(
  Array.from(api.selectedKeys(state)),
  ["docs/b.txt", "docs/a.txt"],
  "selection follows deterministic current-result order",
);

api.setPlan(state, { planId: "stale-plan", action: "move" });
state.executionId = "123e4567-e89b-42d3-a456-426614174000";
const replacementRevision = state.revision;
api.replaceList(state, {
  selectionId: "s2",
  bucket: "root",
  keys: ["next/c.txt"],
});
assert.deepEqual(Array.from(api.selectedKeys(state)), [], "replacement selection resets state");
assert.equal(state.planId, "", "replacement clears the stale plan");
assert.equal(state.action, "", "replacement clears the stale action");
assert.equal(state.executionId, "", "replacement clears stale idempotency state");
assert.ok(state.revision > replacementRevision, "replacement invalidates pending previews");

api.setSelected(state, "stale/old.txt", true);
api.selectAll(state);
assert.deepEqual(
  Array.from(api.selectedKeys(state)),
  ["next/c.txt"],
  "select-all includes only the current result",
);

api.setPlan(state, { planId: "manage-plan", action: "delete" });
const manageRevision = state.revision;
api.selectOnly(state, "next/c.txt");
assert.deepEqual(
  Array.from(api.selectedKeys(state)),
  ["next/c.txt"],
  "Manage selects exactly its current result row",
);
assert.equal(state.planId, "", "Manage clears any destructive preview plan");
assert.ok(state.revision > manageRevision, "Manage invalidates stale preview state");

api.replaceList(state, {
  selectionId: "s3",
  bucket: "root",
  keys: ["docs/a.txt", "archive/a.txt", "images/logo.png"],
});
api.selectAll(state);
const move = api.buildMove(state, {
  destinationBucket: "family",
  destinationPrefix: "/moved/2026/",
});
assert.deepEqual(
  Array.from(move.objects, item => ({ ...item })),
  [
    { bucket: "root", key: "docs/a.txt" },
    { bucket: "root", key: "archive/a.txt" },
    { bucket: "root", key: "images/logo.png" },
  ],
  "move sources preserve current-result order",
);
assert.deepEqual(
  Array.from(move.destinations, item => ({ ...item })),
  [
    { bucket: "family", key: "moved/2026/a.txt" },
    { bucket: "family", key: "moved/2026/a.txt" },
    { bucket: "family", key: "moved/2026/logo.png" },
  ],
  "move destinations preserve each source basename",
);
assert.deepEqual(
  Array.from(move.duplicateDestinations),
  ["family/moved/2026/a.txt"],
  "duplicate destinations are detected deterministically",
);

const uuid1 = "123e4567-e89b-42d3-a456-426614174000";
const uuid2 = "123e4567-e89b-42d3-b456-426614174001";
let uuidCalls = 0;
const randomUUID = () => [uuid1, uuid2][uuidCalls++];
assert.equal(api.executionId(state, "plan-1", randomUUID), uuid1);
assert.equal(api.executionId(state, "plan-1", randomUUID), uuid1, "one plan reuses its execution UUID");
assert.equal(api.executionId(state, "plan-2", randomUUID), uuid2, "a new plan gets a new UUID");
assert.equal(uuidCalls, 2);
assert.match(uuid1, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);

api.setPlan(state, { planId: "plan-2", action: "move" });
api.resetAfterResult(state);
assert.deepEqual(Array.from(api.selectedKeys(state)), [], "terminal results clear selection");
assert.equal(state.planId, "", "terminal results clear the plan");
assert.equal(state.executionId, "", "terminal results clear idempotency state");

const lifecycleState = api.create();
api.replaceList(lifecycleState, {
  selectionId: "lifecycle-selection",
  bucket: "root",
  keys: ["docs/a.txt"],
});
api.selectAll(lifecycleState);
api.setPlan(lifecycleState, { planId: "lifecycle-plan", action: "delete" });
lifecycleState.executionId = uuid1;
const lifecycleRevision = lifecycleState.revision;
const reloadRevision = api.beginListReload(lifecycleState);
assert.equal(reloadRevision, lifecycleState.revision, "list reload returns its lifecycle revision");
assert.ok(reloadRevision > lifecycleRevision, "list reload supersedes older lifecycle responses");
assert.equal(api.isCurrentRevision(lifecycleState, lifecycleRevision), false, "list reload rejects the superseded response");
assert.equal(lifecycleState.selectionId, "", "list reload clears the visible selection identity");
assert.equal(lifecycleState.bucket, "", "list reload clears the visible source bucket");
assert.deepEqual(Array.from(lifecycleState.resultKeys), [], "list reload clears visible result keys");
assert.deepEqual(Array.from(api.selectedKeys(lifecycleState)), [], "list reload clears the invisible selection");
assert.equal(lifecycleState.planId, "", "list reload clears the pending plan");
assert.equal(lifecycleState.action, "", "list reload clears the pending action");
assert.equal(lifecycleState.executionId, "", "list reload clears the pending idempotency UUID");

const raceState = api.create();
api.replaceList(raceState, {
  selectionId: "race-selection",
  bucket: "root",
  keys: ["docs/a.txt", "docs/b.txt"],
});
api.selectAll(raceState);
const selectionToken = api.beginPreview(raceState, {
  action: "delete",
  selection_id: "race-selection",
  objects: [
    { bucket: "root", key: "docs/a.txt" },
    { bucket: "root", key: "docs/b.txt" },
  ],
});
api.setSelected(raceState, "docs/b.txt", false);
assert.equal(
  api.isCurrentPreview(raceState, selectionToken),
  false,
  "a selection change makes an in-flight preview response stale",
);

api.selectAll(raceState);
const destinationToken = api.beginPreview(raceState, {
  action: "move",
  selection_id: "race-selection",
  objects: [
    { bucket: "root", key: "docs/a.txt" },
    { bucket: "root", key: "docs/b.txt" },
  ],
  destination_bucket: "family",
  destination_prefix: "first/",
});
api.invalidate(raceState, "move");
assert.equal(
  api.isCurrentPreview(raceState, destinationToken),
  false,
  "a destination change makes an in-flight preview response stale",
);

api.setPlan(raceState, { planId: "retry-plan", action: "move" });
raceState.executionId = uuid1;
assert.equal(api.finalizeExecution(raceState, { state: "in_progress" }), false);
assert.equal(raceState.planId, "retry-plan", "in-progress execution retains its plan");
assert.equal(raceState.executionId, uuid1, "in-progress execution retains its idempotency UUID");
assert.equal(api.beginExecution(raceState), true, "a planned batch enters the busy state once");
assert.equal(raceState.executing, true, "pending execution is observable by controls");
assert.equal(api.beginExecution(raceState), false, "a second execute is rejected while pending");
assert.equal(api.finishExecution(raceState, { state: "in_progress" }), false);
assert.equal(raceState.executing, false, "request completion clears the busy state");
assert.equal(raceState.planId, "retry-plan", "in-progress response still retains its plan");
assert.equal(raceState.executionId, uuid1, "in-progress response still retains its idempotency UUID");
assert.equal(api.finalizeExecution(raceState, { batch_id: uuid1, results: [] }), true);
assert.equal(raceState.planId, "", "terminal execution clears its plan");
assert.equal(raceState.executionId, "", "terminal execution clears its idempotency UUID");

const confirmation = api.deleteConfirmation([
  { source: { bucket: "root", key: "docs/a.txt" } },
  { source: { bucket: "root", key: "docs/b.txt" } },
  { source: { bucket: "family", key: "images/c.png" } },
  { source: { bucket: "family", key: "images/d.png" } },
  { source: { bucket: "root", key: "logs/e.log" } },
  { source: { bucket: "root", key: "logs/f.log" } },
  { source: { bucket: "root", key: "logs/g.log" } },
]);
assert.equal(confirmation.count, 7, "delete confirmation reports selected count");
assert.deepEqual(
  Array.from(confirmation.entries),
  [
    "root/docs/a.txt",
    "root/docs/b.txt",
    "family/images/c.png",
    "family/images/d.png",
    "root/logs/e.log",
  ],
  "delete confirmation lists at most the first five objects",
);
assert.equal(confirmation.remainder, 2, "delete confirmation reports the remainder");

console.log("admin object batch state harness: PASS");
