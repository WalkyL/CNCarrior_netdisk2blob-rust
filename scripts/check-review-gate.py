#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
"""High-tier holistic review gate detector (ADR-007).

Determines whether a change is "material" and therefore MUST be reviewed
holistically by high-tier review models (GLM-5.3 / kimi-k3 / astra) before it
can be merged/accepted.

Usage:
  python scripts/check-review-gate.py [--base <ref>] [--json]

Default detection scope is the working tree (unstaged + staged). Pass --base
<ref> to compare a branch against a base ref instead (e.g. --base origin/main).

Exit code:
  0  -> GATE_PASSES (change is below all material thresholds; no holistic gate)
  1  -> GATE_FIRES  (material change; holistic review REQUIRED before merge)
  2  -> error (could not compute diff / unexpected state)
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

# Material change thresholds (ADR-007).
LINE_THRESHOLD = 500
# Security-boundary markers (M4): any of these tokens touching the diff path
# implies a security-relevant change. Conservative keyword match.
SECURITY_MARKERS = (
    "auth",
    "cors",
    "csp",
    "cookie",
    "crypto",
    "credential",
    "key",
    "tls",
    "frame-ancestors",
    "x-frame",
    "same-site",
    "same_site",
    "secure",
)
# Frozen-contract markers (M3): config/env contract, CLI flags, admin routes.
CONTRACT_MARKERS = (
    "CCBG_",
    "MCP_",
    "ROUTE_",
    "AdminMode",
    "pub fn",
    "pub struct",
    "pub enum",
    "pub trait",
)
# Hot-path markers (M5): data-plane / replication / metadata-store / WAL.
HOT_PATH_MARKERS = (
    "data-plane",
    "replication",
    "metadata-store",
    "wal",
    "spool",
    "S3",
    "handle_",
    "put_object",
    "get_object",
)
# New-dependency markers (M6): Cargo.toml dependency lines.
DEPENDENCY_MARKERS = ("[dependencies]", " = {", " = \"")

PROJECT_ROOT = Path(__file__).resolve().parent.parent


def run_git(*args: str, cwd: Path = PROJECT_ROOT) -> str:
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def diff_for_scope(base: str | None, cwd: Path = PROJECT_ROOT) -> tuple[str, list[str]]:
    """Return (combined_diff_text, changed_files) for the detection scope."""
    if base:
        # Branch vs base ref.
        diff = run_git("diff", "--stat", base, cwd=cwd)
        name_only = run_git("diff", "--name-only", base, cwd=cwd)
        full = run_git("diff", base, cwd=cwd)
    else:
        # Working tree: unstaged + staged.
        unstaged = run_git("diff", cwd=cwd)
        staged = run_git("diff", "--cached", cwd=cwd)
        full = unstaged + "\n" + staged
        diff = run_git("diff", "--stat", cwd=cwd) + "\n" + run_git("diff", "--cached", "--stat", cwd=cwd)
        name_only = run_git("diff", "--name-only", cwd=cwd) + "\n" + run_git("diff", "--cached", "--name-only", cwd=cwd)
    files = [f for f in name_only.splitlines() if f.strip()]
    return full, files


def count_changed_lines(diff_text: str) -> int:
    added = 0
    removed = 0
    for line in diff_text.splitlines():
        if line.startswith("+") and not line.startswith("+++"):
            added += 1
        elif line.startswith("-") and not line.startswith("---"):
            removed += 1
    return added + removed


def crates_touched(files: list[str]) -> set[str]:
    crates: set[str] = set()
    for f in files:
        parts = f.replace("\\", "/").split("/")
        if len(parts) >= 2 and parts[0] == "crates":
            crates.add(parts[1])
    return crates


def classify(files: list[str], diff_text: str) -> tuple[set[str], set[str]]:
    """Return (matched_rules, notes) as sets of strings."""
    rules: set[str] = set()
    notes: set[str] = set()

    changed_lines = count_changed_lines(diff_text)
    if changed_lines >= LINE_THRESHOLD:
        rules.add("M1")
        notes.add(f"M1: {changed_lines} changed lines >= {LINE_THRESHOLD}")

    crates = crates_touched(files)
    if len(crates) > 2:
        rules.add("M2")
        notes.add(f"M2: {len(crates)} crates touched ({sorted(crates)})")

    crates_root = PROJECT_ROOT / "crates"
    for f in files:
        norm = f.replace("\\", "/")
        if norm.endswith("Cargo.toml"):
            rules.add("M6")
            notes.add(f"M6: dependency manifest changed ({norm})")

    # Frozen contract (M3) / security (M4) / hot path (M5) keyword scan.
    # Scan ONLY added lines (leading '+', excluding '+++') so context lines in
    # the diff do not cause false positives.
    added_lines = [
        line[1:].lower()
        for line in diff_text.splitlines()
        if line.startswith("+") and not line.startswith("+++")
    ]
    added_body = "\n".join(added_lines)
    for marker in CONTRACT_MARKERS:
        if marker.lower() in added_body:
            rules.add("M3")
            notes.add(f"M3: contract marker '{marker}' added")
            break
    for marker in SECURITY_MARKERS:
        if marker.lower() in added_body:
            rules.add("M4")
            notes.add(f"M4: security marker '{marker}' added")
            break
    for marker in HOT_PATH_MARKERS:
        if marker.lower() in added_body:
            rules.add("M5")
            notes.add(f"M5: hot-path marker '{marker}' added")
            break

    # M7: unsafe / transmute / raw pointer / static mut. Require a real Rust
    # unsafe construct (not the CSP keyword `unsafe-inline`).
    m7_hits = (
        "unsafe {",
        "unsafe fn",
        "unsafe impl",
        "unsafe trait",
        "transmute",
        "static mut",
        "as *const",
        "as *mut",
    )
    if any(t in added_body for t in m7_hits):
        rules.add("M7")
        notes.add("M7: unsafe/raw-pointer/static-mut construct added")

    # M8: AdminMode / admin auth chain / async semantics.
    if "adminmode" in added_body or "require_admin_access" in added_body or "require_admin_csrf" in added_body:
        rules.add("M8")
        notes.add("M8: AdminMode or admin auth chain change")

    return rules, notes


def recommended_reviewers(rules: set[str]) -> list[str]:
    if "M3" in rules or "M4" in rules:
        return ["glm-5.3", "kimi-k3", "astra"]
    return ["glm-5.3", "kimi-k3"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__file__)
    parser.add_argument("--base", default=None, help="Compare against a base git ref")
    parser.add_argument("--json", action="store_true", help="Emit JSON only")
    args = parser.parse_args()

    try:
        diff_text, files = diff_for_scope(args.base)
    except RuntimeError as error:
        if args.json:
            print(json.dumps({"gate": "error", "reason": str(error)}))
        else:
            print(f"error: {error}", file=sys.stderr)
        return 2

    rules, notes = classify(files, diff_text)
    fired = bool(rules)
    reviewers = recommended_reviewers(rules) if fired else []

    if args.json:
        print(json.dumps({
            "gate": "GATE_FIRES" if fired else "GATE_PASSES",
            "rules": sorted(rules),
            "notes": sorted(notes),
            "reviewers": reviewers,
            "changed_files": files,
        }, ensure_ascii=False, indent=2))
    else:
        print(f"gate: {'GATE_FIRES' if fired else 'GATE_PASSES'}")
        for note in sorted(notes):
            print(f"  {note}")
        if fired:
            print(f"  required high-tier reviewers: {', '.join(reviewers)}")
            print("  holistic review evidence REQUIRED before merge (ADR-007)")
        else:
            print("  below all material thresholds; per-diff review only")

    return 0 if not fired else 1


if __name__ == "__main__":
    sys.exit(main())
