# Review Evidence — Admin SPA CSP `img-src` fix (restore data: images)

This records the ADR-007 holistic review gate result for the change that adds an
explicit `img-src 'self' data:` directive to the CCBG Admin SPA's
Content-Security-Policy header, restoring the inlined SVG brand icon and the
browser-flow PNG screenshots (both `data:` URLs) that were previously blocked by
the `default-src 'self'` fallback.

- **Commit reviewed:** `ae1aa63` (against parent `618f842`)
- **Change summary:** one production line (`img-src 'self' data:` added to the
  CSP string in `async fn admin_index`), plus a `parse_csp_directives` test
  helper and two new unit tests.
- **Rules fired:** M4 (security marker `csp` added). Per ADR-007 §3, a
  security-boundary change requires the full high-tier reviewer set.
- **Required reviewer set:** GLM-5.3, kimi-k3, astra (all three).
- **Verdict:** GATE PASSES (3/3 APPROVE, zero findings).

## Verdicts

| Reviewer | Effort | Verdict | Findings |
|----------|--------|---------|----------|
| GLM-5.3  | max    | APPROVE | none |
| kimi-k3  | none   | APPROVE | none |
| astra    | —      | APPROVE | none |

## Grounded rationale (from the change)

The Admin SPA renders a brand icon and two browser-flow screenshots as inlined
`data:` URLs. The CSP had no `img-src`, so it inherited `default-src 'self'`,
which blocks `data:` URLs. The fix adds an explicit `img-src 'self' data:`.
`script-src 'self' 'nonce-{nonce}'` is unchanged, so no script execution is
enabled; `data:` is limited to the `img-src` source list, and an SVG loaded as
an `<img>` cannot execute script. No other directive (`default-src`,
`script-src`, `style-src`, `frame-ancestors`, `base-uri`, `form-action`) is
disturbed.

## Verification (measured)

- `cargo build -p gatewayd`: OK (only pre-existing dead-code warnings, no errors).
- `cargo test -p gatewayd`: **470 passed; 0 failed** (806.19s). Includes the two
  new CSP tests and the two previously-flaky WAL tests, which passed this run.

## Evidence artifacts

Raw model responses (prompt hash `5979444d78ed75fb…`, identical prompt to all
three reviewers) are retained in the scratch directory `.cache/`:

- `.cache/csp-icon-gate-prompt.md` — shared review prompt (diff + context + criteria)
- `.cache/csp-icon-gate-glm53.json` — GLM-5.3 (effort max), APPROVE
- `.cache/csp-icon-gate-kimi.json` — kimi-k3, APPROVE
- `.cache/csp-icon-gate-astra.json` — astra, APPROVE

## Reviewers' stated limitations (none blocking)

- GLM-5.3: the 1.27 MiB SPA template and `test_state()`/`Config` were not
  directly inspected; build/test results taken as reported; exact `img-src`
  source-list ordering is a strictness choice, not a defect.
- kimi-k3: surrounding `main.rs` not directly inspected; security assessment
  reasoned from the CSP spec and the unchanged nonce-only `script-src`, not
  dynamic analysis.
- astra: browser behavior was not independently tested.

All three returned APPROVE with an empty findings list, so the ADR-007 gate
passes for this change.
