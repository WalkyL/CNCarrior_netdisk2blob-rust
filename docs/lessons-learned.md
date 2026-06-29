# CCBG Lessons Learned

## Object Delete Convergence

- Delete paths must be idempotent. If the backend object is already gone, `NotFound` should not
  block metadata cleanup.
- Residual object cleanup must remove the full metadata set together: placement, logical object,
  and protection plan.
- Cleanup UI should describe the action as removing residual object metadata, not only placement,
  so operators do not assume a narrower effect than the code actually has.
- Bulk cleanup should classify rows by live backend state and metadata state, not by prefix listing
  alone.
- Missing placement metadata must be treated as a repair condition, not as a signal to fall back
  to the current primary provider.
- Cleanup tooling needs an orphan view for `logical_objects` and `object_protection_plans`; a
  placement-only scan can hide real residue.
- Replication-delete persistence and WAL replay are recovery mechanisms, not substitutes for an
  atomic delete guarantee.
- Multipart upload sessions need their own expiry and cleanup cadence, or aborted uploads will
  linger as hidden state even after object deletion looks healthy.
- Backup import must validate cross-table consistency before replacing state, otherwise a restore
  can reintroduce orphan metadata.

## Deployment

- When deploying from Windows to a Linux host, build the Linux ELF inside the local Podman
  build-runner image instead of relying on `cargo build --release` on the host.
- Deploy `gatewayd` and `assets/admin/index.html` together, then verify both remote hashes and
  service health before considering the deploy complete.

## S3 Compatibility Gaps

- Directory traversal for mounted clients must be driven by local placement metadata, and the
  emitted listing timestamps must stay S3-compatible; provider-native timestamp strings can break
  `rclone mount` even when exact-key reads already work.
- Multipart completion must accept entity-encoded ETags, return response headers promptly, and
  persist the final S3-compatible multipart ETag; otherwise clients can time out or reject the
  upload as corrupted.
- Large-object smoke tests should stay explicit about the currently validated size boundary and
  provider capability flags; passing a stress test does not mean the provider has unlocked a larger
  permanent upload policy.
- Cleanup after soak or recovery work must delete both cloud objects and local metadata rows; a
  successful backend delete is not enough if placement, logical object, or protection-plan residue
  remains.

## Provider Keepalive

- Keepalive verification must run in a dedicated low-frequency loop, not as a side effect of
  webhook delivery.
- The smallest stable keepalive path is each provider's direct `health()` probe; CDP/browser
  stays capture-only and must not become a long-running dependency.
- Separate polling knobs for webhook alerts and provider lease probes so one can be disabled or
  slowed without affecting the other.
- OneDrive stays parked and does not participate in the active keepalive path.

## Pluggable Entry Points

- Capability entry scripts should be idempotent so operators can rerun them after host reboots
  without creating duplicate browser or bridge processes.
- Prefer checking the smallest necessary local state before launching anything; do not turn entry
  scripts into permanent background daemons.
- Keep browser/CDP handling capture-only and outside the core runtime so low-memory deployments can
  omit it entirely.

## Related Docs

- [Object Delete Convergence spec](SPEC.md#object-delete-convergence)
- [`.49` object delete convergence deploy record](ops-014-49-object-delete-convergence-deploy.md)
- [PVE/LXC deployment guide](pve-lxc-deployment.md)
