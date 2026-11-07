SPDX-License-Identifier: LicenseRef-CCBG-Commercial

# ADR-007: High-Tier Holistic Review Gate for Material Changes

## Status

Accepted

## Date

2026-11-08

## Context

CCBG is a large Rust workspace spanning many crates (data plane, control plane,
replication, metadata store, provider bridges, admin UI, MCP). Changes can
accumulate and drift from the frozen architecture, erode module boundaries,
raise memory pressure, and degrade code quality in ways a per-diff review by a
single coding model may miss. We need a **deterministic, automatic gate**: when a
change is material (in scope or risk), a high-tier review model must perform a
**holistic** review before the change is accepted. The gate must be objective
and repeatable, and must not depend on the author's judgment.

## Decision

Establish the **High-Tier Holistic Review Gate**. A material change MUST be
reviewed holistically by high-tier review models (GLM-5.3 / kimi-k3 / astra)
before merge/acceptance, against the criteria below. The gate is an **additional
layer** on top of the existing per-diff review, not a replacement.

### 1. Trigger — a change is "material" if ANY of the rules fires

| Rule | Detection (objective) |
|------|-----------------------|
| M1 | Net added + deleted lines across the change ≥ **500** (working tree + staged, or branch vs `--base`) |
| M2 | Changed files span **> 2 crates** (under `crates/`) — a cross-crate change |
| M3 | Any change to a **frozen contract**: public API signature, config/env contract (`CCBG_*`, `MCP_*`), CLI flag, on-disk schema, protocol wire format, admin API route/DTO |
| M4 | Any change to a **security boundary**: auth, CORS/CSP, cookies, crypto, credentials, key handling, TLS |
| M5 | Any change in a **hot path**: data-plane request path, replication engine, metadata-store transaction, WAL, spool |
| M6 | A **new dependency** added (any `Cargo.toml` `[dependencies]` addition) or a dependency feature enabled |
| M7 | Any `unsafe` / `transmute` / raw-pointer / `static mut` change |
| M8 | Any change to `AdminMode` (incl. `Terminal`), the admin auth chain, or `unsafe`/`async`/lifetime semantics |

`scripts/check-review-gate.py` implements M1–M8 from `git diff`. If it fires, the
change cannot merge until a holistic review is recorded as APPROVE.

### 2. Holistic review criteria (objective, checked against the whole change, not line-by-line)

1. **Goal fidelity (目标不偏离)** — implements only the frozen contract; no
   scope creep, no speculative features, no unrelated refactors bundled in the
   same change.
2. **Cohesion & coupling (高内聚低耦合)** — module boundaries respected;
   dependency direction follows `docs/component-dependency-map.*`; no god
   objects; no leaking internals across crate boundaries; no new cycle.
3. **Pluggability (能力可插拔)** — new capabilities behind traits/ports/config;
   no hardcoded provider/backend switching; extension points not duplicated;
   consistent with existing provider-bridge / catalog patterns.
4. **Memory bound (内存占用最小)** — bounded allocations on hot paths; spooling
   thresholds respected (`CCBG_MAX_IN_MEMORY_OBJECT_BYTES` etc.); no unbounded
   `Vec`/`HashMap` growth; no per-request allocations that scale with payload
   without bound; no leaks; consistent with `docs/resource-budget.md`.
5. **Code style & taste (风格与品味)** — minimal, idiomatic, readable; precise
   naming; no clever tricks, no hidden magic, no over-abstraction; consistent
   error handling (no blanket `unwrap()`/`panic!` on fallible paths); no
   `unsafe` without a documented invariant; deterministic; matches the dominant
   style of the codebase. Concretely, the reviewer MUST flag as a defect:
   - **obfuscated / needlessly verbose naming** (e.g. `handleDataForObjectViaProviderXyz`,
     `getAndThenProcessTheThing`, single-letter names beyond loop indices);
   - **run-on / verbose expression** (e.g. `if (data && data.length && data.length > 0 &&
     data[0] && data[0].value && data[0].value !== null)`), nested ternaries,
     chains of `!= null && != undefined`, over-defensive null checks;
   - **copy-paste duplication** and **speculative abstraction** (a trait/abstraction
     with a single implementation added "in case it's needed later");
   - **hidden state / magic numbers / boolean-parameter tunneling** that obscures
     intent.
6. **No dead / speculative code (无垃圾代码)** — no dead code, no copy-paste, no
   TODO/FIXME/HACK litter, no confusing renames, no speculative complexity, no
   commented-out blocks.
