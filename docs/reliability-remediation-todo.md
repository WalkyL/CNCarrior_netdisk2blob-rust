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
| T01 | Durable versions/claims/schema and backup compatibility | architecture | pending |
| T02 | DB-first enqueue/retry, striped worker locks, recovery | T01 | pending |
| T03 | Checked DLQ replay and API outcomes | T01, T02 | pending |
| T04 | Recoverable overwrite/staging/WAL | T01–T03 | pending |
| T05a | Mobile family error propagation | architecture | pending |
| T05b | Mobile non-destructive staged overwrite | T05a | pending |
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
