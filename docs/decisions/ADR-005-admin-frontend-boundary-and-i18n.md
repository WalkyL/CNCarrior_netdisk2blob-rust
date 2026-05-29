SPDX-License-Identifier: LicenseRef-CCBG-Commercial

# ADR-005: Keep Admin UX Logic In Frontend Assets And Localize Dynamic Panels In JS

## Status

Accepted

## Date

2026-05-28

## Context

The Admin UI is now large enough that mixing HTML templates, dynamic copy, FAQ
matching, AI explanation prompts, and runtime panel rendering back into Rust
would make `gatewayd` heavier to maintain and easier to regress.

The project also has two explicit constraints:

- frontend concerns stay in frontend assets unless a backend API is strictly
  required
- Rust runtime memory must stay bounded and should not grow just to support UI
  presentation or operator guidance

Recent work also exposed a recurring issue: static Chinese labels were
translated, but dynamically re-rendered operator assistant panels could still
show English if their generated DOM did not re-run localization.

## Decision

Use the extracted Admin asset at `crates/gatewayd/assets/admin/index.html` as
the primary home for Admin UI behavior and localization.

- Rust serves the asset and exposes only the APIs needed for status, auth,
  topology, logs, object actions, FAQ lookup inputs, and other runtime control
  surfaces
- Admin-side AI explanation stays frontend-only; the browser composes the log
  excerpt, matched FAQ entries, and prompt for the configured LLM endpoint
- dynamic Admin panels must localize through JS `tr(...)` keys or exact-text
  remapping, and re-apply translations after each render when panels rebuild DOM
- provider-specific UI guidance belongs in frontend rendering helpers and
  catalog/config data, not in new Rust-only presentation branches
- any new admin-facing design choice should be recorded in project docs or ADRs
  instead of living only in inline code comments or chat history

## Consequences

- `gatewayd` remains responsible for APIs, auth, state, and bounded runtime
  data structures, not for assembling operator-facing prose-heavy UI fragments
- frontend iterations can continue without raising Rust memory pressure or
  widening backend responsibilities
- localization work must audit both initial HTML and runtime-generated DOM
- future Admin UI work should prefer:
  1. extracted HTML/CSS/JS asset changes
  2. config/catalog updates
  3. backend API changes only when the UI cannot function without new data

## Alternatives Considered

### Move More Admin Rendering Back Into Rust

Rejected. It would couple UI copy and runtime data assembly too tightly to the
daemon, increase review cost, and make localization slower.

### Add A Dedicated Frontend Build System First

Deferred. The current single-asset Admin page is workable. A separate frontend
build may come later, but it is not required to preserve the current boundary.
