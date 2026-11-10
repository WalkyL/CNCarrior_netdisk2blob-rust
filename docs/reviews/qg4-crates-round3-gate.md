# qg-4 Crate Review — Round-3 Gate (ADR-007 High-Tier Review, APPROVED)

This records the third submission of the qg-4 holistic review of the remaining
high-risk crates (metadata-store, replication-engine, object-crypto,
provider-mobile, provider-telecom, provider-unicom, provider-onedrive) to the
ADR-007 gate, after the round-2 finding was fixed. Reviewer pair: **kimi-k3 +
glm-5.3@max** (gpt-6-astra had no quota; the user's fallback rule).

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg4c-crates-round3-prompt.md` (21,071 bytes,
  SHA-256 `3cf8058b1fa709178d8342371a1d13cac9cd4c56fad3ee806bf4887cd0470c12`).
- **Reviewers:** kimi-k3 (`qg4c-kimi-response.json`, HTTP 200, 171s, 1,373 chars),
  glm-5.3@max (`qg4c-glm53-response.json`, HTTP 200, 219s, 1,617 chars).
- **Verdict:** `approve` (both), `findings: []`.

## Round-3 result

Both reviewers returned **`approve` with an empty findings list**. The qg-4
crate review gate is satisfied for the seven high-risk crates.

### What each reviewer confirmed

- **kimi-k3**: accepted the PRODUCTION/TEST classification from the scanner's
  `#[cfg(test)]` detection; accepted the decode-path asymmetry (u32 via the
  checked `row_u32` helper, u64/u128 via bare same-width `as`) per the prior
  SQLite-binding characterization; found no production dead code in the
  `#[allow(dead_code)]` targets.
- **glm-5.3@max**: confirmed the same-width/checked-conversion framing; judged
  the `.expect()` calls at object-crypto 590-598 (slice-to-array over the fixed
  envelope header) as statically-true invariants; found no criterion-1-4
  violation visible in the supplied evidence.

### Reviewer limitations (not findings)

Both noted the evidence is a line-level scan (guards/clamps/bound-checks on
non-cast lines are invisible) and that the 4 `#[allow(dead_code)]` in
provider-unicom + 1 in provider-onedrive were supplied as counts only (no
location), so the criterion-6 cross-check was incomplete but no masked dead code
was confirmed.

## Resolution chain (rounds 1-2 → round-3 approve)

| Round | Reviewer | Findings | Disposition |
|---|---|---|---|
| 1 | glm | 3 minor | R1 part_number narrowing FIXED; R2 telecom magic number FIXED; R3 informal register = scanner false positive (regex tightened, 0 informal project-wide). |
| 2 | glm | 1 minor | Three more unchecked i64→u32 narrowing sites FIXED via shared `row_u32` helper (cast count 122→119). |
| 3 | kimi + glm | 0 | **APPROVED**. |

## Verification (after round-2 fix)

- `cargo check -p metadata-store`: clean.
- `cargo test -p metadata-store`: **45 passed / 0 failed**.
- `cargo test -p metadata-store -p provider-telecom`: 21 passed / 0 failed.
- Re-run `qg4_locate.py`: metadata-store 119 casts, no phantom/duplicate entries.

## Gate status: **APPROVED**

The qg-4 crate review (metadata-store, replication-engine, object-crypto,
provider-mobile, provider-telecom, provider-unicom, provider-onedrive) passes
the ADR-007 gate. Combined with the F1 gate (qg-7, APPROVED) and the gatewayd
hotspot review (qg-3), the project-wide holistic review under the quality
directive is complete for all covered regions.

## Artifacts

- Prompt: `.cache/reviews/qg4c-crates-round3-prompt.md`
- kimi-k3: `.cache/reviews/qg4c-kimi-response.json`, `.cache/reviews/qg4c-kimi-.final.txt`
- glm-5.3@max: `.cache/reviews/qg4c-glm53-response.json`, `.cache/reviews/qg4c-glm53-.final.txt`
- Gate chain: `docs/reviews/qg4-crates-round1-gate.md` (r1),
  `docs/reviews/qg4-crates-round3-gate.md` (r3, APPROVED)
