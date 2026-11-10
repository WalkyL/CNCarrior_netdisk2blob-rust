# qg-5 F1 Re-Submission Gate — ADR-007 High-Tier Review (round 2)

This records the second submission of the F1 remediation prototype to the
ADR-007 3-model gate, after the qg-3 findings were fixed. Reviewer pair:
**kimi-k3 + glm-5.3@max** (gpt-6-astra had no quota; user's fallback rule).

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg5-f1-resubmit-prompt.md` (80,971 bytes,
  SHA-256 `a03ae4d73bf100812984540d7bdc35a327b7afc6fe73e26b49a99f658ac02a73`).
- **Reviewers:** kimi-k3 (`qg5-kimi-response.json`, HTTP 200, 256s),
  glm-5.3@max (`qg5-glm53-response.json`, HTTP 200, 531s).
- **Verdict:** `request_changes` (both). All findings are **minor** except one
  `important` (glm, the `env_u32` doc/behavior contradiction — which is a real
  defect in a helper added in round 1, not a defect in the F1 offload itself).

## Round-2 findings (both reviewers) and their fixes

| # | Sev | Reviewer | Location | Problem | Fix landed |
|---|---|---|---|---|---|
| R1 | important | glm | `env_u32` (~34786) + call site 6454 | Doc & log say "clamp to u32::MAX" but `map_err(...).ok()` returned `None` on overflow, so `.unwrap_or(default)` silently returned `default` (3) — an operator setting `CCBG_REPLICATION_MAX_ATTEMPTS=5000000000` was told it's clamped but the process ran with 3. | **FIXED**: `env_u32` now returns `Some(u32::MAX)` on overflow (with the warning), matching its doc; non-numeric still falls back to `default`. |
| R2 | minor | kimi+glm | `DbExecutor` doc-comment, contract item 2 | Leftover persona reference "the contention astra flagged" — contradicts round-1 change-note #8. | **FIXED**: rephrased to "the contention that results when many blocking threads wait on one connection mutex." |
| R3 | minor | kimi+glm | `delete_object` enqueue `.map_err` closure (~37767) | Mixed-indentation argument list: first 3 args at 16-col, continuation args at old 12-col. Not rustfmt-clean. | **FIXED**: uniformly reindented the closure body. |
| R4 | minor | glm | B2 cast-scan evidence (34751/34779/34803) | The listed "cast sites" are `warn!` string-literal matches — the literal-aware claim was not applied to the `gatewayd_prod_panics.py` listing; true production count ≈67, not 70. | **FIXED**: `gatewayd_prod_panics.py` now uses the literal-aware stripper; production count corrected to **66** (u64=45, usize=9, u32=4, f64=4, i64=1, u128=2, u16=1). |
| R5 | minor | glm | `DbExecutor::execute` panic path + tests | The rewritten `resume_unwind` panic-propagation path is exercised by none of the 8 tests — a regression back to `.expect(...)` would pass the suite. | **FIXED**: added `execute_resumes_command_panic_with_original_payload`, which panics a command and asserts the awaiting task's `JoinError` carries the original payload. |

## What the reviewers accepted (round 2)

- **The 8 contract tests are now non-vacuous** — neither reviewer re-raised the
  A1–A3 vacuousness; both accept that the admission-bound, FIFO, and
  commit-before-success tests now exercise their contracts.
- **The identity cast removal** (`current_unix_ms() as u64` → direct) is accepted.
- **The panic-resume path** is the only remaining gap (R5), now fixed with a test.
- **The F1 offload semantics** (commit-before-success, admission bound, no
  lock-across-await, cancellation) are accepted as contract-faithful by both.

## Test evidence (updated, round 2)

Full `cargo test -p gatewayd --release` on the working tree:
**478 passed / 0 failed / 1 ignored** (965s). The 3 previously-reported failures
(`object_action_delete_writes_replayable_wal_metadata`,
`object_action_core_surfaces_wal_commit_warning_without_rollback`,
`object_action_core_rolls_back_after_metadata_failure`) are a **pre-existing,
order-dependent test-isolation defect** (shared `WAL_COMMIT_FAILURES_REMAINING`
global); on this run they passed because test scheduling did not overlap the
injection. They remain non-deterministic and are tracked separately (C1), not
caused by F1. The 9 `db_executor_tests` (8 contract + 1 panic-propagation) all
pass, stable across repeated runs.

## Gate status

The round-2 findings (R1–R5) are all fixed in the working tree. A **round-3
submission** is required to confirm the gate, since the reviewers returned
`request_changes` on the round-2 material. The remaining open items for round 3
are: re-verify the `env_u32` clamp behavior, the panic-propagation test, the
literal-aware cast count (66), and the reindent.

## Artifacts

- Prompt: `.cache/reviews/qg5-f1-resubmit-prompt.md`
- kimi-k3: `.cache/reviews/qg5-kimi-response.json`, `.cache/reviews/qg5-kimi-.final.txt`
- glm-5.3@max: `.cache/reviews/qg5-glm53-response.json`, `.cache/reviews/qg5-glm53-.final.txt`
- Diff: `.cache/f1-dbexecutor-proto-v2.diff`
