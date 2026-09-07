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

api.replaceList(state, {
  selectionId: "s2",
  bucket: "root",
  keys: ["next/c.txt"],
});
assert.deepEqual(Array.from(api.selectedKeys(state)), [], "replacement selection resets state");

api.setSelected(state, "stale/old.txt", true);
api.selectAll(state);
assert.deepEqual(
  Array.from(api.selectedKeys(state)),
  ["next/c.txt"],
  "select-all includes only the current result",
);

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

console.log("admin object batch state harness: PASS");
