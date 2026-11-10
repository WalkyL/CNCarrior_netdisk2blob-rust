# qg-7 F1 Round-4 Gate — ADR-007 High-Tier Review (round 4, APPROVED)

This records the fourth submission of the F1 remediation prototype to the
ADR-007 3-model gate, after the round-3 findings (G1, G2) were fixed.
Reviewer pair: **kimi-k3 + glm-5.3@max** (gpt-6-astra had no quota; the user's
fallback rule).

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg7-f1-round4-prompt.md` (81,486 bytes,
  SHA-256 `f04694aa11913c981e3b423a2a5786fac51b3511925d5413cbb8364dd2912d16`).
- **Reviewers:** kimi-k3 (`qg7-kimi-response.json`, HTTP 200, 577s, 91,032 bytes),
  glm-5.3@max (`qg7-glm53-response.json`, HTTP 200, 455s, 156,448 bytes).
- **Verdict:** `approve` (both), `findings: []`.

## Round-4 result

Both reviewers returned **`approve` with an empty findings list**. The F1
remediation prototype passes the ADR-007 High-Tier Holistic Review Gate.

### What each reviewer confirmed

- **kimi-k3**: accepted the 9 `db_executor_tests` as non-vacuous and
  contract-meaningful; confirmed the production count of 66 casts is internally
  consistent (45+9+4+4+1+2+1=66) and the 0 production `.unwrap()` sites; noted
  the `env_u32` clamp and panic-propagation fixes are carried forward.
- **glm-5.3@max**: confirmed commit-before-success is preserved (the new
  commit-to-publish cancellation window is durability-safe); judged the three
  narrowing cast sites (4487 `value as u32`, 14769 `entries.len() as u32`,
  38942 `part_number as u16`) non-gratuitous against domain bounds; confirmed
  the gating quantities (66 production casts, 0 production `.unwrap()`,
  0 TODO/FIXME/HACK) are internally consistent.

### Reviewer limitations (not findings — none were re-raised as defects)

Both reviewers noted the change-documentation artifacts
(`docs/reviews/f1-postfix-measurement.md`, `docs/reviews/f1-baseline-measurement.md`)
were not supplied verbatim and were accepted per the prompt's assertions; the
benchmark raw output and the 478-passed release run were accepted as recorded,
not re-executed. The pre-existing, order-dependent `WAL_COMMIT_FAILURES_REMAINING`
test-isolation defect was explicitly out of scope and tracked separately.

### Immaterial narrative inconsistencies (reconciled)

Both reviewers flagged a gatewayd total cast count reported as **85** in the
round-3 carry-forward changelog versus **86** in B3. The authoritative value
from `.cache/objective_report.json` is **85** (`any_cast: 85`,
`same_width_cast: 81`, per-file sum 85). The "86" was a stale snapshot figure in
the prompt narrative. Both prompt builders (`build_qg6_prompt.py`,
`build_qg7_prompt.py`) now state **gatewayd 1.26/1K (85)** consistently, and the
F1 diff contributes exactly one production cast (`current_unix_ms() as u128` at
line 28191), already inside the 85. The gating quantity — **66 production
casts** — is unchanged and internally consistent.

## Round-4 fixes carried (G1, G2 from round 3)

| # | Sev | Reviewer | Fix landed |
|---|---|---|---|
| G1 | minor | kimi | B2 cast inventory rebuilt from the clean literal-aware scan: 66 production casts, no phantom/doc-comment/empty entries. |
| G2 | minor | glm | Both test `AppState` builders now use `DbExecutor::new(config.data_plane_max_in_flight)` (was hardcoded `DbExecutor::new(8)`). |

## Verification after G2 fix

- `cargo check -p gatewayd --all-targets`: clean (only pre-existing warnings).
- `cargo test -p gatewayd --release db_executor`: **9 passed / 0 failed** (0.12s).

## Gate status: **APPROVED**

The F1 remediation prototype (bounded `DbExecutor` via `spawn_blocking` +
admission `Semaphore`) has passed the ADR-007 3-model gate. Round-1 (3 findings),
round-2 (5 findings), and round-3 (2 findings) all returned `request_changes`
and were fixed; round-4 returns `approve` with an empty findings list from both
kimi-k3 and glm-5.3@max. The gate is satisfied for the F1 change.

## Artifacts

- Prompt: `.cache/reviews/qg7-f1-round4-prompt.md`
- kimi-k3: `.cache/reviews/qg7-kimi-response.json`, `.cache/reviews/qg7-kimi-.final.txt`
- glm-5.3@max: `.cache/reviews/qg7-glm53-response.json`, `.cache/reviews/qg7-glm53-.final.txt`
- Diff: `.cache/f1-dbexecutor-proto-v3.diff` (52,760 bytes, 792 insertions / 86 deletions)
- Gate chain: `docs/reviews/qg3-gatewayd-review.md` (r1),
  `docs/reviews/qg5-f1-resubmit-gate.md` (r2), `docs/reviews/qg6-f1-round3-gate.md` (r3),
  `docs/reviews/qg7-f1-round4-gate.md` (r4, APPROVED)
