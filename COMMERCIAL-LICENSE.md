SPDX-License-Identifier: LicenseRef-CCBG-Commercial

# CCBG Commercial License

Copyright (c) 2026 walky. All rights reserved.

This is a source-available commercial license boundary for Carrier Cloud Blob
Gateway ("CCBG"). It is intended as a working project license and must be
reviewed by qualified counsel before public launch.

## Scope

The commercial license applies to all core gateway source code and private
implementation material unless a narrower file or directory license says
otherwise. That includes, without limitation:

- `crates/`, `tools/`, `scripts/`, `deploy/`, and provider catalogs under
  `config/`
- gateway runtime code, provider adapters, metadata, replication, encryption,
  backup, WAL, browser automation, and policy logic
- build manifests, release provenance, and non-public operational material

## Default Restrictions

Without a separate written license from walky, you may not:

- use CCBG or derivative works for commercial purposes
- provide CCBG, a modified CCBG, or a substantially equivalent derivative as a
  paid or free hosted service, managed service, SaaS, appliance, marketplace
  offering, or competitive gateway product
- redistribute, sublicense, sell, lease, or transfer source code or binaries
- remove, alter, hide, or misrepresent copyright, license, trademark,
  provenance, release fingerprint, or source attribution notices
- use the CCBG name, logos, or confusingly similar marks in a way that implies
  sponsorship, endorsement, or origin by walky
- copy protected implementation details into another commercial or hosted
  product without written permission

## Enterprise And Commercial Use

Commercial use requires a written commercial license from walky. A commercial
license can grant rights for internal production deployment, redistribution,
hosting, OEM embedding, support, or custom integration only to the extent stated
in the signed agreement.

## Personal Source Review Grant

An individual real user may apply for a personal, non-commercial source review
grant after at least 90 consecutive days of genuine personal use of CCBG or a
CCBG-hosted service.

If granted in writing, this personal grant may allow the named individual to:

- inspect the source code for security, privacy, reliability, and learning
  purposes
- build and run the code for their own non-commercial personal environment
- submit private vulnerability reports, issue reports, or proposed patches to
  walky

This grant does not allow the individual to:

- use the code commercially
- provide a hosted service or managed service
- redistribute, mirror, publish, sublicense, or transfer the source code or
  binaries
- share access with an employer, client, team, company, lab, community, or
  public repository
- remove or alter provenance, copyright, license, trademark, or release
  fingerprint notices
- use the code to build, train, benchmark, or operate a competing commercial
  product or hosted service

Approval is discretionary. walky may require reasonable proof of real personal
use, account continuity, abuse-free usage, and identity/contact information.
The grant is personal, non-transferable, non-exclusive, revocable on breach,
and separate from any open-source license.

## Public Frontend And Documentation

Public-facing documentation, screenshots, Cloudflare public frontend code, and
brand/product copy may be distributed separately under
`PUBLIC-MATERIALS-LICENSE.md` or another explicit public-materials notice.
Those terms do not grant rights to the core gateway implementation.

## Prior Versions

If any earlier public release was distributed under another license, that
license may continue to apply to that exact prior release. This license governs
new versions and new material released under this repository boundary.

## No Warranty

CCBG is provided "as is", without warranty of any kind, express or implied,
including warranties of merchantability, fitness for a particular purpose,
non-infringement, availability, security, or data durability. To the maximum
extent permitted by law, walky is not liable for any claim, damages, data loss,
service interruption, or other liability arising from use of CCBG.
