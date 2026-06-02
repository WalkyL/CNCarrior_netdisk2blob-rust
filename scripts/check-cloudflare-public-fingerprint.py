#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import hashlib
import json
import re
import urllib.request
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PUBLIC_DIR = ROOT / "public" / "cloudflare"
DEFAULT_OUT = ROOT / "target" / "cloudflare-public-fingerprint"
PROVENANCE_MD = ROOT / "PROVENANCE.md"
REQUIRED_FINGERPRINT_FILES = [
    "index.html",
    "manifest.json",
    ".well-known/ccbg-provenance.json",
    "_headers",
    "assets/app.js",
    "assets/app.js.map",
    "release-notes/v0.1.1-public.md",
    "README.md",
]
FORBIDDEN_SUFFIXES = {
    ".rs",
    ".rlib",
    ".db",
    ".sqlite",
    ".sqlite3",
    ".tar",
    ".gz",
    ".zip",
    ".pem",
    ".key",
    ".env",
}
FORBIDDEN_NAME_PARTS = ["token", "cookie", "refresh", "secret", "credential"]


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def public_files() -> list[Path]:
    return sorted(path for path in PUBLIC_DIR.rglob("*") if path.is_file())


def load_provenance() -> dict:
    return json.loads((PUBLIC_DIR / ".well-known" / "ccbg-provenance.json").read_text(encoding="utf-8"))


def root_provenance_pair() -> tuple[str, str]:
    text = PROVENANCE_MD.read_text(encoding="utf-8", errors="replace")
    fingerprint_match = re.search(r"ccbg-[0-9][^\s`]+", text)
    sha_match = re.search(r"\b[a-f0-9]{64}\b", text)
    if not fingerprint_match or not sha_match:
        raise SystemExit("failed to parse fingerprint and SHA-256 from PROVENANCE.md")
    return fingerprint_match.group(0), sha_match.group(0)


def validate_public_boundary(files: list[Path]) -> None:
    for path in files:
        relative = path.relative_to(PUBLIC_DIR).as_posix()
        lowered = relative.lower()
        if path.suffix.lower() in FORBIDDEN_SUFFIXES:
            raise SystemExit(f"forbidden private/core-like artifact in public Cloudflare directory: {relative}")
        for part in FORBIDDEN_NAME_PARTS:
            if part in lowered:
                raise SystemExit(f"forbidden sensitive filename in public Cloudflare directory: {relative}")


def validate_fingerprint(fingerprint: str, fingerprint_sha256: str) -> None:
    root_fingerprint, root_sha256 = root_provenance_pair()
    if fingerprint != root_fingerprint:
        raise SystemExit("public fingerprint does not match PROVENANCE.md")
    if fingerprint_sha256 != root_sha256:
        raise SystemExit("public fingerprint SHA does not match PROVENANCE.md")
    for relative in REQUIRED_FINGERPRINT_FILES:
        path = PUBLIC_DIR / relative
        if not path.is_file():
            raise SystemExit(f"required public fingerprint file missing: {relative}")
        text = path.read_text(encoding="utf-8", errors="replace")
        if fingerprint not in text:
            raise SystemExit(f"release fingerprint missing from {relative}")
    provenance = load_provenance()
    if provenance.get("release_fingerprint") != fingerprint:
        raise SystemExit("provenance endpoint release_fingerprint mismatch")
    if provenance.get("fingerprint_sha256") != fingerprint_sha256:
        raise SystemExit("provenance endpoint fingerprint_sha256 mismatch")


def build_manifest(files: list[Path], fingerprint: str, fingerprint_sha256: str) -> dict:
    entries = []
    for path in files:
        relative = path.relative_to(PUBLIC_DIR).as_posix()
        entries.append(
            {
                "path": relative,
                "size_bytes": path.stat().st_size,
                "sha256": sha256_file(path),
            }
        )
    return {
        "schema_version": 1,
        "root": "public/cloudflare",
        "release_fingerprint": fingerprint,
        "fingerprint_sha256": fingerprint_sha256,
        "file_count": len(entries),
        "files": entries,
        "manifest_sha256": sha256_bytes(
            json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()
        ),
    }


def remote_bytes(base_url: str, relative: str) -> bytes:
    url = base_url.rstrip("/") + "/" + relative
    with urllib.request.urlopen(url, timeout=15) as response:
        return response.read()


def validate_remote(base_url: str, manifest: dict) -> None:
    for entry in manifest["files"]:
        remote_hash = sha256_bytes(remote_bytes(base_url, entry["path"]))
        if remote_hash != entry["sha256"]:
            raise SystemExit(f"remote hash mismatch for {entry['path']}: {remote_hash} != {entry['sha256']}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate and validate Cloudflare public fingerprint manifest.")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT), help="output directory")
    parser.add_argument("--deployed-base-url", default=None, help="optional deployed Cloudflare base URL to compare")
    args = parser.parse_args()
    provenance = load_provenance()
    fingerprint = str(provenance["release_fingerprint"])
    fingerprint_sha256 = str(provenance["fingerprint_sha256"])
    files = public_files()
    validate_public_boundary(files)
    validate_fingerprint(fingerprint, fingerprint_sha256)
    manifest = build_manifest(files, fingerprint, fingerprint_sha256)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    output = out_dir / "public-cloudflare-fingerprint-manifest.json"
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if args.deployed_base_url:
        validate_remote(args.deployed_base_url, manifest)
    print(output)
    print(manifest["manifest_sha256"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
