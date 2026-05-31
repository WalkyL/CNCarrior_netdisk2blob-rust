#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import platform
import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "target" / "release-provenance"
PROVENANCE_MD = ROOT / "PROVENANCE.md"


def now_iso(source_date_epoch: int | None) -> str:
    if source_date_epoch is None:
        return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()
    return dt.datetime.fromtimestamp(source_date_epoch, tz=dt.timezone.utc).replace(microsecond=0).isoformat()


def git_commit() -> str | None:
    completed = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return completed.stdout.strip()


def git_dirty() -> bool | None:
    completed = subprocess.run(
        ["git", "-C", str(ROOT), "status", "--porcelain"],
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return bool(completed.stdout.strip())


def current_fingerprint() -> str:
    text = PROVENANCE_MD.read_text(encoding="utf-8", errors="replace")
    match = re.search(r"ccbg-[0-9][^\s`]+", text)
    if not match:
        raise SystemExit("failed to locate current release fingerprint in PROVENANCE.md")
    return match.group(0)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_entry(path: Path) -> dict:
    resolved = path if path.is_absolute() else ROOT / path
    if not resolved.is_file():
        raise SystemExit(f"artifact does not exist: {path}")
    return {
        "path": resolved.relative_to(ROOT).as_posix() if resolved.is_relative_to(ROOT) else str(resolved),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }


def github_context() -> dict:
    keys = [
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_SHA",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_WORKFLOW",
        "GITHUB_ACTOR",
    ]
    return {key.lower(): os.environ.get(key) for key in keys if os.environ.get(key)}


def build_payload(args: argparse.Namespace) -> dict:
    source_date_epoch = args.source_date_epoch
    artifacts = [artifact_entry(Path(item)) for item in args.artifact]
    core = {
        "schema_version": 1,
        "release_name": args.release_name,
        "tag": args.tag,
        "release_fingerprint": args.fingerprint or current_fingerprint(),
        "generated_at": now_iso(source_date_epoch),
        "canonical_repo": "https://github.com/WalkyL/CNCarrior_netdisk2blob-rust",
        "git_commit": git_commit(),
        "git_dirty": git_dirty(),
        "github": github_context(),
        "build_environment": {
            "python": platform.python_version(),
            "platform": platform.platform(),
        },
        "build_steps": args.build_step,
        "artifacts": artifacts,
    }
    fingerprint_input = dict(core)
    fingerprint_input.pop("generated_at", None)
    core["provenance_sha256"] = hashlib.sha256(
        json.dumps(fingerprint_input, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return core


def write_markdown(payload: dict, out_path: Path) -> None:
    lines = [
        "# CCBG Release Provenance",
        "",
        f"- Release: `{payload['release_name']}`",
        f"- Tag: `{payload['tag']}`",
        f"- Fingerprint: `{payload['release_fingerprint']}`",
        f"- Git commit: `{payload['git_commit']}`",
        f"- Git dirty: `{payload['git_dirty']}`",
        f"- Provenance SHA256: `{payload['provenance_sha256']}`",
        "",
        "## Build Steps",
        "",
    ]
    for step in payload["build_steps"]:
        lines.append(f"- `{step}`")
    lines.extend(["", "## Artifacts", ""])
    for artifact in payload["artifacts"]:
        lines.append(f"- `{artifact['path']}` `{artifact['sha256']}` `{artifact['size_bytes']} bytes`")
    lines.append("")
    out_path.write_text("\n".join(lines), encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate release provenance records.")
    parser.add_argument("--release-name", default="development", help="release name")
    parser.add_argument("--tag", default=os.environ.get("GITHUB_REF_NAME", "untagged"), help="release tag")
    parser.add_argument("--fingerprint", default=None, help="release fingerprint override")
    parser.add_argument("--artifact", action="append", default=[], help="artifact file to hash")
    parser.add_argument("--build-step", action="append", default=[], help="build command or step name")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT), help="output directory")
    parser.add_argument(
        "--source-date-epoch",
        type=int,
        default=int(os.environ["SOURCE_DATE_EPOCH"]) if os.environ.get("SOURCE_DATE_EPOCH") else None,
        help="fixed generated_at timestamp",
    )
    args = parser.parse_args()
    if not args.artifact:
        parser.error("at least one --artifact is required")
    if not args.build_step:
        parser.error("at least one --build-step is required")
    payload = build_payload(args)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    json_path = out_dir / "release-provenance.json"
    md_path = out_dir / "release-provenance.md"
    json_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    write_markdown(payload, md_path)
    print(json_path)
    print(md_path)
    print(payload["provenance_sha256"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
