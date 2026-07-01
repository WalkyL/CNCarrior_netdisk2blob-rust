# SMB Sidecar Host Test Matrix

## Objective

Treat `smb-sidecar-host` as a critical-path component and keep its validation split into two layers:

- fast unit coverage for config parsing, state transitions, hashing, naming, and contract shape
- Linux/LXC integration coverage for real `rclone` / `smbd` / `systemd-run` / FUSE behavior

## Unit Coverage

Run on every local change and CI build:

```bash
cargo test -p smb-sidecar-host
```

Required unit coverage areas:

- env parsing with comments, blank lines, and control-plane override
- gateway endpoint normalization for IPv4 wildcard, IPv6 unspecified, and explicit hosts
- transient unit naming stability and sanitization
- sidecar model build success for valid config
- sidecar model rejection for malformed users, malformed shares, unknown applications, and missing credentials
- runtime spec hash changes when effective runtime config changes
- generated `rclone.conf` / `smb.conf` contract shape
- runtime payload fallback shape when `status.json` is absent
- runtime payload counters and `listener_ready` behavior when no live `smbd` exists
- deduplicated share error aggregation
- `disabled` and `stopped` status/metadata write paths

## Linux Integration

Run on a real Linux host or CI runner with root privileges and required packages installed:

Unified entrypoint:

```bash
scripts/test-smb-sidecar-host-all.sh
```

It always runs unit coverage first, then auto-runs or skips each Linux integration stage based on
the current host's capabilities.

For dedicated test hosts, use strict mode to fail if any stage is skipped:

```bash
SMB_SIDECAR_TEST_REQUIRE_FULL=1 scripts/test-smb-sidecar-host-all.sh
```

The current GitHub Actions `release-assets-build-runner` workflow also runs
`scripts/test-smb-sidecar-host-all.sh` inside the Linux build-runner before packaging the LXC asset.

For dedicated strict-mode runners, use:

```bash
scripts/check-smb-sidecar-test-host.sh strict
SMB_SIDECAR_TEST_REQUIRE_FULL=1 scripts/test-smb-sidecar-host-all.sh
```

GitHub Actions strict-mode workflow:

- `.github/workflows/smb-sidecar-host-strict.yml`
- expected runner labels: `self-hosted`, `linux`, `x64`, `smb-sidecar-test`

Baseline script:

```bash
scripts/test-smb-sidecar-host-integration.sh
```

Runtime script for Linux root environments with Samba/rclone installed:

```bash
sudo scripts/test-smb-sidecar-host-runtime.sh
```

Systemd + real-running script for dedicated Linux test hosts with `/dev/fuse` and transient-unit support:

```bash
sudo scripts/test-smb-sidecar-host-systemd-running.sh
```

Recovery script for stale metadata / re-converge scenarios:

```bash
sudo scripts/test-smb-sidecar-host-recovery.sh
```

- `smb-sidecar-host sync` with `systemd-run` available
- `smb-sidecar-host sync` with `systemd-run` unavailable, verifying direct child-process fallback
- `smb-sidecar-host stop` after both runtime modes above
- idempotent second `sync` when `desired_hash` is unchanged and runtime is already healthy
- recovery `sync` when metadata says healthy but one process is gone
- `sync` with zero shares but valid SMB user set, verifying listener still comes up
- `sync` with `/dev/fuse` missing, verifying `state=degraded`, share error text, and `mounted_share_count=0`
- `sync` with `/dev/fuse` present, verifying `state=running` and mounted share count convergence
- `sync` with missing optional VFS module, verifying startup/log failure is observable
- `sync` after stale mountpoints and stale transient units are left behind

## LXC / PVE Integration

Run on a guest that matches the supported deployment profile:

- `--enable-smb-sidecar` install path
- `--s3-only` install path with sidecar stop/disable cleanup
- reconcile after enabling `/dev/fuse` on the host
- reconcile after guest reboot
- verify transient units remain outside `ccbg-smb-sidecar-sync.service` cgroup

## Upgrade Regression

Run for any release that changes sidecar runtime behavior:

- upgrade from a package that predates the Rust helper
- upgrade with existing `status.json` / `managed-runtime.json`
- upgrade while a previous transient `smbd` / `rclone` unit is still active
- verify packaged files no longer depend on `deploy/lxc/ccbg-smb-sidecar.py`

## Release Gate

Do not consider `smb-sidecar-host` changes complete unless:

1. unit tests pass
2. targeted Linux integration tests pass
3. targeted LXC/PVE validation passes when runtime behavior changed
