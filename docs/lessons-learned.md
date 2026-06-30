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
- SMB sidecar host-process mode needs its optional Samba VFS modules to match the generated
  `smb.conf`. If `fruit` is enabled in config but `samba-vfs-modules` is missing, the sidecar can
  still look `running` while client login fails at `IPC$`.
- `rclone mount` permission flags alone are not enough for SMB group write access. Without an
  explicit `--umask 007`, `--dir-perms 0770` / `--file-perms 0660` are masked down to
  `0750` / `0640`, which lets SMB users read but blocks create/delete.
- For SMB sidecar host-process mode, do not use `ccbg-smb-sidecar-sync.service` as the liveness
  signal. It is intentionally short-lived; the real checks are `python3 /opt/ccbg/scripts/ccbg-smb-sidecar.py status`
  plus the transient `ccbg-smb-sidecar-*.service` units.
- The documented release host must match the actual active build machine. When the build host moves
  from one LAN node to another, update the release SOP, checklist, host-boundary ADR, and workflow
  entrypoint notes in the same change; stale host labels will send operators down the wrong path.
- A containerized build on the same Windows host cannot use `127.0.0.1:10808` as its proxy; the
  build-runner container must use the host LAN IP (`192.168.1.52:10808` in the current setup).
- Keep release workflow defaults aligned with the image that actually exists. If the current
  build-runner image cannot link Darwin targets, macOS jobs should stay disabled by default rather
  than failing every manual release run.
- GitHub Actions artifact downloads are not guaranteed to keep one fixed directory shape. Release
  helper scripts must tolerate both `dest/<files>` and `dest/<artifact>/<files>` layouts.
- When Bash/WSL launches Windows CLIs such as `gh.exe` or `python.exe`, convert absolute POSIX
  paths before handing them over. Otherwise `/mnt/d/...` can be misread as `D:\mnt\d\...` and
  release helper scripts fail for path reasons instead of real build reasons.
- On mixed Windows environments, full quality gates and final asset merge/upload do not need to use
  the same shell. Use the shell that gives stable test behavior for the gate, and the shell that
  gives correct CLI path semantics for the artifact merge.
- When launching long-running sidecar processes under `systemd-run`, prefer `StandardOutput=` /
  `StandardError=` properties over shell redirection. It avoids transient-unit escape warnings and
  keeps runtime logs attached to the unit cleanly.
- Sidecar runtime generation should tolerate missing optional VFS plugins and degrade to the
  modules actually present on the host instead of producing a false-healthy SMB listener that
  rejects all sessions.

## S3 Compatibility Gaps

- Directory traversal for mounted clients must be driven by local placement metadata, and the
  emitted listing timestamps must stay S3-compatible; provider-native timestamp strings can break
  `rclone mount` even when exact-key reads already work.
- Multipart completion must accept entity-encoded ETags, return response headers promptly, and
  persist the final S3-compatible multipart ETag; otherwise clients can time out or reject the
  upload as corrupted.
- Public `HEAD` / `LIST` responses must prefer the gateway's persisted logical `ETag` over a
  backend-native development `ETag`; otherwise multipart-complete objects can be written
  correctly but later fail client-side verification.
- Stub-provider smoke and real-client interoperability are different layers. `rclone` needs its
  validated compatibility knobs (`use_unsigned_payload`, `use_multipart_etag`,
  `disable_checksum`) in smoke and operator docs, or the harness will report false negatives that
  do not match production S3 application flows.
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
