# Personal Non-Commercial Source Review Process

## Purpose

This process implements the personal source review path described in [COMMERCIAL-LICENSE.md](../COMMERCIAL-LICENSE.md).

It is not an open-source grant. It is a discretionary written permission for a named individual to inspect and run selected source for personal, non-commercial review after at least 90 consecutive days of genuine personal use.

## Eligibility

Minimum eligibility:

- individual person, not a company, team, lab, employer, client, or community account
- at least 90 consecutive days of genuine personal use
- no active abuse, fraud, scraping, credential sharing, or policy breach
- request is for personal security, privacy, reliability, learning, bug reporting, or private patch review
- no commercial, hosted-service, redistribution, OEM, employer, client, benchmark, training, or competitive product use

## Intake

Public intake starts at `https://register.agi2030.online/?product=carrier-cloud-blob-gateway&intent=personal-source-review`.
That identity center is the canonical place to collect sign-in, contact, and
continuity context before a grant is reviewed.

The GitHub issue template
[.github/ISSUE_TEMPLATE/personal-source-review.yml](../.github/ISSUE_TEMPLATE/personal-source-review.yml)
is a fallback triage path only. If a request arrives through GitHub first, direct
the applicant to the AGI2030 Identity Center before collecting private proof or
issuing a written grant.

Do not collect secrets in public issues:

- no provider tokens
- no cookies
- no refresh tokens
- no government IDs
- no private account credentials

If identity or continuity proof is needed, collect it out-of-band and record only a redacted audit note.

## Review Steps

1. Confirm the applicant is an individual.
2. Confirm the request states personal non-commercial intent.
3. Confirm stated use duration is at least 90 consecutive days.
4. Check abuse and support history.
5. Confirm requested scope is source review only, not redistribution or hosting.
6. Choose export scope.
7. Generate review grant decision.
8. Generate source review package through the RELEASE-002 package flow.
9. Record the grant ID, package fingerprint, expiration, reviewer, and decision timestamp.

## Export Scope

Default approved package scope:

- license and notice files
- selected source files needed for review
- build metadata needed to reproduce the reviewed artifact
- redacted configuration samples
- provenance and package manifest

Default exclusions:

- secrets and real credentials
- private deployment logs
- user data
- unrelated local build artifacts
- commercial customer material
- unreleased enterprise-only integrations unless explicitly approved

## Grant Terms Summary

If approved in writing, the named individual may:

- inspect source for security, privacy, reliability, and learning
- build and run it for their own non-commercial personal environment
- send private vulnerability reports, bug reports, or proposed patches to walky

The grant does not allow:

- commercial use
- hosted or managed service use
- redistribution, mirroring, publishing, sublicensing, or transfer
- sharing with an employer, client, company, lab, team, or public repository
- removing copyright, license, trademark, provenance, or fingerprint notices
- building, training, benchmarking, or operating a competing commercial product

## Simulation

Run:

```bash
scripts/source-review-flow.py --simulate
```

The simulation writes an audit decision under `target/source-review-flow/` and proves the process can be repeated without exporting source.
