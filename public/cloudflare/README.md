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

## GitHub Actions Deployment

The repository deployment path is branch-based:

- pushing `test` deploys the Worker + Assets test target
- pushing `main` deploys the production Worker + Assets target

Required GitHub secrets:

- `CLOUDFLARE_API_TOKEN` or `CF_API_TOKEN`
- `CLOUDFLARE_ACCOUNT_ID` or `CF_ACCOUNT_ID`

Optional GitHub variables:

- `CCBG_CF_TEST_WORKER` defaults to `ccbg-public-test`
- `CCBG_CF_TEST_DOMAIN` is optional; if it is unset, the test branch only updates the test Worker
- `CCBG_CF_PROD_WORKER` defaults to `ccbg-public`
- `CCBG_CF_PROD_DOMAIN` defaults to `carrier-disk-gateway.agi2030.online`

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
```

## Provenance

The current public frontend fingerprint is:

```text
ccbg-0.1.0-walky-20260526-e756003d846d2c46
```

The same fingerprint appears in HTML meta tags, `manifest.json`,
`.well-known/ccbg-provenance.json`, `_headers`, `assets/app.js`,
`assets/app.js.map`, and release notes.
