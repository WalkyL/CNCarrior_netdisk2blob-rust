# ADR-007 Gate Evidence — F1 DB-executor offload (prototype)

This records the ADR-007 holistic review gate result for the F1 remediation
prototype in `crates/gatewayd/src/main.rs` (uncommitted working-tree diff, base
`0a1d139`, 657 insertions / 71 deletions, 728 changed lines).

- **Date:** 2026-11-10
- **Gate detection** (`scripts/check-review-gate.py`):
  - M1: 728 changed lines ≥ 500 → fires
  - M4: security marker `key` added (conservative keyword match on object-key
    parameters, not crypto keys — benign, but fires by rule)
  - M5: hot-path marker `data-plane` added → fires
  - M3 (frozen contract), M7 (`unsafe`/`static`), M8 (admin/auth chain) did
    **not** fire. Required reviewer set per ADR-007 §3 for M4: **all three**
    (GLM-5.3, kimi-k3, astra).

## Reviewer verdicts

| Reviewer | Verdict | Findings |
|---|---|---|
| **GLM-5.3** | **request_changes** | 3 important + 7 minor |
| **kimi-k3** | **request_changes** | 2 important + 5 minor |
| **astra** | **request_changes** | 2 important + 4 minor |

All three independently returned **request_changes**; the ADR-007 gate therefore
**blocks** acceptance until the findings are addressed and re-reviewed.

## Consolidated findings (deduplicated across the 3 reviewers)

### Important

1. **Vacuous admission-bound test** — `execute_respects_admission_bound`
   collects lazy `execute` futures then awaits them sequentially, so commands
   never overlap and `peak <= 2` passes even with the semaphore removed (all 3).
   *Fix:* drive futures concurrently (`join_all`/`tokio::spawn`) and assert both
   `peak >= 2` and `peak <= 2`.
2. **Vacuous FIFO-order test** — `execute_dispatch_order_is_fifo_per_caller`
   sequentially awaits lazy futures, so order `[0,1,2,3,4]` is trivial (all 3).
   *Fix:* submit concurrently with bound 1 and assert dispatch order.
3. **No off-worker assertion / test name overclaims** — 
   `execute_runs_command_on_blocking_thread_and_returns_ok` claims execution on a
   blocking thread but asserts nothing about it; the suite passes for an inline
   implementation, leaving the core F1 property guarded only by the ignored
   benchmark (GLM). *Fix:* capture `std::thread::current().id()` and assert it
   differs from the test task's thread.
4. **Benchmark root future runs via `block_on` off the worker** —
   `#[tokio::test(flavor = "multi_thread", worker_threads = 1)]` runs the root
   future on the harness thread, so `app.oneshot(...).await` in the benchmark
   does **not** execute on the 1-worker runtime; the measured handler latency and
   worker-starvation may not faithfully reproduce the production 1-worker
   condition (astra). *Fix:* wrap the handler workload in a `tokio::spawn` task
   and await its handle so handlers, replication loops, and the heartbeat run on
   the worker.

### Minor

5. **Failure-test comment contradiction** — 
   `execute_does_not_report_success_when_command_failed_before_commit`'s comment
   ("must not increment the commit counter") contradicts the helper (increments
   unconditionally) and the assertion (expects 1); the helper cannot model
   "failed before commit" (all 3). *Fix:* increment only on `Ok` (model
   commit-on-success/rollback-on-failure), assert 0 on failure.
6. **`.expect("...is never aborted")` mislabels the reachable failure** — for
   `spawn_blocking` the only `JoinError` is a command panic (abort has no
   effect), so the message misidentifies the cause; `.expect` raises a fresh
   panic instead of resuming the original (GLM, kimi). *Fix:*
   `.await.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()))`, or
   reword the message.
7. **Cancellation test timing-dependent** — 
   `execute_cancelled_while_waiting_for_permit_does_not_consume_permit` relies on
   120ms/10ms sleeps; scheduling delays can make it flaky (astra). *Fix:* hold
   the holder with an explicit release signal and poll the waiter until `Pending`.
8. **Heartbeat Burst behavior + unbounded gap Vec** — the canary uses `interval`'s
   default Burst policy (overdue ticks fire in a burst, producing near-zero gaps
   that mask the real starvation), and appends to an unbounded `Vec` (astra). 
   *Fix:* `MissedTickBehavior::Skip`, and use a bounded histogram / track max
   separately.
9. **Doc/contract mismatch: bound clamp undocumented** — `Semaphore::new(...max(1))`
   silently rewrites a configured 0 to 1, contradicting the doc "at most
   `max_in_flight`"; and the claim "The bound is documented at the call site" is
   false (no comment at the construction site) (GLM, kimi). *Fix:* document the
   clamp or validate at config load; add a construction-site comment.
10. **No `docs/` update for the dual role of `data_plane_max_in_flight`** — the
    knob now also bounds DB-command admission, but no `docs/` change ships
    (ADR-007 §2a) (GLM). *Fix:* add a config-reference/ops note.
11. **Formatting: misaligned `delete_object` arg list + over-long dead-letter
    line** (114 cols vs the 100-col sibling split) — not rustfmt-clean (GLM,
    kimi). *Fix:* run rustfmt over the touched regions.
12. **Magic literal `DbExecutor::new(8)` in test constructors** — hardcodes the
    bound though `config` is in scope (GLM). *Fix:*
    `DbExecutor::new(config.data_plane_max_in_flight)`.
13. **Mixed register / unexplained shorthand in doc comments** — "gpt-6-astra"
    vs "astra", "astra step 1/3", "f1-a5" are unintelligible without the review
    thread (GLM). *Fix:* use durable artifact references or expand.

## Disposition

The gate **blocks**. The production implementation is accepted as faithful to the
contract by all three (no production-behavior defect found), but the **test
evidence is inadequate** (vacuous/overclaiming contract tests) and the
**measurement is compromised** (benchmark root future not on the worker, heartbeat
Burst masking). These must be fixed and the change re-submitted to the gate.

## Artifacts

- Prompt: `.cache/reviews/f1-dbexecutor-prompt.md` (SHA-256
  `e3e48ded1295b5b7c4259329a04215edc66b95deda7abe61ef5d42d92cec3d36`)
- Reviewer responses: `.cache/reviews/f1-glm53-response.json`,
  `.cache/reviews/f1-kimi-response.json`, `.cache/reviews/f1-astra-response.json`
- Raw reviewer text: `.cache/reviews/f1-glm53-response.json.text.txt`,
  `.cache/reviews/f1-kimi-response.text.txt`
- Parsed (astra, clean JSON): `.cache/reviews/f1-astra-parsed.json`
- Diff under review: `.cache/f1-dbexecutor-proto.diff`
- Measurement evidence: `docs/reviews/f1-postfix-measurement.md`,
  `docs/reviews/f1-baseline-measurement.md`
