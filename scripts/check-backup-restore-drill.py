#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DRILL_ROOT = ROOT / "target" / "backup-restore-drill"
REQUIRED_FILES = [
    "checkpoint/checkpoint-summary.json",
    "credential/credential-inventory.json",
    "wal/wal-records.json",
    "metadata/metadata-snapshot.json",
    "report/drill-input.json",
]
SAMPLE_FILES: dict[str, Any] = {
    "checkpoint/checkpoint-summary.json": {
        "checkpoint_lsn": 100,
        "replay_from_lsn": 101,
    },
    "credential/credential-inventory.json": {
        "entries": [
            {
                "provider": "stub",
                "credential_ref": "provider-credentials/stub.json",
                "contains_secret_material": False,
            }
        ]
    },
    "wal/wal-records.json": [
        {
            "lsn": 101,
            "phase": "committed",
            "operation": "metadata_sync",
            "object_ref": "s3://drill/example.txt",
        }
    ],
    "metadata/metadata-snapshot.json": {
        "logical_object_count": 1,
        "placement_count": 1,
        "pending_replication_jobs": 0,
    },
    "report/drill-input.json": {
        "drill_id": "ci-smoke",
        "checkpoint_backup_file": "ccbg-backup-ci-smoke.ccbgbak",
        "restore_target": "offline-ci",
        "operator": "ci",
    },
}


@dataclass
class Finding:
    code: str
    message: str


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def get_int(data: dict[str, Any], key: str, ctx: str) -> tuple[int | None, Finding | None]:
    value = data.get(key)
    if not isinstance(value, int):
        return None, Finding("invalid_int", f"{ctx}.{key} must be int")
    return value, None


