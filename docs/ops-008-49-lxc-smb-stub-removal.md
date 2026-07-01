# OPS-008: .49 LXC SMB Sidecar and Stub Removal Acceptance

## Status

Accepted

## Date

2026-06-03

## Target

- Host: `root@192.168.1.49`
- Hostname: `ccbg01`
- Package: `target/lxc-package/ccbg-lxc-package.tar.gz`
- Package SHA256: `c4aeee3f85e553b4982a5fbaecbdb2852ae0015b3b23872d5c31922582180ed0`

## Decision

User-facing install paths must not start from `stub`.

Default host packages now use a real carrier provider default:

```dotenv
CCBG_PRIMARY_PROVIDER=unicom
```

The package still does not include real provider credentials. Before credentials are saved, the
provider should show unavailable instead of falling back to local stub. `StubBackend` remains only as
an internal CI/smoke/test fixture.

## LXC Package Guard

`scripts/build-lxc-package.sh` now rejects non-Linux binaries when building the LXC package.

Reason: building on a Windows workstation can produce `target/release/gatewayd.exe`; that must never
be packaged as `bin/gatewayd` for Linux LXC guests.

Allowed LXC inputs:

```bash
scripts/build-lxc-package.sh --target x86_64-unknown-linux-gnu
scripts/build-lxc-package.sh --binary target/gatewayd-linux-x86_64
```

The package build checks the selected binary with `file` and requires an ELF binary.

## .49 Deployment Result

Installed with:

```bash
tar --no-same-owner -xzf /tmp/ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
./scripts/install.sh --enable-smb-sidecar
systemctl restart ccbg.service
systemctl start ccbg-smb-sidecar-sync.service
```

The installer kept existing `/etc/ccbg/ccbg.env` and wrote the new sample as
`/etc/ccbg/ccbg.env.package-20260602145709`.

Validated runtime state:

```text
env=CCBG_PRIMARY_PROVIDER=unicom
package_sample=CCBG_PRIMARY_PROVIDER=unicom
control_plane_primary=unicom
control_plane_has_stub=False
health_backend=unicom-cloud-drive
health_status=healthy
health_has_stub=False
admin_default_http=401
0.0.0.0:445 smbd
0.0.0.0:61080 gatewayd
0.0.0.0:61081 gatewayd
smb_state=running
smb_listener_ready=True
smb_total_rss_bytes=3604480
```

## SMB Sidecar Boundary

`--enable-smb-sidecar` installs SMB dependencies, enables sidecar systemd units, turns on SMB in the
gateway configuration/control-plane, and starts managed `smbd` on `0.0.0.0:445`.

The .49 LXC guest currently does not expose `/dev/fuse`. This is acceptable for the package-level
listener/startup test: managed `smbd` listens on `0.0.0.0:445`, and Admin can configure SMB users.
Real rclone-backed shares such as `CCBGRoot` still require the LXC guest to expose `/dev/fuse`.

## 2026-06-06 FUSE Enablement and Sidecar Repair Follow-up

Follow-up package and code state:

- Package: `target/lxc-package/ccbg-lxc-package.tar.gz`
- Package SHA256: `ba2027cdc4c7e0e78f3a3f3eb218c2f12d518b4f9b51466cd2be6cadcbfbca5b`
- Deployed `gatewayd` SHA256: `4f56487f775c3338d732998ec1f002bc2625b06ac5bebfc9f958e51eabe208d1`
- Deployed Admin HTML SHA256: `becc45f92ed148ba879621d908abf2439cadef333c47af9a33c198f9e102ae0a`

Observed sidecar failure before the repair:

```text
state=error
listener=0.0.0.0:445
listener_ready=false
last_error=smbd exited immediately; see /var/lib/ccbg/smb-sidecar/data/runtime/logs/smbd-launch.log
```

Root cause:

- `smbd` still expected `/run/samba/ncalrpc`
- in this LXC guest the distro `smbd.service` was intentionally disabled, so nothing created that
  runtime pipe root before the managed sidecar launched `smbd`

Code repair:

- `smb-sidecar-host` now creates `/run/samba` and `/run/samba/ncalrpc` before
  launching managed `smbd`

