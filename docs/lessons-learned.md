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

## Deployment

- When deploying from Windows to a Linux host, build the Linux ELF inside the local Podman
  build-runner image instead of relying on `cargo build --release` on the host.
- Deploy `gatewayd` and `assets/admin/index.html` together, then verify both remote hashes and
  service health before considering the deploy complete.

## Related Docs

- [Object Delete Convergence spec](SPEC.md#object-delete-convergence)
- [`.49` object delete convergence deploy record](ops-014-49-object-delete-convergence-deploy.md)
- [PVE/LXC deployment guide](pve-lxc-deployment.md)
