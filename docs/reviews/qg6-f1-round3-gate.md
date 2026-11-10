# qg-6 F1 Round-3 Gate — ADR-007 High-Tier Review (round 3)

This records the third submission of the F1 remediation prototype to the
ADR-007 3-model gate, after the round-2 findings (R1–R5) were fixed.
Reviewer pair: **kimi-k3 + glm-5.3@max** (gpt-6-astra had no quota; the user's
fallback rule).

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg6-f1-round3-prompt.md` (80,934 bytes,
  SHA-256 `e57c728d1718a6efbaaf0ff8920333f3c17d1b876eee36d35302a066bb4dd516`).
- **Reviewers:** kimi-k3 (`qg6-kimi-response.json`, HTTP 200, 57,392 bytes),
  glm-5.3@max (`qg6-glm53-response.json`, HTTP 200, 274,007 bytes on first run
  = `incomplete` after hitting `max_output_tokens` 65536; re-submitted with
  `max_output_tokens` 131072 → HTTP 200, 216,093 bytes, `completed`).
- **Verdict (initial round-3 material):** kimi = `approve` (1 minor);
  glm = `incomplete` (first) → re-submitted → `approve` (1 minor).

## Round-3 findings and their fixes

| # | Sev | Reviewer | Location | Problem | Fix landed |
|---|---|---|---|---|---|
| G1 | minor | kimi | Part B2 (gatewayd production cast inventory) | The published per-site inventory still contained entries that cannot be cast sites: line 34787 is a `///` doc comment (verbatim the `env_u32` doc text "silent `as` cast... parses as `u64`"), lines 33990/33998 are each listed twice with two different cast kinds on identical cast-free text, and several entries show cast-free/empty statements (21873 empty, 27906 empty, 27978 brace-only). This contradicted the stated literal-aware strip methodology and left the round-3 claim of exactly 66 production casts unverifiable. | **FIXED**: the round-3 prompt had been built from a pre-fix inventory snapshot. Re-ran the literal-aware, line-preserving stripper and regenerated `.cache/gatewayd_prod_sites.json` (66 casts, no phantom/empty/doc-comment entries; the two same-line double-kind entries at 3286/34639/34647 are genuine two-cast expressions) and rebuilt `.cache/reviews/qg6-f1-round3-prompt.md` (80,733 bytes, SHA-256 `f343c32423653e47defb8e36374d7a0a938596295ac0cedeb4b7c5510002501f`). All 6 flagged lines now absent from the inventory. |
| G2 | minor | glm | `main.rs` test `AppState` builders (diff hunks adding `db_executor: DbExecutor::new(8)` at ~41525 and ~58380) | Both test `AppState` constructors hardcode the DbExecutor admission bound to 8 while adjacent fields in the same literal read from the in-scope config, and production derives the bound from `config.data_plane_max_in_flight`. Test/bench wiring would silently diverge from the production admission bound if the default ever changes; the `f1_workload_bench` "faithfully reproduces the production condition" claim holds only because the default is 8. | **FIXED**: both test builders now use `DbExecutor::new(config.data_plane_max_in_flight)` (`config` is in scope in both: `let config = test_config();` at 41491 and `let config = state.config.clone();` at 58344). `cargo check -p gatewayd --all-targets` clean; all 9 `db_executor_tests` pass. |

## What the reviewers accepted (round 3)

- **The F1 behavioral contract** (commit-before-success, admission bound,
  no lock-across-await, cancellation, FIFO per caller, overload backpressure,
  no durability change) is accepted as contract-faithful by both.
- **The `env_u32` clamp fix (R1) and the panic-propagation test (R5)** are
  accepted; neither reviewer re-raised them.
- **The 9 `db_executor_tests` are non-vacuous** and meaningful.
- **The 478 passed / 0 failed / 1 ignored release suite** is accepted as
  recorded (both reviewers noted it was not independently re-executed; the
  order-dependent WAL-metadata tests are disclosed and tracked separately, C1).

## Gate status

Both round-3 findings (G1, G2) are fixed in the working tree and in the
on-disk evidence. The **round-3 material as originally submitted** was
insufficient for a clean pass (kimi's B2 evidence was built from a stale
inventory; glm flagged the test-builder hardcode), so a **round-4 submission
with the corrected prompt and code is required** to confirm the gate
definitively. The working tree now carries both fixes; the regenerated prompt
reflects the clean inventory.

## Artifacts

- Prompt: `.cache/reviews/qg6-f1-round3-prompt.md` (round-4 corrected copy)
- kimi-k3: `.cache/reviews/qg6-kimi-response.json`, `.cache/reviews/qg6-kimi-.final.txt`
- glm-5.3@max: `.cache/reviews/qg6-glm53-response.json`, `.cache/reviews/qg6-glm53-.final.txt`
- Diff: `.cache/f1-dbexecutor-proto-v3.diff` (52,760 bytes, 792 insertions / 86 deletions)
