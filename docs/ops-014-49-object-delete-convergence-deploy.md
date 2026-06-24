# OPS-014: `.49` Object Delete Convergence Deploy

## Status

Deployed to `.49` (`root@192.168.1.49`, hostname `ccbg01`) on 2026-06-25.

## Objective

Deploy the object delete convergence fix to `.49`:

- S3 `DeleteObject` treats backend `NotFound` as idempotent success
- Admin object-action delete treats backend `NotFound` as idempotent success
- stale placement cleanup removes placement, logical object, and protection-plan metadata together
- Admin UI copy matches the new cleanup semantics

## Important Build Constraint

`.49` runs a Linux ELF `gatewayd`. The Windows host must not rely on `cargo build --release -p gatewayd` alone because that can produce a Windows `gatewayd.exe` while the deployed host still needs a Linux ELF.

This deploy used the local Podman build-runner image to build the Linux binary explicitly:

```bash
podman run --rm -v "D:/workspaces/ccbg:/workspace" -w /workspace localhost/product-build-runner:latest bash -lc "cargo build --release --locked --target 'x86_64-unknown-linux-gnu' -p 'gatewayd'"
```

The local build-runner image used here was:

- `localhost/product-build-runner:latest`

## Local Artifacts

Linux ELF:

- path: `target/x86_64-unknown-linux-gnu/release/gatewayd`
- sha256: `ac591c524a3217a4cc0328f05ff8ce133f9a408793bbc989cc4c7ab1cd0504fc`

Admin asset:

- path: `crates/gatewayd/assets/admin/index.html`
- sha256: `f78872fb519be6efa4cf729952f209f1efd60fe9cf08c89e76c0e65fa96982fc`

Git commit:

- `d3389cf` `Fix object delete convergence`

## Deploy Commands

```bash
ssh root@192.168.1.49
systemctl stop ccbg.service
mkdir -p /opt/ccbg/backups /opt/ccbg/bin /opt/ccbg/assets/admin

scp target/x86_64-unknown-linux-gnu/release/gatewayd root@192.168.1.49:/opt/ccbg/bin/gatewayd.new
scp crates/gatewayd/assets/admin/index.html root@192.168.1.49:/opt/ccbg/assets/admin/index.html.new

install -m 0755 /opt/ccbg/bin/gatewayd.new /opt/ccbg/bin/gatewayd
install -m 0644 /opt/ccbg/assets/admin/index.html.new /opt/ccbg/assets/admin/index.html
rm -f /opt/ccbg/bin/gatewayd.new /opt/ccbg/assets/admin/index.html.new

systemctl start ccbg.service
```

## Remote Installed State

Installed hashes on `.49` after deploy:

- `/opt/ccbg/bin/gatewayd`
  - `ac591c524a3217a4cc0328f05ff8ce133f9a408793bbc989cc4c7ab1cd0504fc`
- `/opt/ccbg/assets/admin/index.html`
  - `f78872fb519be6efa4cf729952f209f1efd60fe9cf08c89e76c0e65fa96982fc`

Systemd state immediately after deploy:

- `ccbg.service`
  - `ActiveState=active`
  - `SubState=running`
  - `ExecMainPID=17147`

## Acceptance

Backend checks:

- `curl -fsS http://127.0.0.1:61080/healthz`
  - returned healthy JSON
- `curl -fsSI http://127.0.0.1:61081/`
  - returned `302 Found`
  - `location: /login`

Admin UI checks:

- cleanup copy now says the action removes residual object metadata
- cleanup copy mentions placement, logical object, and protection plan together

## Cleanup

Deployment temp files were removed from `.49` after verification:

- removed `/opt/ccbg/bin/gatewayd.new`
- removed `/opt/ccbg/assets/admin/index.html.new`

Retained:

- `/opt/ccbg/backups/`
- deployed `/opt/ccbg/bin/gatewayd`
- deployed `/opt/ccbg/assets/admin/index.html`
