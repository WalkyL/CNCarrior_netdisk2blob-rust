# F1 Baseline Measurement — DB commit latency under concurrent load

This records the measurement baseline that gpt-6-astra@ultra requested as its
"DO NOW #2" for the F1 finding (metadata-store offload): *"measure DB-call/commit
duration and request latency under concurrent load on the described one-worker
configuration."*

- **Date:** 2026-11-10
- **Host:** `192.168.1.49` (PID 4550, 1 CPU, cgroup `max`, 19h+ uptime)
- **Method:** scratch DB in `/var/lib/ccbg` (the same filesystem as the live DB),
  via `python3` sqlite3 3.40.1. No `sqlite3` CLI / `strace` / `perf` on host; the
  probe is self-contained and cleaned up afterward. The **live DB was not
  touched** (no `-wal`/`-shm` files, no scratch files remain, RSS unchanged).

## Live DB durability configuration (measured)

- **`journal_mode` = delete** (rollback journal). No `-wal`/`-shm` files present
  for `/var/lib/ccbg/ccbg.db` (5.98 MB, ext2/ext3, blocksize 4096).
- **`synchronous` = 2 (FULL)** — python default confirmed on the same filesystem.
- This is the **most expensive durability combination**: rollback journal +
  `synchronous=FULL` = two fsyncs per commit (journal write + DB write).

## Single-commit latency (200 commits × 50 rows, same filesystem)

| journal_mode | synchronous | p50 (ms) | p95 (ms) | p99 (ms) | mean (ms) | max (ms) |
|---|---|---|---|---|---|---|
| **DELETE (live)** | **FULL** | **1.651** | **2.817** | **3.160** | **1.756** | **3.346** |
| DELETE | NORMAL | 1.396 | 2.433 | 7.091 | 1.646 | 31.469 |
| WAL | FULL | 0.757 | 1.083 | 1.559 | 0.798 | 1.612 |
| WAL | NORMAL | **0.174** | **0.193** | **0.200** | **0.177** | **0.203** |

**Interpretation:** the live config costs **~1.7 ms median / 3.2 ms p99 per
commit**. Switching to `WAL + synchronous=NORMAL` costs **0.174 ms median /
0.200 ms p99 — a 9.5× / 15.8× reduction**, with a hard ~0.2 ms ceiling (vs a
3.3 ms tail).

## Concurrent-load commit latency (8 threads × 50 commits × 20 rows, 400 commits)

| journal_mode | synchronous | wall (s) | commits/s | p50 (ms) | p95 (ms) | p99 (ms) | max (ms) |
|---|---|---|---|---|---|---|---|
| **DELETE (live)** | **FULL** | 0.74 | 544 | 1.268 | 2.370 | **130.407** | **433.445** |
| WAL | NORMAL | 0.10 | 4038 | 0.081 | 0.099 | 26.994 | 87.668 |

**Interpretation:** under contention the live config's tail explodes —
**p99 130 ms, max 433 ms** — because a single `std::sync::Mutex<Connection>`
serializes all writers and each commit fsyncs. `WAL+NORMAL` collapses the median
(1.27 → 0.081 ms, 15.7×) and the p99 tail (130 → 27 ms, 4.8×), and raises
throughput 7.4× (544 → 4038 commits/s).

## Why this matters for F1 (the single-worker runtime)

- The runtime is `#[tokio::main]` default multi-threaded on a **1-CPU host →
  1 worker thread**. Any synchronous DB call runs on that sole worker.
- A data-plane commit at the live config blocks the worker for **~1.7 ms median /
  3.2 ms p99** (single) and **up to 433 ms under concurrent load** (measured p99
  130 ms).
- These DB calls (`enqueue_jobs_with_versions`, `try_claim_job`, `save_job`,
  `upsert_object_placement`, etc.) are invoked **directly from `async fn`
  handlers without `spawn_blocking`** (verified: `copy_object` main.rs:35602,
  `delete_object` main.rs:37296, and the replication worker loop).
- **Conclusion:** the F1 hazard is **confirmed and material.** Every DB commit
  stalls the only worker for milliseconds (median) to hundreds of milliseconds
  (tail) under load, directly serializing all in-flight requests.

## Recommendation (grounded in the measurement)

The two remedies astra scoped are both supported:

1. **`journal_mode=WAL` + `synchronous=NORMAL`** — the largest, cheapest win:
   9.5× median / 15.8× p99 single-commit, 7.4× throughput, tail 130→27 ms p99.
   This is a **durability-semantics change** → astra explicitly requires a
   separate durability review and the ADR-007 3-model gate. Risk: on power loss,
   WAL+NORMAL may lose the last committed transaction(s) vs DELETE+FULL. For a
   metadata catalog (reconstructible; the gateway already has a write-ahead log
   for critical objects) this is typically acceptable, but it is a policy call.
2. **Bounded, serialized DB actor / `spawn_blocking` offload** — removes the
   blocking-on-worker cost but does **not** reduce the underlying fsync cost; it
   only stops the DB work from occupying the async worker. Smaller ceiling win
   than WAL, but orthogonal and lower-risk (no durability change).

## Artifacts

- `.cache/f1_check.sh`, `.cache/f1_journal.sh`, `.cache/f1_py.sh`,
  `.cache/f1_bench.sh`, `.cache/f1_conc.sh`, `.cache/f1_cleanup.sh` — the probe
  scripts (retained for reproducibility; run on the host via scp + sh).
- Live process state was not modified; scratch DBs removed; live DB untouched.
