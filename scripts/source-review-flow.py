#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
OUT_DIR = ROOT / "target" / "source-review-flow"
MIN_USE_DAYS = 90
ALLOWED_SCOPE = {
    "license_files",
    "selected_source",
    "build_metadata",
    "redacted_config_samples",
    "provenance_manifest",
}


def now_utc() -> dt.datetime:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0)


def parse_date(value: str) -> dt.date:
    return dt.date.fromisoformat(value)


def sample_request(today: dt.date) -> dict:
    start = today - dt.timedelta(days=120)
    return {
        "applicant_id": "simulated-personal-user",
        "applicant_kind": "individual",
        "contact": "user@example.invalid",
        "usage_start_date": start.isoformat(),
        "request_date": today.isoformat(),
        "declared_personal_use": True,
        "declared_non_commercial": True,
        "commercial_or_hosted_intent": False,
        "redistribution_intent": False,
        "employer_or_team_access": False,
        "abuse_history": False,
        "requested_scope": [
            "license_files",
            "selected_source",
            "build_metadata",
            "redacted_config_samples",
            "provenance_manifest",
        ],
    }


def request_fingerprint(request: dict) -> str:
    encoded = json.dumps(request, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def evaluate(request: dict, today: dt.date) -> tuple[str, list[str]]:
    reasons: list[str] = []
    if request.get("applicant_kind") != "individual":
        reasons.append("applicant must be an individual")
    if not request.get("declared_personal_use"):
        reasons.append("personal use declaration is required")
    if not request.get("declared_non_commercial"):
        reasons.append("non-commercial declaration is required")
    if request.get("commercial_or_hosted_intent"):
        reasons.append("commercial or hosted-service intent is not allowed")
    if request.get("redistribution_intent"):
        reasons.append("redistribution intent is not allowed")
    if request.get("employer_or_team_access"):
        reasons.append("employer/team access is not allowed")
    if request.get("abuse_history"):
        reasons.append("abuse history requires rejection or manual escalation")

    try:
        usage_start = parse_date(str(request.get("usage_start_date", "")))
    except ValueError:
        reasons.append("usage_start_date must be YYYY-MM-DD")
    else:
        use_days = (today - usage_start).days
        if use_days < MIN_USE_DAYS:
            reasons.append(f"requires at least {MIN_USE_DAYS} days of use, got {use_days}")

    requested_scope = set(request.get("requested_scope") or [])
    disallowed_scope = sorted(requested_scope - ALLOWED_SCOPE)
    if disallowed_scope:
        reasons.append("requested scope contains disallowed entries: " + ", ".join(disallowed_scope))

    return ("approved_for_manual_grant" if not reasons else "rejected_or_escalate", reasons)


def write_simulation() -> Path:
    timestamp = now_utc()
    today = timestamp.date()
    request = sample_request(today)
    decision, reasons = evaluate(request, today)
    grant_id = "psr-" + request_fingerprint(request)[:16]
    payload = {
        "schema_version": 1,
        "generated_at": timestamp.isoformat(),
        "grant_id": grant_id,
        "request_fingerprint_sha256": request_fingerprint(request),
        "decision": decision,
        "reasons": reasons,
        "minimum_use_days": MIN_USE_DAYS,
        "approved_scope": sorted(ALLOWED_SCOPE) if decision == "approved_for_manual_grant" else [],
        "source_exported": False,
        "next_step": "run RELEASE-002 package flow only after written approval",
    }
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    output = OUT_DIR / "simulated-personal-source-review-decision.json"
    output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate the personal source review process.")
    parser.add_argument("--simulate", action="store_true", help="run a deterministic fake applicant flow")
    args = parser.parse_args()
    if not args.simulate:
        parser.error("only --simulate is currently supported")
    output = write_simulation()
    print(f"source review flow simulation passed: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
