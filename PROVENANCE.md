SPDX-License-Identifier: LicenseRef-CCBG-Commercial

Copyright (c) 2026 walky

# Provenance And Release Fingerprints

CCBG release artifacts carry a consistent provenance fingerprint so copied
frontend assets, documentation, binaries, logs, backups, and release bundles can
be tied back to the canonical project history.

Current development fingerprint:

```text
ccbg-0.1.12-walky-20260630
```

Fingerprint SHA-256:

```text
b80d0b733277d9fa83b8ae8dcd468f6ad0a460301db449ef9e85e87aeebfbe93
```

Fingerprint seed:

```text
carrier-cloud-blob-gateway|2026|walky|v0.1.12|commercial-core|personal-source-review-v1
```

Seed SHA-256:

```text
ef4c91d401857cfd45d324c0a1c20aef85e71d625c3b779c66f25d82a0071bd6
```

## Where The Fingerprint Appears

- `gatewayd --version`
- `GET /__ccbg`
- Admin `GET /api/status` under `runtime.provenance`
- metrics health `GET /healthz` under `runtime.provenance`
- startup logs
- encrypted gateway backup bundle metadata and archive headers
- Cloudflare public frontend HTML meta tags
- Cloudflare `manifest.json`
- Cloudflare `.well-known/ccbg-provenance.json`
- Cloudflare `_headers` as `X-CCBG-Provenance`
- frontend source map metadata and release notes

## Release Evidence Checklist

For every public or commercial release:

1. create a signed git tag
2. build the commercial gateway binary with `CCBG_RELEASE_FINGERPRINT`
   matching the release manifest
3. build or copy the Cloudflare public frontend with the same fingerprint
4. generate release tarballs and SHA-256 files
5. generate an SBOM or dependency inventory that includes the same fingerprint
6. publish release notes containing the fingerprint, tag, artifact hashes, and
   canonical repository URL
7. archive logs or timestamped records proving when the release was built and
   published

## Automation

Use [scripts/generate-release-provenance.py](scripts/generate-release-provenance.py)
to generate release provenance JSON and Markdown records from concrete release
artifacts:

```bash
scripts/generate-release-provenance.py \
  --release-name v0.1.12 \
  --tag v0.1.12 \
  --artifact target/openwrt-lite/ccbg-openwrt-lite.tar.gz \
  --build-step "scripts/build-openwrt-lite-package.sh"
```

The generated record hashes every listed artifact, captures the git commit,
GitHub Actions context when present, build steps, local build environment, and a
`provenance_sha256` over the release evidence.

## Licensing Note

The Personal Source Review Grant is not an open-source license. It is a
discretionary, written, non-commercial personal grant for verified users after
90 or more consecutive days of real use. Commercial use, hosted services,
redistribution, and competitive products require a written commercial license.

If any prior public release was distributed under another license, that license
may continue to apply to that exact prior release. New versions should use the
commercial/public-materials boundary documented here.
