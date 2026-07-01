# OPS-013: `.49` MCP Storage Discovery And Affiliation Routing Deploy

## Status

Deployed to `.49` (`root@192.168.1.49`, hostname `ccbg01`) on 2026-06-24.

## Objective

Deploy the following changes to `.49`:

- public MCP storage-access discovery surface
- Admin S3 application mapping fields (`affiliation`, `application_root`, `user_root_template`)
- application credential export / copy guidance updates
- `affiliation`-aware home write preference for new writes, without overriding matching `content_policy`

## Important Build Constraint

`.49` runs a Linux ELF `gatewayd`. A Windows release host can easily have:

- a newly built `target/release/gatewayd.exe`
- an older `target/release/gatewayd` or `target/x86_64-unknown-linux-gnu/release/gatewayd`

That means `cargo build --release -p gatewayd` on Windows is not a safe `.49` deploy step by itself.

This deploy used the local Podman build-runner image to build the Linux ELF explicitly:

```bash
scripts/build-linux-release-in-podman.sh --target x86_64-unknown-linux-gnu --package gatewayd --package smb-sidecar-host
scripts/build-lxc-package.sh --skip-build --target x86_64-unknown-linux-gnu
```

The local build-runner image used here was:

- `localhost/product-build-runner:latest`

## Local Artifacts

Linux ELF:

- path: `target/x86_64-unknown-linux-gnu/release/gatewayd`
- sha256: `d8897dc5635e3c1f719ef07e53374cf48f260c036198d22378e67fc69c761725`

Admin asset:

- path: `crates/gatewayd/assets/admin/index.html`
- sha256: `806fd3916010d488f614308ef03d0ce7767c1f42bc1f34cc6e0bccaf2caa05bd`

LXC package:

- path: `target/lxc-package/ccbg-lxc-package.tar.gz`
- sha256: `f84f31318aa0ab0fdb5f3b0ac82dbc8a119903027c98429faeb445d3652d899c`

## Deploy Commands

```bash
scp target/lxc-package/ccbg-lxc-package.tar.gz root@192.168.1.49:/tmp/ccbg-lxc-package.tar.gz
ssh root@192.168.1.49
rm -rf /tmp/ccbg-lxc-package
mkdir -p /tmp/ccbg-lxc-package
cd /tmp/ccbg-lxc-package
tar --no-same-owner -xzf /tmp/ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
./scripts/install.sh --enable-smb-sidecar
```

Installer behavior observed:

- existing `/etc/ccbg/ccbg.env` was preserved
- package sample was written to:
  - `/etc/ccbg/ccbg.env.package-20260624125836`

## Remote Installed State

Installed hashes on `.49` after deploy:

- `/opt/ccbg/bin/gatewayd`
  - `d8897dc5635e3c1f719ef07e53374cf48f260c036198d22378e67fc69c761725`
- `/opt/ccbg/assets/admin/index.html`
  - `806fd3916010d488f614308ef03d0ce7767c1f42bc1f34cc6e0bccaf2caa05bd`
- `/tmp/ccbg-lxc-package.tar.gz`
  - `f84f31318aa0ab0fdb5f3b0ac82dbc8a119903027c98429faeb445d3652d899c`

Systemd state immediately after deploy:

- `ccbg.service`
  - `ActiveState=active`
  - `SubState=running`
  - `ExecMainPID=14244`
  - `ExecMainStartTimestamp=Wed 2026-06-24 12:58:39 UTC`

## Acceptance

Backend checks:

- `curl -fsS http://127.0.0.1:61080/healthz`
  - returned `healthy`
- `curl -fsSI http://127.0.0.1:61081/`
  - returned `302 Found`
  - `location: /login`

Admin asset checks:

- `bucket + prefix` guidance is present
- `region` neutrality guidance is present
- `归属地`
- `应用根`
- `用户根模板`
- `显示 S3 凭据`
- `复制 S3 凭据`
- `接入片段`
- `CCBG_S3_USER_ROOT_TEMPLATE` snippet output
- `/api/applications/{id}/credentials`
- `showApplicationCredentials(...)`
- `copyApplicationCredentials(...)`

## Cleanup

Deployment temp files were removed from `.49` after verification:

- removed `/tmp/ccbg-lxc-package`
- removed `/tmp/ccbg-lxc-package.tar.gz`

Retained:

- `/opt/ccbg/backups/`
- deployed `/opt/ccbg/bin/gatewayd`
- deployed `/opt/ccbg/assets/admin/index.html`
