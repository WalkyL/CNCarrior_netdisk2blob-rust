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

## Local Verification

Run on the build workspace:

```bash
node admin-js-parse-check
python -m py_compile deploy/lxc/ccbg-smb-sidecar.py
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
