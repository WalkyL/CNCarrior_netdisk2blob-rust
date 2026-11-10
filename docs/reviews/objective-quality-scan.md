# Objective Quality-Gate Scan — CCBG Rust workspace (ADR-007 §2.5-2.7 + minimal type conversion)

This records the deterministic, pattern-based objective scan (qg-1/qg-2) that
guarantees complete coverage of the objective ADR-007 criteria across all
109K+ lines, before the batched holistic reviewer (kimi-k3 + glm-5.3@max) reads
the high-risk regions.

- **Date:** 2026-11-10
- **Scanner:** `.cache/objective_scan.py` (deterministic regex/heuristic; cast
  counting is **literal-aware** — string literals and comments are stripped
  before matching, so the words "as u64" inside a `warn!` message are not
  counted as a conversion; see the C4 note below).
- **Input:** all `crates/**/*.rs` (17 crates, 116,837 lines measured).
- **Criteria:** (A) style/taste & naming; (B) type-conversion minimization;
  (C) text register; (D) dead/speculative code.

## Global totals

| Criterion | Count | Notes |
|---|---|---|
| Any `as` cast | 320 | type-conversion directive |
| Same-width `as` cast | 316 | the true "unnecessary cast" signal |
| `.parse()` | 60 | conversion round-trips |
| Long snake_case identifiers (>4 words) | 4006 | naming-density signal |
| Long CamelCase identifiers | 8763 | mostly legitimate type/route names |
| Nested ternary / run-on chains | 19 | style signal |
| `.unwrap()` (non-test) | 135 | panic-path signal |
| `#[allow(dead_code)]` | 33 | dead-code signal |
| TODO/FIXME/HACK | 0 | clean |
| Commented-out code blocks | 1 | clean |
| Informal register tokens in comments | 0 | clean (after scanner tightening; the earlier 7 were false positives matching Rust code identifiers `Ok`/`.ok()` and the technical term `bit` in bit-flip tests) |

## Ranked hotspots (per 1000 lines)

### Type-conversion density (the directive's primary target)

| Crate | casts | cast/1K |
|---|---|---|
| **metadata-store** | 123 | **23.6** |
| **object-crypto** | 41 | **23.4** |
| provider-unicom | 21 | 4.0 |
| auth-broker | 3 | 2.5 |
| provider-mobile | 15 | 2.3 |
| blob-core | 12 | 1.9 |
| gatewayd (F1 region) | 86 | 1.3 |

### Naming density (style/taste)

| Crate | snake/1K |
|---|---|
| **admin-api** | 246.0 |
| **metadata-store** | 219.0 |
| auth-broker | 186.1 |
| blob-core | 123.5 |
| **gatewayd** | 110.8 |
| provider-telecom | 85.5 |

### `unwrap()` (panic-path) hotspots

| Crate | unwrap |
|---|---|
| **gatewayd** | 87 |
| **replication-engine** | 21 |
| auth-broker | 10 |
| browser-cdp | 10 |
| provider-onedrive | 4 |

## Interpretation

- **The F1 prototype region (gatewayd) is NOT a cast hotspot** (1.3/K vs 23.6/K
  in metadata-store) — the F1 change does not add type-conversion pressure.
- **metadata-store and object-crypto are the type-conversion directive's
  primary targets** (metadata-store 122/1K-cast total, object-crypto 41, both
  dominated by `same_width_cast`; the production-only figures are lower — see
  the qg-4 crate review).
- **Register is clean** (0 informal tokens after scanner tightening; 0
  TODO/FIXME/HACK; 1 commented block). The earlier 7 "informal" tokens were
  scanner false positives: `\bok\b` matched the Rust `Ok` enum variant and the
  `.ok()` method, and `\bbit\b` matched the precise technical term "bit" in
  bit-flip corruption tests — none is informal English prose.
- The `snake_per_k` and `long_camel` raw counts over-flag legitimate names
  (route constants, type names); the holistic reviewer must distinguish genuine
  verbosity from idiomatic Rust naming.
- **C4 (scan methodology, from qg-3):** the original count included matches
  inside string literals (e.g. the two `warn!` messages "failed to parse env var
  as u64/usize"). The scanner is now literal-aware and line-preserving (it keeps
  newlines inside string literals, block comments, and line-continuation
  escapes, so per-line attribution stays aligned). gatewayd's real cast count is
  **85** (was 90), and the production (non-test) count is **66**.

## Artifacts

- Scanner: `.cache/objective_scan.py`
- Raw report: `.cache/objective_report.json`
- Summarizer: `.cache/summarize_scan.py`