PVE host change used to expose FUSE to CT `104`:

```bash
pct stop 104
cat >> /etc/pve/lxc/104.conf <<'EOF'
features: fuse=1,nesting=1
lxc.cgroup2.devices.allow: c 10:229 rwm
lxc.mount.entry: /dev/fuse dev/fuse none bind,create=file,optional 0 0
EOF
pct start 104
```

Guest-side reconcile after the host change:

```bash
mkdir -p /run/samba/ncalrpc
systemctl start ccbg-smb-sidecar-sync.service
/opt/ccbg/bin/smb-sidecar-host status
```

Validated final runtime state on `.49`:

```text
/dev/fuse present
ccbg-root:root on /mnt/ccbg/smb/mounts/root type fuse.rclone
192.168.1.49:445 smbd
state=running
listener_ready=true
enabled_share_count=1
mounted_share_count=1
process_count=2
last_error=null
```

Admin verification on the logged-in dashboard/session:

- SMB Runtime moved from `error` to `degraded` after the sidecar repair
- SMB Runtime moved from `degraded` to `running` after `/dev/fuse` was exposed and the sidecar
  reconciled successfully
- `Shares` returned from `0/1` to `1/1`

## Local Verification

Run on the build workspace:

```bash
node admin-js-parse-check
bash -n deploy/lxc/install.sh deploy/lxc/ccbg-smb-sidecar.sh scripts/build-lxc-package.sh
python scripts/check-cloudflare-public-fingerprint.py --out-dir target/cloudflare-fingerprint-check
git diff --check
sha256sum -c target/lxc-package/ccbg-lxc-package.tar.gz.sha256
sha256sum -c MANIFEST.sha256
```

Results:

- Admin inline JavaScript parsed successfully.
- SMB sidecar Python compiled successfully.
- LXC shell scripts passed `bash -n`.
- Cloudflare public fingerprint check passed.
- `git diff --check` had no whitespace errors.
- LXC package tarball checksum and package `MANIFEST.sha256` passed.

## 2026-06-05 China Mobile 16 GiB Follow-up

This follow-up did not replace the main `.49` systemd service. It used isolated temporary gateway
processes on loopback-only ports so the main service stayed healthy on `:61080/:61081`.

Main-service guardrails that stayed green during the test:

```bash
systemctl is-active ccbg
curl -fsS http://127.0.0.1:61080/healthz
```

Temporary isolated Mobile instance:

```text
gateway=127.0.0.1:63290
admin=127.0.0.1:63291
auth_callback=127.0.0.1:63292
metrics=127.0.0.1:63293
binary=/tmp/gatewayd-mobile-test-3
log=/tmp/ccbg-mobile-test-3.log
```

Validated code state:

- kept the China Mobile batching fix
  - `file/create` sends at most the first `100` `partInfos`
  - `parallelUpload=false`
  - remaining upload URLs are fetched through `file/getUploadUrl`
- removed the later route-policy host override experiment because it caused upstream `503`
  responses and did not improve the large-upload result

Probe command:

```bash
python3 /tmp/streaming_signed_put.py 127.0.0.1 63290 root mobile-16g-probe-20260606-v6.bin 17179869184 ccbg change-me
```

Observed result:

```text
HTTP 500 from gateway
China Mobile file/create rejected the request: code=04010319 message=Insufficient Rights
```

Conclusion:

- the batching fix is valid and should stay
- but `.49` still has no verified evidence that the current China Mobile account/session can upload
  `16 GiB`
- release/Admin/docs copy must continue to describe China Mobile large-file support conservatively
  until a newer limit-probe or isolated live test proves otherwise

Follow-up on 2026-06-06:

- `provider-mobile` now also handles the China Mobile `file/create` overwrite branch where
  the upstream responds with `success=true`, `rapidUpload=false`, `exist=true`, and a missing
  `uploadId`
- when that branch is observed under the managed root, the gateway resolves same-name existing
  files, deletes them through native `file/batchDelete`, and briefly retries `file/create`
- this follow-up only fixes incremental overwrite writes for existing managed graph keys; it does
  not change the conservative large-file support statement above
