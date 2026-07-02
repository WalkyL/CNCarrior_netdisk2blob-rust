# OPS-016: `.49` Mobile Limit Probe Refresh And Orphan Metadata Cleanup

## Status

Accepted

## Date

2026-07-02

## Objective

1. Refresh the China Mobile large-file evidence on `.49` without disrupting the current Unicom
   dogfood primary service.
2. Close the first metadata repair gap by making orphan `logical_objects` /
   `object_protection_plans` visible and cleanup-capable through the existing object reconcile
   surface.

## Runtime Boundary

`.49` remains dogfooded on the main Unicom-backed service:

- primary provider: `unicom`
- write targets: `mobile`, `unicom`, `telecom`
- main S3 API: `0.0.0.0:61080`
- main Admin UI: `0.0.0.0:61081`

The Mobile limit probe below used the existing control-plane API and a secondary provider backend
instance. It did not replace the main service topology.

## Build / Deploy Provenance

- release workflow: `release-assets-build-runner.yml`
- successful build-runner artifact run: `28543382069`
- run URL:
  - `https://github.com/WalkyL/CNCarrior_netdisk2blob-rust/actions/runs/28543382069`
- artifact: `ccbg-lxc-package`
- artifact id: `8021035353`
- artifact package SHA-256:
  - `89e3aebcf6b839a02f8212e54375232e64c96e2c05bb4e407a4139d23c25f3d3`
- deployed commit:
  - `17dfb34433bd26992520f30ae3815fa5230aa9a7`

`.49` deploy result from this artifact:

- `/opt/ccbg/bin/smb-sidecar-host`
  - `cbcc48e3c5630b920e7e827d3a58be04cb9b535d677da0db4f9871e5283e4b46`
- SMB sidecar status after deploy:
  - `state=running`
  - `listener_ready=true`
  - `enabled_share_count=1`
  - `mounted_share_count=1`

## Mobile Limit Probe Refresh

Control-plane API key source on `.49`:

```bash
grep '^CCBG_CONTROL_API_KEY=' /etc/ccbg/ccbg.env
```

Probe path used:

```text
POST /api/providers/mobile/limit-probe
```

The probe reused saved `.49` Mobile credentials and left the main Unicom service healthy before and
after the run.

### 8 GiB Probe

Request shape:

```json
{
  "bucket": "root",
  "key_prefix": "ccbg-limit-probes",
  "sizes": ["8GiB"],
  "read_back": false,
  "delete_after": true,
  "stop_after_first_success": false
}
```

Observed result:

```text
upload_ok=false
upload_error=upstream error: China Mobile file/create rejected the request: code=04010319 ...
```

Conclusion:

- `.49` still has fresh live evidence that China Mobile upstream rejects at least `8 GiB`
- the current project/release/Admin wording must continue to treat large-file support conservatively

### 16 GiB Probe

Request shape:

```json
{
  "bucket": "root",
  "key_prefix": "ccbg-limit-probes",
  "sizes": ["16GiB"],
  "read_back": false,
  "delete_after": true,
  "stop_after_first_success": false
}
```

Observed result:

```text
upload_ok=false
upload_error=body stream error: No space left on device (os error 28)
```

Host filesystem state during the run:

```text
/dev/mapper/pve-vm--104--disk--0   26G   11G   14G  45% /
```

Conclusion:

- the `16 GiB` probe on the current `.49` deployment shape is now blocked by local spool/disk
  capacity before it can prove an upstream Mobile allowance
- this is not evidence that China Mobile large-file support is now verified
- the newest safe statement is:
  - `8 GiB` still fails with upstream `04010319`
  - `16 GiB` is additionally blocked by the current `.49` local spool/disk boundary

## Metadata Repair / Cleanup Follow-up

Code state updated in this round:

- `object_reconcile` preview now surfaces orphan metadata rows that have:
  - no `object_placements` record
  - but still have `logical_objects` and/or `object_protection_plans`
- `object_reconcile` execute can now cleanup those orphan rows by reusing the existing
  `delete_object_metadata(...)` transaction

Why this matters:

- placement-only scans could hide real residue
- operators now have a first-class way to see and remove orphan metadata instead of relying on a
  separate ad-hoc script

Validation run locally:

```bash
cargo test -p gatewayd object_reconcile_
```

Observed targeted result:

```text
8 passed; 0 failed
```

New coverage included:

- orphan metadata rows appear in reconcile preview
- orphan metadata cleanup removes residual logical/protection metadata when no placement exists

## Release Messaging Impact

Do not describe China Mobile large-file support as “verified” or “fixed” after this round.

Updated safe wording:

- China Mobile still has no verified >4 GiB upload success evidence on `.49`
- `.49` has fresh evidence that `8 GiB` is rejected upstream with `04010319`
- `16 GiB` is currently blocked by the `.49` local spool/disk boundary before it can establish a
  stronger upstream claim
