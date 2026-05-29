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
