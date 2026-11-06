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
| T07 | Four-provider exact list prefix preservation | Mobile file waits T05 | pending |
| T08a | Action-scoped CDP checkpoints | architecture | pending |
| T08b | Explicit JavaScript value binding/catalog migration | T08a | pending |
| T09 | Encryption declared-size validation | architecture | pending |
| T10 | Smoke harness failure propagation/regression | architecture | pending |
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
