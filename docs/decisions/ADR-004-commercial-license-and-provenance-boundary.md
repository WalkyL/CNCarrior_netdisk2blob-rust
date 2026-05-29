SPDX-License-Identifier: LicenseRef-CCBG-Commercial

# ADR-004: Use Commercial Core License With Public Materials And Personal Source Review

## Status

Accepted

## Date

2026-05-26

## Context

CCBG needs to be visible enough for users to evaluate, deploy, and discuss, but
the core gateway implementation contains provider adapters, metadata,
replication, encryption, browser automation, and operational details that should
remain commercially controlled.

The project also needs a path for trusted individual users to inspect source
after real personal use, without granting commercial use, redistribution, or
hosted-service rights.

## Decision

Use a commercial-first license boundary:

- core gateway, provider adapters, config catalogs, scripts, tools, and release
  tooling use `LicenseRef-CCBG-Commercial`
- public documentation and the Cloudflare public frontend use
  `LicenseRef-CCBG-Public-Materials`
- verified individual users may apply for a written personal, non-commercial
  source review grant after at least 90 consecutive days of genuine personal use
- enterprise, hosted, redistribution, OEM, and competitive use require a
  separate written commercial license
- release artifacts must carry a shared provenance fingerprint across public
  frontend assets, gateway runtime output, status APIs, logs, backups, release
  notes, SBOM, and signed release records

Current development fingerprint:

```text
ccbg-0.1.0-walky-20260526-e756003d846d2c46
```

## Alternatives Considered

### MIT

Rejected. MIT would allow commercial copying, redistribution, hosted services,
and removal of product control from new versions.

### AGPL For The Whole Repository

Rejected for the core. AGPL would be clearer open source, but it would still
publish implementation details and would not preserve the intended commercial
authorization boundary.

### Delayed Open Source / BSL-Style Release

Deferred. A delayed conversion could be added later, but it should be a
deliberate product decision with legal review and a clear change date.

## Consequences

- Do not describe the whole project as OSI open source.
- Keep the public Cloudflare frontend separate from the core gateway.
- Keep SPDX headers and Cargo `license-file` metadata aligned with the custom
  license files.
- Treat the personal source review path as a discretionary written grant, not an
  automatic public license.
- Legal text must be reviewed by qualified counsel before public launch.
