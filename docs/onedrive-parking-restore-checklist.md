<!-- SPDX-License-Identifier: LicenseRef-CCBG-Commercial -->
<!-- Copyright (c) 2026 walky -->

# OneDrive Parking Restore Checklist

OneDrive is a parking provider. It must stay disabled in default examples and packages until there is a real user need, a named operator, and a rollback owner. This checklist is the gate for restoring it into any user-facing flow.

## 1. Product Gate

- Record the user scenario, bucket scope, object size range, expected daily write volume, and why existing operator providers are insufficient.
- Decide whether OneDrive is only a secondary async target, a read-side fallback target, or a memory-only target.
- Confirm that the public docs still describe OneDrive as optional, not a default provider.
- Record the decision in the release note or an ADR before changing defaults.

## 2. Configuration Gate

- Keep package defaults at `CCBG_ONEDRIVE_ENABLED=false` and `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`.
- Enable only in the target test environment with explicit env overrides.
- Add `onedrive` to `CCBG_SYNC_TARGETS` only after provider probe and write smoke pass.
- Add `onedrive` to `CCBG_FALLBACK_READ_ORDER` only after read-after-sync smoke proves completed replicas are readable.
- Keep `CCBG_PRIMARY_PROVIDER` on `unicom`, `telecom`, `mobile`, or `stub`; do not make OneDrive the primary provider without a separate design review.

## 3. Authentication Gate

- Validate OAuth redirect URL or device-code mode for the target deployment.
- Store tokens in the configured session file or token file with owner-only permissions.
- Confirm Admin and API responses do not echo access tokens, refresh tokens, cookies, authorization headers, client secrets, or local token file paths.
- Rotate the test credential once and verify the runtime reload path.

## 4. Provider Probe Gate

- Run capability probe for list, upload, download, delete, quota, and file-size limits.
- Update `config/provider-probes/onedrive.json` only with verified limits and the probe timestamp.
- Verify timeout, retry, and throttling behavior under at least one forced Graph error.
- Record object-size limits before enabling memory or backup prefixes.

## 5. Replication And Fallback Gate

- Start with a narrow bucket or prefix allowlist.
- Verify async replication enqueue, completion, retry, and dead-letter behavior.
- Verify `HeadObject` and `GetObject` can read from OneDrive only after the replica is marked complete.
- Confirm fallback response headers identify the actual provider.
- Confirm disabling OneDrive leaves primary reads and writes unaffected.

## 6. Observability Gate

- Confirm Admin status, operations overview, metrics, and logs show OneDrive as disabled, degraded, or healthy without exposing secrets.
- Add or verify alerts for replication backlog, failed jobs, auth expiry, quota pressure, and provider health.
- Run webhook notification smoke for at least one induced OneDrive failure.

## 7. Regression Commands

Run these before any release that exposes OneDrive again:

```bash
python3 scripts/check-onedrive-parking.py
python3 scripts/check-onedrive-restore-checklist.py
cargo test -p gatewayd onedrive -- --nocapture
cargo test -p provider-onedrive -- --nocapture
python3 scripts/license-check.py --skip-cargo-metadata
git diff --check
```

If provider-specific tests are not available in the current checkout, record that as a release blocker rather than treating the checklist as complete.

## 8. Rollback

- Remove `onedrive` from `CCBG_SYNC_TARGETS` and `CCBG_FALLBACK_READ_ORDER`.
- Set `CCBG_ONEDRIVE_ENABLED=false` and `CCBG_ONEDRIVE_REPLICATION_ENABLED=false`.
- Restart the gateway or reload runtime provider configuration.
- Verify new writes do not enqueue OneDrive replication jobs.
- Keep completed OneDrive replicas as read-only evidence until the operator approves cleanup.
- Record rollback time, reason, affected buckets, and whether any objects require reconciliation.

## Acceptance

The restore is acceptable only when all gates are checked, rollback has been rehearsed in a test environment, and the release notes state that OneDrive remains optional and off by default.
