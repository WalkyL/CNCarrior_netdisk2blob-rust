SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials

# CCBG Cloudflare Public Frontend

This directory is the public, source-available Cloudflare Pages surface for
Carrier Cloud Blob Gateway. It is intentionally separate from the commercial
gateway core and does not handle user cloud-drive credentials.

It provides:

- product overview and documentation links
- FAQ catalog and weighted FAQ match API (`/api/faq/catalog`, `/api/faq/match`)
- install/download entry for LXC/Linux, fnOS(experimental), OpenWrt(experimental)
- commercial authorization entry
- personal non-commercial source review request entry
- static demo control panel
- provenance endpoints and headers

Deploy this directory as Cloudflare Pages assets with Pages Functions enabled.

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
