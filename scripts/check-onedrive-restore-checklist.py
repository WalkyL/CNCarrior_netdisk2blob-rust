#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKLIST = ROOT / "docs" / "onedrive-parking-restore-checklist.md"

REQUIRED_SECTIONS = [
    "## 1. Product Gate",
    "## 2. Configuration Gate",
    "## 3. Authentication Gate",
    "## 4. Provider Probe Gate",
    "## 5. Replication And Fallback Gate",
    "## 6. Observability Gate",
    "## 7. Regression Commands",
    "## 8. Rollback",
    "## Acceptance",
]

REQUIRED_TERMS = [
    "CCBG_ONEDRIVE_ENABLED=false",
    "CCBG_ONEDRIVE_REPLICATION_ENABLED=false",
    "CCBG_SYNC_TARGETS",
    "CCBG_FALLBACK_READ_ORDER",
    "OAuth",
    "provider-probes/onedrive.json",
    "replication",
    "fallback",
    "metrics",
    "alerts",
    "rollback",
    "python3 scripts/check-onedrive-parking.py",
    "cargo test -p provider-onedrive",
]


def main() -> int:
    if not CHECKLIST.is_file():
        raise SystemExit("OneDrive restore checklist is missing")
    text = CHECKLIST.read_text(encoding="utf-8")
    for section in REQUIRED_SECTIONS:
        if section not in text:
            raise SystemExit(f"missing checklist section: {section}")
    lowered = text.lower()
    for term in REQUIRED_TERMS:
        if term.lower() not in lowered:
            raise SystemExit(f"missing checklist term: {term}")
    if "off by default" not in lowered and "默认" not in text:
        raise SystemExit("checklist must explicitly say OneDrive remains off by default")
    print("OneDrive restore checklist verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
