SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials

# CCBG Cloudflare Public Frontend

This directory is the public, source-available Cloudflare Pages surface for
Carrier Cloud Blob Gateway. It is intentionally separate from the commercial
gateway core and does not handle user cloud-drive credentials.

It provides:

- product overview and documentation links
- FAQ catalog and weighted FAQ match API (`/api/faq/catalog`, `/api/faq/match`)
- install/download catalog for PVE LXC, Docker, Podman, Windows, macOS,
  fnOS(experimental), OpenWrt(experimental), plus STM32 / ESP32-S3 embedded
  client examples
- commercial authorization entry through `https://register.agi2030.online`
- personal non-commercial source review request entry through `https://register.agi2030.online`
- install-first dark public homepage aligned with the llm-router operational palette
- provenance endpoints and headers

Deploy this directory as Cloudflare Pages assets with Pages Functions enabled.

If the available Cloudflare credential has Workers permissions but no Pages
permission, deploy the same public surface as a Worker with static Assets:

```bash
cd ../..
scripts/stage-cloudflare-public-assets.sh target/cloudflare-public-assets

export CLOUDFLARE_API_TOKEN="$CF_API_TOKEN"
export CLOUDFLARE_ACCOUNT_ID="$CF_ACCOUNT_ID"
wrangler deploy -c public/cloudflare/wrangler.worker.toml \
  --assets target/cloudflare-public-assets \
  --domain carrier-disk-gateway.agi2030.online
```

This fallback serves `/`, `/faq/`, `/install/`, `/api/faq/catalog`, and
`/api/faq/match`. Static data files under `/data/` are served as assets.
It now also proxies release binaries through `/downloads/latest/<asset-name>`.

## Local Deployment

Cloudflare deployment is run from a local machine with Cloudflare credentials:

```bash
cd ../..
scripts/deploy-cloudflare-public.sh test
scripts/deploy-cloudflare-public.sh production
```

Required local environment variables:

- `CLOUDFLARE_API_TOKEN` or `CF_API_TOKEN`
- `CLOUDFLARE_ACCOUNT_ID` or `CF_ACCOUNT_ID`

Optional local environment variables:

- `CCBG_CF_TEST_WORKER` defaults to `ccbg-public-test`
- `CCBG_CF_TEST_DOMAIN` is optional; if it is unset, the test branch only updates the test Worker
- `CCBG_CF_PROD_WORKER` defaults to `ccbg-public`
- `CCBG_CF_PROD_DOMAIN` defaults to `carrier-disk-gateway.agi2030.online`
- `CCBG_CF_BIND_DOMAIN_ON_DEPLOY` defaults to unset/false. Set it to `true`
  only for the one-time custom-domain binding job with a token that has zone
  route permissions.
- `CCBG_CF_SMOKE_DOMAIN_ON_DEPLOY` defaults to unset/false. Set it to `true`
  only when GitHub-hosted runners can access the production domain without
  Cloudflare WAF or edge security returning a false 403.
- `CCBG_CF_RELEASE_R2_BUCKET` enables R2-backed release caching for both test
  and production deployments.
- `CCBG_CF_TEST_RELEASE_R2_BUCKET` and `CCBG_CF_PROD_RELEASE_R2_BUCKET`
  override the shared bucket on a per-environment basis.
- `PUBLIC_RELEASE_REPO` defaults to `WalkyL/CNCarrior_netdisk2blob-rust`.
- `GITHUB_RELEASE_TOKEN` is optional. Set it when the release source is
  private or when the Worker should resolve release assets through the GitHub
  API with an authenticated token.

Optional Cloudflare bindings for release caching:

- `RELEASE_ASSETS`: optional R2 bucket binding. When present, the public site
  serves `/downloads/latest/<asset-name>` from `latest/<asset-name>` in R2
  before falling back to GitHub.

To prefill the release cache manually:

```bash
scripts/sync-cloudflare-release-cache.sh your-r2-bucket
```

Current rollout as of 2026-06-02:

- test Worker: `ccbg-public-test`
- production Worker: `ccbg-public`
- test release cache bucket: `ccbg-release-assets-test`
- production release cache bucket: `ccbg-release-assets`
- production download verification:
  `HEAD /downloads/latest/ccbg-lxc-package.tar.gz` -> `200`, `x-ccbg-release-source=r2`
  `HEAD /downloads/latest/ccbg-windows-x86_64.zip` -> `200`, `x-ccbg-release-source=r2`
- install catalog exposes both LXC profiles:
  `scripts/install.sh --s3-only` for S3 gateway only, and
  `scripts/install.sh --enable-smb-sidecar` for SMB sidecar dependencies,
  systemd units, Admin/control-plane enablement, and one reconcile pass.

Routine production deploys update the existing Worker + Assets deployment and
skip custom-domain rebinding. The custom domain should already point at the
Worker. Run the public URL smoke checks from an allowed network after deploy.

## Local Preview

```bash
cd public/cloudflare
python3 -m http.server 8788
```

Then open `http://127.0.0.1:8788/`.

For API/function preview:

```bash
cd public/cloudflare
wrangler pages dev .
```

Then verify:

```bash
curl -sS http://127.0.0.1:8788/api/faq/catalog | jq '.count'
curl -sS -X POST http://127.0.0.1:8788/api/faq/match \
  -H 'content-type: application/json' \
  -d '{"query":"mobile token expired","provider":"mobile","context":"logs","limit":3}'
curl -I http://127.0.0.1:8788/downloads/latest/ccbg-lxc-package.tar.gz
```

## Provenance

The current public frontend fingerprint is:

```text
__CCBG_PUBLIC_RELEASE_FINGERPRINT__
```

The same fingerprint appears in HTML meta tags, `manifest.json`,
`.well-known/ccbg-provenance.json`, `_headers`, `assets/app.js`,
`assets/app.js.map`, and release notes.
