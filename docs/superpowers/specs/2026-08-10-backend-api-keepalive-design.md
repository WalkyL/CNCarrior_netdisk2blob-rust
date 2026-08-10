# Backend API Keepalive for Provider Sessions

**Date:** 2026-08-10
**Status:** Approved design
**Scope:** ccbg gatewayd — replace CDP-based session keepalive with backend API probing

## Problem

Provider session keepalive currently works by driving a CDP-controlled browser to
reload the provider web page (`pan.wo.cn` for Unicom, `cloud.189.cn` for Telecom)
every 300 s (`provider_cdp_keepalive_loop`). This:

- Requires a running, enabled Browser/CDP endpoint even when no login/reauth is
  needed — the browser is only truly required for login and re-authentication.
- Only covers Unicom and Telecom; Mobile has no keepalive at all.
- Adds CDP session churn (`location.reload()`) purely to keep a server-side
  session timer fresh.

Meanwhile, every provider backend already performs authenticated API probes that
reset the same server-side session idle timer:

- Unicom: `probe_auth()` — dispatcher JSON-RPC (`CCBG_UNICOM_AUTH_PROBE_OPERATION`,
  default `QUERY_ALL_FILES`).
- Telecom: `user_info()` — `getUserInfoForPortal.action`.
- Mobile: `list_page()` — file listing with the stored token/cookie.

These are already called by `backend.health()` and consumed by the credential
lease probe, which covers all three providers (Unicom, Telecom, Mobile).

## Goals

- Keep provider sessions alive using backend API requests instead of CDP.
- Remove the CDP keepalive loop, its state, its alert, and its config.
- Reuse existing backend probes — no new provider endpoints.
- Keep login and re-authentication browser-based (unchanged).
- Expose keepalive status in Admin via the lease probe state.
- Keep failure behavior identical: `requires_reauth` → primary goes stale →
  writes route to write targets → Admin alert.

## Design

### 1. Remove CDP keepalive

Delete, from `crates/gatewayd/src/main.rs`:

- `provider_cdp_keepalive_loop` and its `tokio::spawn` at startup.
- `maybe_run_provider_cdp_keepalive`.
- `provider_supports_cdp_keepalive`, `provider_cdp_keepalive_url_matches`,
  `provider_cdp_keepalive_target_selector`.
- `ProviderCdpKeepaliveState` / `ProviderCdpKeepaliveStatusRecord` and the
  `provider_cdp_keepalive` field on `AppState` (plus all constructors/tests).
- `{provider}_cdp_keepalive_failing` alert in `build_admin_alerts`.
- Admin status payload fields exposing keepalive summaries
  (`provider_cdp_keepalive` in `ProviderStatusSummaryPayload` and friends).
- Config fields `provider_cdp_keepalive_enabled` /
  `provider_cdp_keepalive_interval_seconds` and their env bindings
  (`CCBG_PROVIDER_CDP_KEEPALIVE_ENABLED`, `CCBG_PROVIDER_CDP_KEEPALIVE_INTERVAL_SECONDS`).

### 2. Credential lease probe becomes the keepalive loop

The existing `provider_credential_lease_probe_loop` already calls
`backend.health()` for Unicom, Telecom, Mobile and persists a
`ProviderCredentialLeaseRecord` with `last_verified_at_unix_ms`,
`last_success_at_unix_ms`, `last_error`, `first_failure_at_unix_ms`,
`requires_reauth`, and `status`. No new probe is needed — this loop IS the
backend keepalive.

Changes:

- **Configurable interval.** Replace the hard-coded `5 * 60_000` ms
  (and the reauth `60_000` ms) advance in `provider_credential_lease_probe` with
  a config value:
  - New env: `CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS`, default
    `300`, floor enforced (e.g. `>= 30`).
  - Reauth case stays short (fixed `60 s`) so a failing primary is re-probed
    quickly regardless of the configured interval.
  - Keep the `next_probe_at_unix_ms_by_provider` batching logic and the
    `credential_lease_probe_wake` notify.
- **Admin status surfaces last_verified.** Ensure the Admin status/lease summary
  already exposed via `ProviderCredentialLeaseSummaryPayload` is reachable; the
  Admin "provider" section shows `last_verified_at` (a.k.a. keepalive freshness)
  and `last_error` for each of Unicom/Telecom/Mobile. If the current Admin view
  already renders lease fields, only the CDP-specific keepalive UI block is
  removed; otherwise add the lease fields to the status payload that the Admin
  asset already renders.

### 3. Failure handling (unchanged)

On probe failure the lease probe already:
- Sets `requires_reauth`, records `first_failure_at` on transition.
- Persists `last_error` and `status=reauth_required`/`unavailable`.
- Advances next probe to `60 s`.
- `is_primary_stale` reads `lease.requires_reauth` → `put`/`delete` auto-route to
  write targets.
- Admin alert machinery (`build_admin_alerts`) flags the provider.

No behavioral change; only the CDP keepalive alert is removed.

### 4. Provider backend surface (no provider-crate changes)

`backend.health()` is the single entry point used by the lease probe. Each
provider's `health()` already issues the authenticated probes listed above, so no
new methods or config are added to `provider-unicom`, `provider-telecom`, or
`provider-mobile`.

## Configuration

| Removed | Replaced by |
|---------|-------------|
| `CCBG_PROVIDER_CDP_KEEPALIVE_ENABLED` | — (no equivalent; keepalive is always on with the lease probe when credentials exist) |
| `CCBG_PROVIDER_CDP_KEEPALIVE_INTERVAL_SECONDS` | `CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS` (default 300, min 30) |

`deploy/lxc/ccbg.env` template updated accordingly. `.49` env updated on deploy.

## Data flow

```
every <interval> (default 300 s; 60 s when requires_reauth)
  credential lease probe
    └─ backend.health()           # existing authenticated API probe per provider
         ├─ Unicom  probe_auth    (dispatcher JSON-RPC)
         ├─ Telecom user_info     (getUserInfoForPortal.action)
         └─ Mobile  list_page     (token/cookie listing)
    ├─ success → last_verified_at/last_success_at updated, requires_reauth cleared
    └─ failure → last_error, requires_reauth=true, first_failure_at set once
                 → is_primary_stale → writes route to write targets
                 → Admin alert
```

## Testing

- `cargo test -p gatewayd` — existing lease-probe tests still pass
  (`credential_lease_probe_` group); CDP-keepalive tests are removed with the
  feature.
- New/extended unit tests:
  - interval config parses and floors at min (`30`).
  - lease probe advances next-probe by configured interval on success, `60 s`
    on `requires_reauth`.
  - Admin status payload exposes `last_verified_at`/`last_error` per provider and
    no longer exposes CDP keepalive fields.
  - No `provider_cdp_keepalive` references remain (compile-level).

## Out of scope

- Browser-based login and re-authentication (unchanged).
- Adding new provider API endpoints for keepalive.
- Auto re-login on consecutive keepalive failures (explicitly deferred).

## Rollout

1. Implement in `crates/gatewayd/src/main.rs`.
2. Update `deploy/lxc/ccbg.env` (remove CDP keepalive vars, add lease probe
   interval).
3. Build Linux ELF (`cargo-zigbuild` cross-compile) and LXC package; deploy to
   `.49`; remove `CCBG_PROVIDER_CDP_KEEPALIVE_*` from `/etc/ccbg/ccbg.env` and
   set `CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS=300`.
4. Verify on `.49`: healthz, Admin status shows lease `last_verified_at`, no
   CDP keepalive alerts.
