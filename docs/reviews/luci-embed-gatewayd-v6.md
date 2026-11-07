# Review Evidence — LuCI Embed of CCBG Admin SPA (方案 A)

This records the ADR-007 holistic review gate result for the change that embeds
the CCBG Admin SPA into an OpenWRT LuCI page via an iframe.

- **Commit reviewed:** `d69510b20548e3d4048844895d640b515faeeb27`
- **Rules fired:** M1 (≥500 changed lines), M3 (frozen contract `CCBG_*`),
  M4 (security boundary: auth / CORS / CSP / cookies), M8 (admin auth chain)
- **Required reviewer set:** GLM-5.3, kimi-k3, astra (all three, per ADR-007 §3
  for M3/M4 security-boundary and frozen-contract changes)
- **Verdict:** GATE PASSES

## Verdicts (final round v6)

| Reviewer | Verdict | Final state reviewed | Key resolution |
|----------|---------|----------------------|----------------|
| GLM-5.3  | APPROVE | v5 (core + shared canonical serializer) | all round-4 findings resolved |
| kimi-k3  | APPROVE | v6 | both round-5 findings resolved |
| astra    | APPROVE | v6 | both round-5 findings resolved |

## Rounds

1. **GLM-5.3 (v1)** — APPROVE with 8 findings (same-origin bypass, cookie_secure
   warning, frame-ancestors validation, case normalization, CORS silent drop,
   CORS dedup, CSRF audit log, boundary tests). All fixed in v2.
2. **kimi-k3 + astra (v2)** — REQUEST_CHANGES. kimi #1-#4 + astra #1-#5 fixed in
   v3.
3. **astra (v3)** — REQUEST_CHANGES (3 findings: non-IPv6 host validation,
   transport-scheme inference, test short-circuit). Fixed in v4.
4. **kimi-k3 (v4)** — APPROVE (all round-3 findings resolved).
   **astra (v4)** — REQUEST_CHANGES (1 finding: WHATWG IPv4/IPv6 serialization
   divergence). Fixed in v5.
5. **kimi-k3 (v5)** — APPROVE. **GLM-5.3 (v5)** — APPROVE (all round-4 findings
   resolved). **astra (v5)** — REQUEST_CHANGES (2 findings: bare-`0x` IPv4
   detection, integration-test coverage). Fixed in v6.
6. **kimi-k3 (v6)** — APPROVE. **astra (v6)** — APPROVE. Both round-5 findings
   resolved, no new findings.

## Evidence artifacts

Raw model responses (prompt hash, verdict JSON) are retained in the scratch
directory `.cache/luci-embed-eval/`:

- `glm53-gatewayd-rust.json` (v1), `glm53-gatewayd-rust-v5.json` (v5)
- `kimi-rereview-v2.json`, `kimi-rereview-v3.json`, `kimi-rereview-v4.json`,
  `kimi-rereview-v6.json`
- `astra-rereview-v2.json`, `astra-rereview-v3.json`, `astra-rereview-v4.json`,
  `astra-rereview-v5.json`, `astra-rereview-v6.json`

## Verification

Full gatewayd test suite (single-threaded) on the committed state:

```
test result: ok. 468 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

All three reviewers returned APPROVE on the final state, so the ADR-007 gate
passes for commit `d69510b`.
