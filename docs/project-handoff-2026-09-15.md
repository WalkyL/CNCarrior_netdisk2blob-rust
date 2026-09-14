# CCBG Project Handoff

Date: 2026-09-15

## Repository

- Repository: `https://github.com/WalkyL/CNCarrior_netdisk2blob-rust`
- Branch: `main`
- Handoff commit before this document: `1d83991`
- The batch object-management implementation is committed locally on `main`.
- No provider crate or `BlobBackend` protocol was changed.

## Delivered Feature

Admin Web now supports safe multi-select batch object management from the
current object-browser result:

- batch delete and batch move only;
- move target bucket plus prefix, preserving each source basename;
- read-only preview with selection, identity, topology, target, and capability
  checks;
- plan-owned five-minute expiry and bounded selection/ledger state;
- persistent idempotency and `in_progress` / `rejected` / `interrupted`
  recovery states;
- per-item late revalidation before the shared single-object action core;
- existing metadata, WAL, replication, and rollback semantics preserved;
- state-driven UI selection, stale-response invalidation, busy controls, one
  delete confirmation, and per-item read/home provider display.

Placement cleanup remains separate: it removes gateway metadata only and does
not delete the cloud object. Batch delete explicitly removes the cloud object
and gateway metadata.

## Verification

Fresh local verification on the final tree:

- `cargo test --workspace -- --test-threads=1`: all workspace tests passed;
  gatewayd 402/402, with all other crate tests and doc-tests passing.
- `cargo test -p admin-api`: 8/8 passed.
- `cargo test -p gatewayd object_batch -- --test-threads=1`: 35/35 passed.
- `node scripts/test-admin-object-batch-state.mjs`: passed.
- Admin page contract, action-core, WAL warning, `cargo check -p gatewayd`,
  and `git diff --check`: passed.
- gatewayd reports existing warning-level unused/dead-code diagnostics. The
  broad formatter baseline remains unsuitable as a clean gate.

## Review Closure

Task reviews and the final whole-branch review are complete. The final scoped
review approved all A-G findings:

- execution-time source/destination revalidation;
- independent plan lifetime;
- read source versus authoritative home provider in the UI;
- lock-free read-only preview;
- execution busy state and idempotent retry retention;
- useful row-level Manage navigation;
- delete confirmation count, first five objects, and remainder count.

The review also corrected API payload examples, reason-code definitions,
metadata-only cleanup wording, legacy OneDrive policy migration, and recovery
durability behavior.

## `.43` Test Deployment

Target: `walky@192.168.1.43` (`testlinux01`), user service
`ccbg-43.service`.

- Deployed binary SHA-256:
  `769e17917b04664e89923d9bd15f7d1269bfa53bce075f280fec1c92d565600c`
- Deployed Admin HTML SHA-256:
  `360d4d8eeb2385f28d1b1e0ce7db55a492c83877c31c34dcc4afe987de118b5f`
- Service: active, PID `260019` at the last verification.
- `http://192.168.1.43:61080/healthz`: HTTP 200.
- Unauthenticated Admin root: HTTP 302 to `/login`.
- Unauthenticated Admin API: HTTP 401.
- Authenticated `/readyz`: HTTP 503 because `CCBG_UNICOM_TOKEN` is missing.
- Old binary and Admin HTML backup:
  `/home/walky/apps/ccbg/backups/deploy-20260914T193638Z`.
- Deployment staging files were removed after verification.

No real cloud-object delete or move was executed on `.43`; provider acceptance
requires configuring the Unicom token through the approved credential path.

## Remaining Work

1. Configure and verify the `.43` Unicom credential without putting secrets in
   shell history, logs, or this document.
2. Run authenticated `/readyz`, Admin login, object listing, preview, and a
   controlled test-object move/delete on `.43`.
3. Keep the rollback backup until that acceptance is complete; then prune old
   deployment backups according to the release policy.
4. Keep batch tests serial while the process-global WAL fault injector remains
   unchanged.

## Operational Boundaries

- External provider/S3 writers are outside the gateway mutation lock; batch
  operations are not distributed transactions.
- `WAL` finalization failures are warnings after a successful remote action;
  they do not imply remote rollback.
- Do not use the old SDD review directory as a runtime input. The formal plan,
  design specification, this handoff, and `docs/lessons-learned.md` are the
  durable project records.
- Never copy credentials, cookies, provider responses, or API keys into issue
  comments, commit messages, or handoff documents.