7. **Text register (文字风格)** — every piece of project prose must be written
   in one of exactly three rigorous registers:
   - **严谨的英语** (rigorous English): correct grammar, precise terminology,
     no slang, no filler, no run-on or overly ornate phrasing;
   - **严谨的白话中文** (rigorous vernacular Chinese): standard written
     Chinese (简体), precise, unambiguous, no colloquialisms, no dialect, no
     internet slang, no code-switching mid-sentence without cause;
   - **严谨的中文文言文** (rigorous Classical Chinese): correct 文言 grammar and
     diction, used only where deliberate and warranted (e.g. an epigraph or a
     formal ADR preamble), not as a default.
   This applies to identifiers, comments, doc strings, log messages, error
   messages, user-facing UI copy, commit messages, and docs. Code that mixes
   registers gratuitously, uses broken or ambiguous phrasing, or relies on
   ungrammatical shorthand is a defect. (English identifiers are preferred in
   code; Chinese is acceptable in comments/docs but must be rigorous.)

### 2a. Mandatory change documentation

Every accepted change MUST land a corresponding update to the project docs, so
the codebase, its decisions, and any handoff are traceable without chat history.

- **Feature / behavior change** → a section in `docs/` (e.g. the relevant
  deployment/feature doc) describing what changed and how to use it.
- **Architecture / contract change** → an ADR under `docs/decisions/` (frozen
  contract changes REQUIRE an ADR).
- **Configuration contract change** (`CCBG_*`, `MCP_*`) → document the new
  variable in the relevant env/config doc (`config/example.env`,
  `docs/openwrt-lite-deployment.md`, etc.).
- **Deployment / handoff change** → the runbook or handoff doc it affects
  (`docs/reliability-remediation-todo.md`, release checklist, etc.).
- A change that modifies behavior WITHOUT a corresponding doc update fails the
  holistic review (this is checked as part of Goal Fidelity in §2).

### 3. Review models and protocol

- Reviewer models: **GLM-5.3** (highest reasoning effort), **kimi-k3**, **astra**.
- At least **2** independent models per material change; use **all 3** for
  security-boundary (M4) or frozen-contract (M3) changes.
- Review is **read-only**: the reviewer model returns findings; it does not edit
  code. The author addresses findings, then the change is re-reviewed.
- Verdict handling:
  - All APPROVE → gate passes.
  - Any REQUEST_CHANGES → gate blocks; author fixes and re-submits.
  - Models disagree → **human adjudicates** (never silently prefer one model).
- Evidence is recorded (prompt + model verdict JSON) under `.cache/reviews/` or
  `docs/reviews/` with the commit hash it reviewed.
- The style/taste criteria in §2 item 5 are **mandatory review targets**: a
  reviewer that APPROVEs a change containing obfuscated naming, run-on
  expressions, non-idiomatic phrasing, copy-paste, or speculative abstraction is
  failing its gate duty, not just offering taste commentary.
- The **text register** criteria in §2 item 7 are likewise **mandatory**: a
  reviewer must flag any identifier, comment, doc string, log line, error
  message, UI copy, commit message, or doc that is not rigorous English, rigorous
  vernacular Chinese, or rigorous Classical Chinese.

### 4. Operationalization

- `scripts/check-review-gate.py [--base <ref>] [--json]` prints `GATE_FIRES` /
  `GATE_PASSES` plus the matched rules and the recommended reviewer set.
- Default detection is the working tree (unstaged + staged); pass `--base <ref>`
  to compare a branch against a base ref.
- The release / merge process runs the script; if it fires and no APPROVE
  evidence exists for the current change, the change is blocked.
- **Change documentation check** (§2a): a material change that does not include a
  corresponding `docs/` update is flagged by the holistic review and blocks the
  gate until the doc lands.

### 5. Relationship to existing conventions

- Per-diff review by coding models is unchanged and remains the first line of
  defense; this ADR adds a holistic layer for material changes.
- The frozen-architecture rule is **not** relaxed: low-tier/coding models still
  implement only frozen contracts; the gate confirms the implementation stays on
  target and meets the quality bar.

## Consequences

- Material changes get a second, holistic, model-driven review → lower drift,
  higher cohesion, bounded memory, consistent style/taste.
- Small, localized changes (below all thresholds) proceed with the existing
  per-diff review only → no added friction for routine fixes.
- Gate requires recording review evidence → small process overhead; reviewers
  are the existing high-tier models, so no new infrastructure.

## Alternatives Considered

### Human-only holistic review
Rejected: does not scale, is not repeatable, and the project already relies on
model review for depth.

### Gate on every change
Rejected: adds friction to trivial fixes without proportionate value.

### Single reviewer model
Rejected: a single model can be confidently wrong on style/taste and miss
cross-cutting drift; two-plus models plus human adjudication on disagreement
reduce that risk.
