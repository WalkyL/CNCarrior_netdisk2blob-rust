# Cloudflare Public Fingerprint Verification

`public/cloudflare` is the public-materials surface. It must not contain private
gateway core artifacts, credentials, source review packages, or commercial
release tarballs.

Run local verification:

```bash
scripts/check-cloudflare-public-fingerprint.py
```

The script verifies:

- the release fingerprint appears in required public files
- `.well-known/ccbg-provenance.json` matches the fingerprint SHA-256
- public files receive a generated hash manifest
- obvious private/core artifacts are absent from `public/cloudflare`

The generated manifest is written to:

```text
target/cloudflare-public-fingerprint/public-cloudflare-fingerprint-manifest.json
```

After a Cloudflare Pages deployment, compare the deployed files:

```bash
scripts/check-cloudflare-public-fingerprint.py \
  --deployed-base-url https://<your-cloudflare-pages-host>
```

This fetches each file listed in the local manifest and compares SHA-256 hashes.

For the Worker + Assets production domain, Cloudflare security features can block
Python `urllib` requests with `403`, and can inject challenge JavaScript into HTML
responses. In that case, treat the deployed hash check as advisory and perform
the release smoke with `curl`:

```bash
curl -I https://carrier-disk-gateway.agi2030.online/
curl -I https://carrier-disk-gateway.agi2030.online/faq/
curl -I https://carrier-disk-gateway.agi2030.online/data/faq-catalog.json
curl -I https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-lxc-package.tar.gz
curl -I https://carrier-disk-gateway.agi2030.online/downloads/latest/ccbg-windows-x86_64.zip
```

Minimum acceptance for the production Worker domain:

- HTML/catalog responses return `200` and the expected `x-ccbg-version` /
  `x-ccbg-provenance` headers.
- JSON/static assets that are not rewritten by Cloudflare, such as
  `data/faq-catalog.json`, `.well-known/ccbg-provenance.json`, `assets/app.js`,
  and `manifest.json`, match the staged SHA-256.
- `/downloads/latest/*` release assets return `200` and
  `x-ccbg-release-source: r2`.
- HTML pages are checked by status, headers, and visible content when Cloudflare
  injects challenge JavaScript.
