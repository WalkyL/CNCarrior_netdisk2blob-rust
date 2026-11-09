# Review Evidence — Architecture optimization review (GLM-5.3@max, verified by gpt-6-astra@ultra)

This records the comprehensive architecture-level review of the CCBG Rust
workspace requested by the operator ("全面把代码看一下，整体架构上有没有可以优化的") and the
second-reviewer verification of that review by gpt-6-astra@ultra.

- **Review date:** 2026-11-09
- **First reviewer:** GLM-5.3 (@max reasoning effort)
- **Verifier (second reviewer):** gpt-6-astra (@ultra)
- **Scope:** architecture-level optimization opportunities only. Excludes
  micro-optimizations, the already-measured glibc memory arena (now 18.6 MiB
  idle RSS), and security-policy correctness.
- **Verdict (GLM-5.3):** meaningful space exists; highest value-per-governance
  cost = **F1** (decouple metadata store from the single-worker tokio runtime);
  largest structural debt = **F2** (single-file `gatewayd/src/main.rs` monolith).
- **Verdict (astra):** GLM-5.3 directionally correct on F1; F2 downgraded
  (extraction alone ≠ proven benefit); several findings downgraded to
  measure-first. Only F1 is a confirmed, actionable architecture finding, and it
  requires a measurement baseline before any offload change.

## GLM-5.3@max findings (final)

| ID | Severity | Area | Finding | Astra verdict |
|----|----------|------|---------|---------------|
| F1 | High | concurrency | Single `rusqlite::Connection` behind `Mutex`, 1 worker → DB calls (fsync-per-commit) may stall the runtime | **CONFIRM** (blocking-in-async; measured impact still needed) |
| F2 | High | structural | `main.rs` monolith (single file, ~58% of workspace) | **DOWNGRADE** (maintainability, not proven perf/reliability benefit) |
| F3 | Med | structural | `AppState` 28-field god-object | **DOWNGRADE** (no evidence field count causes defects) |
| F4 | Med | lifecycle | admin index chained `.replace()` over 1.27 MiB template per request | **DOWNGRADE** (profiling-led) |
| F5 | Med | lifecycle | `zero_probe_body` allocates up to 64 MiB zeros per chunk | **DOWNGRADE → measure-first** |
| F6 | Low | resilience | data-plane rate limit default 0 (unlimited) | **CONFIRM (low)** (operational policy, not defect) |
| F7 | Low | resilience | boot-time WAL replay failure semantics unverified | **DOWNGRADE → verification gate** |
| F8 | Low | structural | `metadata-store → replication-engine` dependency direction | **CONFIRM (low)** (note only) |

### Strengths (both reviewers)
Clean acyclic dependency graph; bounded resources (log ring 2 MiB/2000, semaphore
8, 2 replication workers, notify webhook only if configured); healthy 18.6 MiB
idle RSS; 1-CPU runtime sizing correct; ~36-line avg function size; self-contained
26.5 MB binary; admin security controls present.

### F1 verification (resolved after GLM-5.3, by source inspection)
- Metadata store uses `std::sync::Mutex<rusqlite::Connection>`, not
  `tokio::sync::Mutex`; every accessor locks and runs SQLite synchronously; no
  `journal_mode`/`cache_size`/`mmap`/`temp_store` beyond `foreign_keys = ON`.
- Sync DB calls (`enqueue_jobs_with_versions`) are invoked directly from `async fn`
  handlers (`copy_object` main.rs:35602, `delete_object` main.rs:37296) without
  `spawn_blocking`.
- Only **3** `spawn_blocking` calls exist in all of `gatewayd` (main.rs:14779,
  14859, 30840), all for backup archive I/O — none around the data-plane write path.
- The `std::sync::Mutex` guard is **not** held across an `.await` (zero `.await`
  in `crates/metadata-store/src/lib.rs`).

**Conclusion:** F1 is a **blocking-in-async hazard, not a deadlock hazard.** On the
1-CPU host with one tokio worker, a data-plane commit (with fsync) runs synchronously
on the only worker thread → every other in-flight request stalls for its duration.

### LOC reconciliation (resolves astra's one open item)
- `gatewayd/src/main.rs` = **63,040 lines** (`Measure-Object -Line`), and
  `crates/gatewayd/src` contains **exactly one** `.rs` file (`main.rs`).
- The GLM-5.3 inventory figure of 66,864 lines is **not reproducible** and was
  discarded. The "gatewayd 63,040 (58%)" figure was actually `main.rs`'s count.
- Net: the monolith is real and even more pronounced (one file, ~58% of the
  workspace, no `lib.rs`/module split) — the F2 structural fact stands even though
  astra downgrades its *benefit* until extraction is shown to pay.

## Astra's recommendation (final_priority)

1. **DO NOW — F1 baseline:** measure DB-call/commit duration + request latency
   under concurrent load on the 1-worker config; if stalls are material, prototype
   a **bounded, serialized DB actor** (no durability-pragma changes).
2. **DO NOW — WAL replay verification:** establish failure semantics + test coverage.
3. **DEFER — F2/F3 refactoring:** small seam-based extractions only, test after each.
4. **DEFER — F4/F5 optimizations:** profile template rendering + probe-buffer
   allocation under representative workloads first.
5. **DO NOT —** change WAL/synchronous settings or do a large rewrite on this
   review alone; keep the ADR-007 3-model gate + no-deploy/no-push in force.

## Evidence artifacts

- `.cache/glm53-arch-ask.md` — GLM-5.3 prompt (sha `366a3d7c…`)
- `.cache/glm53-arch.json` — GLM-5.3 raw review (33.8 KB, 236.9s, effort max)
- `.cache/astra-arch-ask.md` — astra re-check prompt (sha `9e50da23…`)
- `.cache/astra-arch.json` — astra re-check (128.4s, finish_reason stop)
- `.cache/astra-arch-loc-reconciliation.md` — LOC reconciliation (Measure-Object)

## Decision status

Analysis-only; **no code was changed** by this review. The F1 measurement
baseline is the immediate next step if the operator elects to pursue it. No
deployment or push occurred; the ADR-007 gate and frozen architecture remain in
force.
