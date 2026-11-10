# qg-3 Holistic Review — F1 Prototype Region + gatewayd Hotspots (ADR-007 High-Tier Gate)

This records the batched holistic review (qg-3) of the F1 prototype region and the
gatewayd production hotspots under the project-wide quality directive, submitted to
the reviewer pair **kimi-k3 + glm-5.3@max** (gpt-6-astra had no quota; the user's
fallback rule governed the pair). Both reviewers returned `request_changes`.

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg3-gatewayd-prompt.md` (73,285 bytes, SHA-256
  `08afb3590d59caaa2e1b79a2c5ac68d0925b6a56d246d7ee35bc53f172275e22`) — Part A: the F1
  prototype diff; Part B: gatewayd production panic/cast inventories + project-wide
  type-conversion hotspot evidence.
- **Reviewers:** kimi-k3 (`qg3-kimi-response.json`, HTTP 200), glm-5.3@max
  (`qg3-glm53-response.json`, HTTP 200).
- **Verdict:** `request_changes` (both reviewers).

## Consolidated findings

### A. F1 contract-test structural defects (both reviewers, `important`)

These are the substantive defects: the 8 `db_executor_tests` do not actually exercise
the contract points they claim to test.

| # | Severity | Location | Problem (consolidated) | Fix |
|---|---|---|---|---|
| A1 | important | `db_executor_tests::execute_respects_admission_bound` | Vacuous: futures are pushed unpolled then awaited **one at a time**, so at most one command is ever in flight; `peak` can never exceed 1. Contract items 2 (admission bound) & 6 (overload backpressure) are untested. | Drive the commands concurrently (`tokio::spawn` each `execute`, or `join_all`), assert `peak >= 2` AND `peak <= 2` (bound=2, six submissions). |
| A2 | important | `db_executor_tests::execute_dispatch_order_is_fifo_per_caller` | Same structural flaw: five futures awaited sequentially → observed order is fixed by await order, not the semaphore permit queue. Contract item 5 (FIFO per caller) is not exercised. | Spawn all five concurrently with bound 1 so real permit queueing occurs; join; assert submission order. |
| A3 | important | `db_executor_tests::execute_does_not_report_success_when_command_failed_before_commit` | Contradicts itself: comment says a failing command "must not increment the commit counter", but `committed_command` increments **unconditionally** and the test asserts commits == 1. The failed-before-commit scenario is never modeled; the counter is silently a run counter. | Make `committed_command` increment only on the `Ok` path (modeling commit-before-success), assert `commits == 0` on the error path; or rename to a run counter and delete the contradicted comment. |

### B. F1 production-code defects (both reviewers, `minor`)

| # | Severity | Location | Problem (consolidated) | Fix |
|---|---|---|---|---|
| B1 | minor | `DbExecutor::execute` `.expect("db executor blocking task is never aborted")` | Message misnames the only reachable failure: the JoinHandle is never exposed, so the task cannot be aborted; the only reachable JoinError is a **panicked command**. The doc-comment claims "the panic propagates", but the implementation raises a new expect panic whose message says "never aborted" and buries the original payload. | On JoinError branch on `is_panic()`: resume the original payload via `try_into_panic()` + `resume_unwind`, else panic with an accurate "aborted" message; or at minimum reword to `.expect("db executor command panicked")`. |
| B2 | minor | worker loop call site (main.rs:28257) `current_unix_ms() as u64` | Gratuitous identity cast: `current_unix_ms()` already returns `u64` (the baseline passed it uncast to `store.mark_job_dead_letter`). Exactly the unnecessary conversion the type-conversion directive targets. | Drop `as u64`, pass `current_unix_ms()` directly. |
| B3 | minor | `mark_job_dead_letter_offload` body + `execute_respects_admission_bound` assert | Two new lines exceed rustfmt's default 100-column max_width (114 and 110 columns), inconsistent with the file's formatting. | Wrap the closure args and the assert args (run `cargo fmt`). |
| B4 | minor | `DbExecutor::new` | Doc/impl mismatch: doc says "at most `max_in_flight` commands" but `Semaphore::new(max_in_flight.max(1))` clamps — `new(0)` admits one. Either document the clamp or enforce `>= 1` at config load and drop the clamp. |
| B5 | minor | `DbExecutor` + `f1_workload_bench` doc-comments | Permanent in-code docs embed transient persona/iteration bookkeeping: "per ADR-007 / gpt-6-astra F1 review", "the failure mode astra called out", "astra step 1/3", "real-workload v3". Not self-contained; provenance unverifiable. | Replace persona references with durable in-repo citations (ADR-007, `docs/reviews/f1-baseline-measurement.md`, `docs/reviews/f1-postfix-measurement.md`); drop "v3"/"step 1/3" bookkeeping. |
| B6 | minor | `db_executor_tests::execute_runs_command_on_blocking_thread_and_returns_ok` | Name claims execution "on a blocking thread" but nothing verifies thread placement; verifies only single execution + Ok passthrough. | Capture `std::thread::current()` inside the command and assert it differs from the caller thread, or rename. |

### C. Pre-existing / non-diff findings (not introduced by F1)

| # | Severity | Location | Problem | Fix |
|---|---|---|---|---|
| C1 | important | main.rs:37894 `WAL_COMMIT_FAILURES_REMAINING` (`OnceLock<Mutex<u32>>`) | Pre-existing test-isolation defect: global failure-injection counter leaks between concurrently-running tests, reproducibly failing `object_action_delete_writes_replayable_wal_metadata`, `object_action_core_surfaces_wal_commit_warning_without_rollback`, `object_action_core_rolls_back_after_metadata_failure`. Same 3 fail on baseline `0a1d139`. Permanently-red suite masks future regressions in the WAL-commit paths F1 depends on. | Make injection per-`AppState` (consistent with the per-state "wal commit failure gate" lock) or serialize the affected tests with a shared static mutex. |
| C2 | minor | Part B1: main.rs:8772-8776, 14920-14928 (9 production `.unwrap()`) | Not genuinely fallible (parse compile-time constants) but deviates from the file's convention of `.expect` with an invariant message for the identical operation class (e.g. 35344, 8645). | Convert the 9 to `.expect("<literal> should be a valid <type>")`. |
| C3 | minor | main.rs:6410 `replication_max_attempts: env_u64(...) as u32` (pre-existing) | Operator-supplied u64 narrowed with bare `as`; values above `u32::MAX` wrap silently (4294967296 → 0, disabling retries), unlike neighboring env parsing that warns. | `u32::try_from(...).unwrap_or(u32::MAX)` with a `warn!` on out-of-range. |
| C4 | minor | Part B2 evidence: main.rs:34691, 34719 | Two listed "casts" are not type conversions — matches of the text `as u64`/`as usize` inside `warn!` **string literals**. The "71 production casts" count is overstated (actual 69). | Re-run the scan AST-aware (exclude string-literal content) and restate the count. |
| C5 | minor | ADR-007 §2a documentation gap | The change makes `data_plane_max_in_flight` double as the DbExecutor admission bound — a backpressure budget now shared by handler-path enqueue helpers and the worker loop. No `docs/` update describes this coupling or the offload behavior; an operator tuning the knob cannot discover its second effect. | Document the dual role of `data_plane_max_in_flight` (backpressure + tail-latency trade-off) in `docs/reviews/f1-postfix-measurement.md` or the config reference. |

## What the reviewers accepted

- **Production implementation is contract-faithful** (both reviewers): the offload
  routing, the `Semaphore` admission bound from existing config, and the
  commit-before-success design were accepted. Neither reviewer found a defect in the
  offload path's semantics.
- **The `as u128` at 28099** merely relocates a pre-existing widening cast (not a new
  conversion); the **only genuinely new conversion** is the `as u64` at 28257 (B2).
- **The 9 production `.unwrap()` sites** are not genuinely fallible paths (constant
  literals); C2 is a style/consistency concern, not a panic-path defect.
- **metadata-store / object-crypto cast hotspots** (B3 evidence): the "inherent, not
  gratuitous" characterization was accepted as supplied.
- **The 25 `#[allow(dead_code)]`** in gatewayd could not be per-site assessed from the
  supplied counts (both reviewers list this as a limitation).

