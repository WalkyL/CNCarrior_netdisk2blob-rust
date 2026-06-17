# OPS-012: `.49` Browser-Flow LLM Repair Deployment

## Status

Deployed to `.49` (`root@192.168.1.49`, hostname `ccbg01`) on 2026-06-17 16:05 UTC.

Do not deploy only `assets/admin/index.html`; the Admin button depends on the new backend routes.

Build note: `.47` is the local workstation. The Linux release binary was built locally with
`D:\Tools\zig\zig.exe` and `cargo zigbuild`, then deployed to `.49` as an LXC package. `.49` is a
`96 MiB` LXC and is not an appropriate Rust release build host.

## Objective

Deploy the carrier login plugin self-repair loop to `.49` so operators can repair drifting `unicom` / `telecom` / `mobile` browser-flow catalogs from Admin with CDP + LLM, instead of waiting for a hardcoded selector release.

## Scope

Included:

- effective packaged-plus-override browser-flow catalog loading
- redacted `POST /api/browser-flows/repair-bundle`
- validated `POST /api/browser-flows/overrides`
- Admin buttons and front-end repair workflow for `unicom`, `telecom`, and `mobile`

Not included:

- automatic save without operator confirmation
- direct LLM file writes
- any change to provider data-plane upload/download behavior

## Local Verification Before Deploy

Local checks run for this change set:

```bash
cargo fmt --check
cargo check -p gatewayd
cargo test -p blob-core browser_flow_catalog_collection_overlays_later_directories
cargo test -p gatewayd browser_flow_override_save_is_used_by_effective_catalog_lookup
cargo test -p gatewayd browser_flow_repair_text_redacts_common_secret_shapes
cargo test -p gatewayd admin_page_exposes_object_actions_panel
```

The Linux release package was built and checked locally:

```bash
cargo zigbuild --release --locked --target x86_64-unknown-linux-gnu -p gatewayd
scripts/build-lxc-package.sh --skip-build --target x86_64-unknown-linux-gnu
sha256sum -c target/lxc-package/ccbg-lxc-package.tar.gz.sha256
```

Package:

- `target/lxc-package/ccbg-lxc-package.tar.gz`
- SHA-256: `f92dab7823e78ba353836842ba32b2a51b92661fceb0d3a94b9b63f496716298`

## `.49` Deploy Shape

Deploy as the standard LXC package flow:

```bash
scp target/lxc-package/ccbg-lxc-package.tar.gz root@192.168.1.49:/tmp/ccbg-lxc-package.tar.gz
ssh root@192.168.1.49
rm -rf /tmp/ccbg-lxc-package
mkdir -p /tmp/ccbg-lxc-package
cd /tmp/ccbg-lxc-package
tar --no-same-owner -xzf /tmp/ccbg-lxc-package.tar.gz
cd ccbg-lxc-package
./scripts/install.sh --enable-smb-sidecar
./scripts/smoke.sh
```

The installer kept the existing `/etc/ccbg/ccbg.env` and wrote the package sample to
`/etc/ccbg/ccbg.env.package-20260617160550`.

Post-install checks verified:

- `systemctl is-active ccbg` is `active`
- `./scripts/smoke.sh` passed against `http://127.0.0.1:61080`
- `curl -fsS http://127.0.0.1:61080/healthz` returned a healthy `unicom-cloud-drive` payload
- `curl -fsSI http://127.0.0.1:61081/` returned `302 Found` to `/login`
- packaged Admin HTML contains `LLM 修复登录插件` at the provider login assistants

Installed hashes on `.49`:

- `/opt/ccbg/bin/gatewayd`:
  `e785235f051967f4e0639475ae9090a084a768961f76fa64b27d8085ede3089f`
- `/opt/ccbg/assets/admin/index.html`:
  `e1513c94bae4ad821e33929234abdff446eca3a57b8cdf8bfce27b7463d235e2`
- `/tmp/ccbg-lxc-package.tar.gz`:
  `f92dab7823e78ba353836842ba32b2a51b92661fceb0d3a94b9b63f496716298`

## Operator Acceptance On `.49`

After deploy, the logged-in Admin session should show:

- `LLM 修复登录插件` in the `unicom` login assistant
- `LLM 修复登录插件` in the `telecom` login assistant
- `LLM 修复登录插件` in the `mobile` login assistant

Suggested live acceptance:

1. Keep a real carrier page open in the selected CDP browser.
2. Open the matching provider login assistant.
3. Trigger `LLM 修复登录插件`.
4. Confirm the bundle is generated and the Admin page either:
   - calls the front-end LLM successfully, or
   - copies the repair prompt for manual use
5. Save a confirmed override only after reviewing the summary/risk.
6. Retry the same login flow and verify the new override is used.
