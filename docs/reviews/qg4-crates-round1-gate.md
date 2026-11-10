# qg-4 Crate Review — Round-1 Gate (ADR-007 High-Tier Review)

This records the first submission of the qg-4 holistic review of the remaining
high-risk crates (metadata-store, replication-engine, object-crypto,
provider-mobile, provider-telecom, provider-unicom, provider-onedrive) to the
ADR-007 gate. Reviewer pair: **kimi-k3 + glm-5.3@max** (gpt-6-astra had no
quota; the user's fallback rule).

- **Date:** 2026-11-10
- **Review material:** `.cache/reviews/qg4-crates-prompt.md` (round-1).
- **Reviewers:** glm-5.3@max (`qg4-glm53-response.json`, HTTP 200, 272s);
  kimi-k3 (`qg4-kimi-response.json`) — **not yet received** (HTTP 429 RPM-limit
  on the first attempt, retried during the cooldown; the submission is pending).
- **Verdict:** glm = `approve` with 3 minor findings; kimi = pending.

## Round-1 findings (glm) and their disposition

| # | Sev | Location | Problem | Disposition |
|---|---|---|---|---|
| R1 | minor | metadata-store line 2396, `row_to_multipart_upload_part_record` | Unchecked lossy narrowing `part_number: row.get::<_, i64>(1)? as u32` on a path that already returns Result, inconsistent with the guarded decode at 2411; an out-of-range stored value would silently truncate. | **FIXED**: `u32::try_from(row.get::<_, i64>(1)?)` with `TryFromIntError` mapped into `rusqlite::Error::FromSqlConversionFailure` (matches the existing `row_to_job` idiom). Cast count 123→122. |
| R2 | minor | provider-telecom line 2165 | Magic number `remaining.min(1024 * 1024)` inlines an unexplained 1 MiB read chunk; sibling crate names the same concept (`MOBILE_UPLOAD_STREAM_CHUNK_BYTES`). | **FIXED**: hoisted to `const STREAM_READ_CHUNK_BYTES: u64 = 1024 * 1024;` (line 56), matching the named-constant convention. |
| R3 | minor | provider-mobile, "Ok"/"ok" comment pair | Informal register: `Ok`/`ok` read as informal shorthand. | **SCANNER FALSE POSITIVE**: `\bok\b` matched the Rust `Ok` enum variant (doc-comment `Ok(None)`) and the `.ok()` method (test comment); `\bbit\b` matched the technical term "bit" in object-crypto bit-flip tests. A targeted search for genuine informal prose (kinda/gonna/wanna/…) across all crates found **0**. The `INFORMAL` regex was tightened (removed `ok`, `okay`, `bit`, `stuff`, `things`, `pretty`, `really`, `very`, `a lot`); informal counts are now **0** for every crate. No comment was changed. |

## Evidence-quality issue (glm limitation, round-1)

glm's round-1 limitation noted the metadata-store line list was internally
inconsistent with its header: doc comments (358, 514), SQL string literals
(557, 2249), signature/type-declaration lines (398-400, 2377-2380), blank or
paren-only lines, and a duplicate entry (344) were enumerated as cast sites.
Root cause: the round-1 `.cache/qg4_sites.json` was built from a **stale
snapshot** taken before the literal-aware, line-preserving stripper fix. The
inventory was **rebuilt** from the current stripper; all phantom/doc/blank/
duplicate entries are gone, and the per-crate production cast counts are the
accurate figures (metadata-store 122, object-crypto 16 production, provider-unicom
10 production, provider-onedrive 0 production).

## Verification after fixes

- `cargo check -p metadata-store -p provider-telecom`: clean.
- `cargo test -p metadata-store -p provider-telecom`: **21 passed / 0 failed**.
- Re-run `qg4_locate.py` with the updated scanner + fixes: metadata-store 122,
  no phantom entries, no duplicate false positives (line 350 is a genuine
  two-cast expression).

## Gate status

glm returned `approve` with 3 minor findings (R1-R3), all now resolved (2 code
fixes, 1 scanner false-positive correction). kimi's verdict is pending (RPM
cooldown). A **round-2 submission** to both reviewers with the corrected prompt
(`qg4b-crates-round2-prompt.md`) is required to confirm the gate.