## Limitations (both reviewers)

1. The 3-failing-tests pre-existing claim was accepted as supplied, not independently reproduced.
2. Whether any caller holds a `std::sync::Mutex` guard across the new `.await` points could not be verified from the hunks.
3. `data_plane_max_in_flight`'s config-load validation and `current_unix_ms()`'s signature were inferred, not read.
4. `#[allow(dead_code)]` / commented-block / long-identifier sites were supplied only as counts.

## Decision

**BLOCKED on the F1 gate (ADR-007)** at review time: the contract tests (A1–A3) were
vacuous. Findings have since been addressed in the working tree (see resolution
below). Re-submit the F1 gate to the reviewer pair after the fixes land.

## Resolution status (fixes applied after this review)

All confirmed findings have been fixed in `crates/gatewayd/src/main.rs` and the
docs; the fix set is ready for the re-submission gate.

| Finding | Status | Fix landed |
|---|---|---|
| A1 admission-bound vacuous | **FIXED** | `execute_respects_admission_bound` now `tokio::spawn`s each `execute`, asserts `peak == 2` (reached and not exceeded). |
| A2 FIFO vacuous | **FIXED** | `execute_dispatch_order_is_fifo_per_caller` now spawns all five concurrently with bound 1, asserting real permit-queue order. |
| A3 failed-before-commit self-contradiction | **FIXED** | `committed_command` increments only on the `Ok` path; the failure test now asserts `commits == 0`. |
| B1 expect misnames reachable failure | **FIXED** | `execute` now resumes the original panic via `resume_unwind(join_error.into_panic())` on `is_panic()`, else panics "aborted". |
| B2 gratuitous `as u64` (28257) | **FIXED** | `current_unix_ms()` passed directly; cast removed. |
| B3 over-length lines (2) | **FIXED** | `mark_job_dead_letter_offload` closure wrapped; bound-test `assert_eq!` wrapped. |
| B4 doc/impl mismatch on clamp | **FIXED** | `DbExecutor::new` doc now states the `max(1)` clamp and its rationale. |
| B5 persona/iteration bookkeeping | **FIXED** | Removed "gpt-6-astra"/"astra step 1/3"/"v3"/"f1-a5" from doc-comments and `println!`. |
| B6 test name overclaims thread placement | **FIXED** | Test now captures `std::thread::current()` in the closure and asserts it differs from the caller thread. |
| C1 pre-existing WAL global leak | **tracked** | Pre-existing defect, not caused by F1; tracked separately (not in this change). |
| C2 9 unwrap→expect | **FIXED (5 production)** | The 5 production sites (CSP/XFO/nosniff/referrer-policy/HSTS) now use `.expect(...)`; the other 4 (`test_app_config`) are test-support. |
| C3 env u64→u32 silent narrowing | **FIXED** | New `env_u32` validates/clamps with a warning; `replication_max_attempts` uses it. |
| C4 scan over-count | **FIXED** | `objective_scan.py` is now literal-aware; gatewayd cast count corrected to 86 (prod 70). |
| C5 docs gap (dual-role knob) | **FIXED** | `docs/reviews/f1-postfix-measurement.md` gained an "Operational semantics of `data_plane_max_in_flight`" section. |

## Artifacts

## Artifacts

- Prompt: `.cache/reviews/qg3-gatewayd-prompt.md` (SHA-256 `08afb3590d59caaa2e1b79a2c5ac68d0925b6a56d246d7ee35bc53f172275e22`)
- kimi-k3 response: `.cache/reviews/qg3-kimi-response.json` / `.cache/reviews/qg3-kimi-response.text.txt`
- glm-5.3@max response: `.cache/reviews/qg3-glm53-response.json` / `.cache/reviews/qg3-glm53-response.text.txt`
- Hotspot inventories: `.cache/gatewayd_prod_sites.json`, `.cache/gatewayd_hotspots.json`
- Request payloads: `.cache/reviews/qg3-kimi-request.json`, `.cache/reviews/qg3-glm53-request.json`
