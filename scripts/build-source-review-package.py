#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import platform
import subprocess
import tarfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = ROOT / "config" / "source-review-package.json"
DEFAULT_OUT = ROOT / "target" / "source-review-package"
MANIFEST_NAME = "SOURCE-REVIEW-MANIFEST.json"


def load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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


def is_excluded(relative: str, patterns: list[str]) -> bool:
    return any(fnmatch.fnmatch(relative, pattern) for pattern in patterns)


def expand_include(pattern: str) -> list[Path]:
    if pattern.endswith("/**"):
        base = ROOT / pattern[:-3]
        if not base.exists():
            return []
        return sorted(path for path in base.rglob("*") if path.is_file())
    matches = sorted(ROOT.glob(pattern))
    return [path for path in matches if path.is_file()]


def collect_files(config: dict) -> list[Path]:
    excludes = list(config.get("exclude") or [])
    seen: set[str] = set()
    files: list[Path] = []
    for pattern in config.get("include") or []:
        for path in expand_include(str(pattern)):
            relative = path.relative_to(ROOT).as_posix()
            if relative in seen or is_excluded(relative, excludes):
                continue
            seen.add(relative)
            files.append(path)
    return sorted(files, key=lambda item: item.relative_to(ROOT).as_posix())


def validate_decision(decision: dict) -> None:
    if decision.get("decision") != "approved_for_manual_grant":
        raise SystemExit("source review package requires an approved personal grant decision")
    approved_scope = set(decision.get("approved_scope") or [])
    required = {"license_files", "selected_source", "build_metadata", "provenance_manifest"}
    missing = sorted(required - approved_scope)
    if missing:
        raise SystemExit("approved grant scope is missing: " + ", ".join(missing))


def build_manifest(config: dict, decision: dict, files: list[Path], generated_at: int) -> dict:
    file_entries = []
    for path in files:
        relative = path.relative_to(ROOT).as_posix()
        file_entries.append(
            {
                "path": relative,
                "size_bytes": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
    fingerprint_input = {
        "schema_version": config.get("schema_version"),
        "package_name": config.get("package_name"),
        "grant_id": decision.get("grant_id"),
        "request_fingerprint_sha256": decision.get("request_fingerprint_sha256"),
        "git_commit": git_commit(),
        "files": file_entries,
    }
    package_fingerprint = hashlib.sha256(
        json.dumps(fingerprint_input, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return {
        "schema_version": 1,
        "package_name": config.get("package_name"),
        "generated_at_unix": generated_at,
        "grant_id": decision.get("grant_id"),
        "request_fingerprint_sha256": decision.get("request_fingerprint_sha256"),
        "git_commit": git_commit(),
        "build_environment": {
            "python": platform.python_version(),
            "platform": platform.platform(),
        },
        "source_exported": True,
        "file_count": len(file_entries),
        "files": file_entries,
        "package_fingerprint_sha256": package_fingerprint,
    }


def add_bytes(tar: tarfile.TarFile, arcname: str, data: bytes, mtime: int) -> None:
    info = tarfile.TarInfo(arcname)
    info.size = len(data)
    info.mode = 0o644
    info.mtime = mtime
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    import io

    tar.addfile(info, io.BytesIO(data))


def add_file(tar: tarfile.TarFile, path: Path, arcname: str, mtime: int) -> None:
    info = tar.gettarinfo(str(path), arcname=arcname)
    info.mode = 0o644
    info.mtime = mtime
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    with path.open("rb") as handle:
        tar.addfile(info, handle)


def build_package(args: argparse.Namespace) -> Path:
    config = load_json(Path(args.config))
    decision = load_json(Path(args.decision))
    validate_decision(decision)
    files = collect_files(config)
    if not files:
        raise SystemExit("source review package file set is empty")
    generated_at = int(args.source_date_epoch if args.source_date_epoch is not None else time.time())
    manifest = build_manifest(config, decision, files, generated_at)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    package_name = str(config.get("package_name") or "ccbg-personal-source-review")
    tar_path = out_dir / f"{package_name}.tar"
    root_name = package_name
    with tarfile.open(tar_path, "w") as tar:
        manifest_bytes = json.dumps(manifest, indent=2, sort_keys=True).encode() + b"\n"
        add_bytes(tar, f"{root_name}/{MANIFEST_NAME}", manifest_bytes, generated_at)
        for path in files:
            relative = path.relative_to(ROOT).as_posix()
            add_file(tar, path, f"{root_name}/{relative}", generated_at)
    sha_path = tar_path.with_suffix(tar_path.suffix + ".sha256")
    sha_path.write_text(f"{sha256_file(tar_path)}  {tar_path.name}\n", encoding="utf-8")
    manifest_path = out_dir / MANIFEST_NAME
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return tar_path


def main() -> int:
    parser = argparse.ArgumentParser(description="Build a personal source review package manifest.")
    parser.add_argument("--decision", required=True, help="approved source review decision JSON")
    parser.add_argument("--config", default=str(DEFAULT_CONFIG), help="source review package config")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT), help="output directory")
    parser.add_argument(
        "--source-date-epoch",
        type=int,
        default=int(os.environ.get("SOURCE_DATE_EPOCH", "0")),
        help="fixed timestamp for deterministic package output",
    )
    args = parser.parse_args()
    tar_path = build_package(args)
    print(tar_path)
    print(tar_path.with_suffix(tar_path.suffix + ".sha256"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
