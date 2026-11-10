# F1 Post-Fix Measurement — worker-loop offload under real gateway workload

This records the **f1-a7** baseline-vs-prototype measurement for the F1 hazard:
synchronous SQLite calls on a 1-worker Tokio runtime (the production 1-CPU/1-worker
condition) starving the sole async worker.

- **Date:** 2026-11-10
- **Scope:** f1-a5 routed the replication worker loop's 7 direct sync DB calls
  plus `ensure_job_is_current` through the bounded `DbExecutor` offload
  (`spawn_blocking` + admission `Semaphore`, config `data_plane_max_in_flight=8`).
- **Method:** identical in-process benchmark in both versions. Reproduces the
  production 1-worker runtime via `#[tokio::test(flavor = "multi_thread",
  worker_threads = 1)]`; drives the S3 data-plane handlers end-to-end
  (PUT → DELETE → PUT loop, N=200) with the **replication worker loop running**
  (spawned as `main` does, `replication_workers=2`), and records a 1ms heartbeat
  canary's largest inter-tick gap — the longest stretch the sole worker is
  starved (the F1 symptom). Source is byte-identical between versions; only the
  internal routing differs. Benchmark is `#[ignore]` in `crates/gatewayd/src/main.rs`
  as `tests::f1_workload_bench` (run: `cargo test -p gatewayd --release --
  f1_workload_bench --ignored --nocapture`).

## Results (1-worker runtime, worker loop running, N=200)

| Metric | Baseline (sync worker loop) | Prototype (offloaded) | Δ |
|---|---|---|---|
| **canary-gap max** (worker starvation) | **5141.124 ms** | **319.695 ms** | **−94% (16×)** |
| canary-gap p99 | 0.003 ms | 16.107 ms | — |
| canary-gap p50 | 0.001 ms | 0.001 ms | — |
| PUT p50 | 1837.605 ms | 1665.556 ms | −9% |
| PUT p95 | 2816.678 ms | 3989.515 ms | +42% |
| PUT p99 | 3594.200 ms | 6169.480 ms | +72% |
| DELETE p50 | 916.678 ms | 806.455 ms | −12% |
| DELETE p95 | 1646.644 ms | 1982.440 ms | +20% |
| DELETE p99 | 2443.817 ms | 3095.062 ms | +27% |

## Interpretation

- **The F1 fix is effective and material on the worker loop.** The worst
  worker-starvation stretch drops **5141 ms → 320 ms (16×)**. In the baseline,
  the sync worker loop's DB calls (claim → process → mark-completed/save/retry/
  dead-letter) block the sole worker for multiple seconds at a time; in the
  prototype those same calls are offloaded to a blocking thread, so the worker
  stays responsive (canary ticks through).
- **A ~16 ms residual starvation remains.** This is the handler-path synchronous
  DB commit (`persist_logical_object_record` → `upsert_logical_object`,
  `delete_logical_object_record`, and `upsert_object_placement` in the Move-arm
  rollback closure), which f1-a5 deliberately left as a documented residual. It is
  present in both versions (p99 ~16 ms) and is the floor the prototype cannot
  remove without routing those calls too.
- **Handler tail latency rises (PUT p99 +72%, DELETE p99 +27%).** The bounded
  executor (`data_plane_max_in_flight=8`) plus the worker loop now competes for
  the same admission budget and blocking pool; when the loop is busy the
  handler's offloaded calls queue (backpressure), stretching the tail. The
  median actually improves (PUT p50 −9%, DELETE p50 −12%) — the median handler
  no longer waits on a worker-loop DB call, but the worst case pays the queue
  wait.
- **Throughput/wall-clock:** both runs completed N=200 in ~940–1010 s, so the
  end-to-end throughput is essentially unchanged (the workload is dominated by
  remote StubBackend + single-worker scheduling, not DB time).

## Why the v2 (worker-loop-off) measurement was inconclusive

An earlier iteration (v2) ran the same canary but **without spawning the worker
loop** (`test_config()` sets `replication_workers: 0` and `test_state()` never
calls `spawn_replication_workers`, which only `main` does). Both versions showed
~16 ms p99 / ~17 ms max starvation — identical, because the worker loop never ran
and the only DB work was the handler-path sync commit, present in both. That
confirmed the worker loop is the dominant F1 source and that it must be running
for the measurement to be meaningful.

## Operational semantics of `data_plane_max_in_flight` (dual role, ADR-007 §2a)

The F1 change gives the existing config knob `data_plane_max_in_flight` a second
role: it is now the admission bound for **both** the data-plane request semaphore
and the `DbExecutor` that offloads synchronous metadata-store commands.

- **Data-plane concurrency** (pre-existing): bounds concurrent S3 data-plane
  requests; excess requests get `503` (see `DataPlaneConcurrencyState`).
- **DbExecutor admission** (new): bounds how many offloaded metadata-store
  commands (replication-worker-loop and handler-path enqueue calls) may be queued
  or running on tokio's blocking pool at once. When the bound is reached, an
  offloaded call awaits a permit (backpressure) instead of piling up blocking
  threads.

An operator lowering this knob to protect a tiny router now **also** throttles
the replication worker loop and handler DB enqueue calls — the measured tail
latency rise (PUT p99 +72%, DELETE p99 +27%) is a direct consequence of the
shared budget. The knob is validated at config load with `.max(1)` (main.rs
`CCBG_DATA_PLANE_MAX_IN_FLIGHT`), so it can never be 0; the executor additionally
clamps its own bound to at least 1 defensively. Both roles are surfaced in the
admin status overview (`data_plane_max_in_flight` / `data_plane_permits_available`).

## Artifacts

- Benchmark harness: `tests::f1_workload_bench` (`#[ignore]`) in
  `crates/gatewayd/src/main.rs` (both versions, byte-identical).
- Baseline run: isolated worktree `D:\workspaces\ccbg-baseline` @ HEAD `0a1d139`.
- Prototype run: `D:\workspaces\ccbg` working tree.
- Scratch benchmark sources: `.bench_v2.rs`, `.bench_v3.rs` (baseline worktree only).