def write_sample(drill_root: Path) -> None:
    for relative, payload in SAMPLE_FILES.items():
        path = drill_root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_report(out_json: Path, drill_root: Path, findings: list[Finding]) -> None:
    report = {
        "schema_version": 1,
        "passed": not findings,
        "drill_root": str(drill_root),
        "finding_count": len(findings),
        "findings": [{"code": item.code, "message": item.message} for item in findings],
    }
    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def check_required_files(drill_root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for relative in REQUIRED_FILES:
        path = drill_root / relative
        if not path.is_file():
            findings.append(Finding("missing_file", f"missing required file: {relative}"))
    return findings


def check_checkpoint_wal_consistency(
    checkpoint_summary: dict[str, Any], wal_records: list[dict[str, Any]]
) -> list[Finding]:
    findings: list[Finding] = []
    checkpoint_lsn, checkpoint_finding = get_int(checkpoint_summary, "checkpoint_lsn", "checkpoint")
    replay_from_lsn, replay_finding = get_int(checkpoint_summary, "replay_from_lsn", "checkpoint")
    for finding in [checkpoint_finding, replay_finding]:
        if finding is not None:
            findings.append(finding)
    if checkpoint_lsn is None or replay_from_lsn is None:
        return findings
    if replay_from_lsn != checkpoint_lsn + 1:
        findings.append(
            Finding(
                "invalid_replay_start",
                f"checkpoint.replay_from_lsn={replay_from_lsn} should equal checkpoint_lsn+1={checkpoint_lsn + 1}",
            )
        )

    if not wal_records:
        findings.append(Finding("empty_wal", "wal.wal-records.json must not be empty"))
        return findings

    committed_after_checkpoint = [
        record
        for record in wal_records
        if record.get("phase") == "committed" and isinstance(record.get("lsn"), int) and record["lsn"] > checkpoint_lsn
    ]
    if not committed_after_checkpoint:
        findings.append(
            Finding(
                "wal_gap",
                "no committed WAL record after checkpoint_lsn; cannot prove post-checkpoint replay path",
            )
        )
    return findings


def check_credential_inventory(credential_inventory: dict[str, Any]) -> list[Finding]:
    findings: list[Finding] = []
    entries = credential_inventory.get("entries")
    if not isinstance(entries, list) or not entries:
        findings.append(Finding("credential_entries_missing", "credential.entries must be a non-empty list"))
        return findings

    for entry in entries:
        if not isinstance(entry, dict):
            findings.append(Finding("credential_entry_invalid", "credential entry must be JSON object"))
            continue
        if not entry.get("provider"):
            findings.append(Finding("credential_provider_missing", "credential entry missing provider"))
        if not entry.get("credential_ref"):
            findings.append(Finding("credential_ref_missing", "credential entry missing credential_ref"))
        if entry.get("contains_secret_material") is True:
            findings.append(
                Finding(
                    "credential_secret_leak",
                    "credential inventory must not include secret material in drill artifact",
                )
            )
    return findings


def check_metadata(metadata_snapshot: dict[str, Any]) -> list[Finding]:
    findings: list[Finding] = []
    for key in ["logical_object_count", "placement_count", "pending_replication_jobs"]:
        if not isinstance(metadata_snapshot.get(key), int):
            findings.append(Finding("metadata_field_missing", f"metadata.{key} must be int"))
    return findings


def check_drill_input(drill_input: dict[str, Any]) -> list[Finding]:
    findings: list[Finding] = []
    for key in ["drill_id", "checkpoint_backup_file", "restore_target", "operator"]:
        value = drill_input.get(key)
        if not isinstance(value, str) or not value.strip():
            findings.append(Finding("drill_input_missing", f"report.drill-input.{key} must be non-empty string"))
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description="Offline backup/restore drill checker for OPS-005.")
    parser.add_argument("--drill-root", default=str(DEFAULT_DRILL_ROOT), help="drill artifact root directory")
    parser.add_argument(
        "--write-sample",
        action="store_true",
        help="write a minimal offline drill artifact before checking it",
    )
    parser.add_argument(
        "--out-json",
        default=None,
        help="optional path to write JSON report; defaults to <drill-root>/report/drill-check-result.json",
    )
    args = parser.parse_args()

    drill_root = Path(args.drill_root)
    if args.write_sample:
        write_sample(drill_root)

    out_json = Path(args.out_json) if args.out_json else drill_root / "report" / "drill-check-result.json"
    findings = check_required_files(drill_root)
    if findings:
        write_report(out_json, drill_root, findings)
        for finding in findings:
            print(f"{finding.code}: {finding.message}")
        print(out_json)
        return 1

    checkpoint_summary = load_json(drill_root / "checkpoint" / "checkpoint-summary.json")
    credential_inventory = load_json(drill_root / "credential" / "credential-inventory.json")
    wal_records = load_json(drill_root / "wal" / "wal-records.json")
    metadata_snapshot = load_json(drill_root / "metadata" / "metadata-snapshot.json")
    drill_input = load_json(drill_root / "report" / "drill-input.json")

    if not isinstance(checkpoint_summary, dict):
        raise SystemExit("checkpoint/checkpoint-summary.json must be object")
    if not isinstance(credential_inventory, dict):
        raise SystemExit("credential/credential-inventory.json must be object")
    if not isinstance(wal_records, list):
        raise SystemExit("wal/wal-records.json must be array")
    if not isinstance(metadata_snapshot, dict):
        raise SystemExit("metadata/metadata-snapshot.json must be object")
    if not isinstance(drill_input, dict):
        raise SystemExit("report/drill-input.json must be object")

    findings.extend(check_checkpoint_wal_consistency(checkpoint_summary, wal_records))
    findings.extend(check_credential_inventory(credential_inventory))
    findings.extend(check_metadata(metadata_snapshot))
    findings.extend(check_drill_input(drill_input))

    write_report(out_json, drill_root, findings)

    if not findings:
        print("backup/restore drill check passed")
        print(out_json)
        return 0

    for finding in findings:
        print(f"{finding.code}: {finding.message}")
    print(out_json)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
