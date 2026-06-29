#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import urllib.request
from pathlib import Path

from ccbg_release_metadata import public_materials_provenance_payload


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "target" / "cloudflare-public-fingerprint"
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
NON_DEPLOYED_REMOTE_FILES = {"_headers"}


def resolve_bash() -> str:
    candidates = [
        Path(r"D:\Git\bin\bash.exe"),
        Path(r"D:\Apps\Git\bin\bash.exe"),
        Path(r"C:\Program Files\Git\bin\bash.exe"),
        Path(r"C:\Users\Walky\AppData\Local\Programs\Git\bin\bash.exe"),
        Path(r"C:\Users\walky\AppData\Local\Programs\Git\bin\bash.exe"),
    ]
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    resolved = shutil.which("bash")
    if resolved:
        lowered = os.path.normcase(os.path.normpath(resolved))
        forbidden = {
            os.path.normcase(os.path.normpath(r"C:\Windows\System32\bash.exe")),
            os.path.normcase(os.path.normpath(r"C:\Users\walky\AppData\Local\Microsoft\WindowsApps\bash.exe")),
        }
        if lowered not in forbidden:
            return resolved
    raise SystemExit("missing non-WSL bash; install Git Bash on the build host or add it to PATH")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def stage_public_dir(out_dir: Path) -> Path:
    staged = out_dir / "staged-public"
    bash_bin = resolve_bash()
    subprocess.run(
        [bash_bin, "scripts/stage-cloudflare-public-assets.sh", str(staged)],
        cwd=ROOT,
        check=True,
    )
    return staged


def public_files(public_dir: Path) -> list[Path]:
    return sorted(path for path in public_dir.rglob("*") if path.is_file())


def load_provenance(public_dir: Path) -> dict:
    return json.loads((public_dir / ".well-known" / "ccbg-provenance.json").read_text(encoding="utf-8"))


def required_fingerprint_files() -> list[str]:
    version = public_materials_provenance_payload()["version"]
    return [
        "index.html",
        "manifest.json",
        ".well-known/ccbg-provenance.json",
        "_headers",
        "assets/app.js",
        "assets/app.js.map",
        "data/faq-catalog.json",
        "data/install-catalog.json",
        f"release-notes/v{version}-public.md",
        "README.md",
    ]
def validate_public_boundary(public_dir: Path, files: list[Path]) -> None:
    for path in files:
        relative = path.relative_to(public_dir).as_posix()
        lowered = relative.lower()
        if path.suffix.lower() in FORBIDDEN_SUFFIXES:
            raise SystemExit(f"forbidden private/core-like artifact in public Cloudflare directory: {relative}")
        for part in FORBIDDEN_NAME_PARTS:
            if part in lowered:
                raise SystemExit(f"forbidden sensitive filename in public Cloudflare directory: {relative}")


def validate_fingerprint(public_dir: Path, fingerprint: str, fingerprint_sha256: str) -> None:
    expected = public_materials_provenance_payload()
    if fingerprint != expected["release_fingerprint"]:
        raise SystemExit("public fingerprint does not match current release metadata")
    if fingerprint_sha256 != expected["fingerprint_sha256"]:
        raise SystemExit("public fingerprint SHA does not match current release metadata")
    for relative in required_fingerprint_files():
        path = public_dir / relative
        if not path.is_file():
            raise SystemExit(f"required public fingerprint file missing: {relative}")
        text = path.read_text(encoding="utf-8", errors="replace")
        if fingerprint not in text:
            raise SystemExit(f"release fingerprint missing from {relative}")
    provenance = load_provenance(public_dir)
    if provenance.get("release_fingerprint") != fingerprint:
        raise SystemExit("provenance endpoint release_fingerprint mismatch")
    if provenance.get("fingerprint_sha256") != fingerprint_sha256:
        raise SystemExit("provenance endpoint fingerprint_sha256 mismatch")
    if provenance.get("version") != expected["version"]:
        raise SystemExit("provenance endpoint version mismatch")
    headers_text = (public_dir / "_headers").read_text(encoding="utf-8", errors="replace")
    if f"X-CCBG-Version: {expected['version']}" not in headers_text:
        raise SystemExit("_headers is missing the current X-CCBG-Version header")


def build_manifest(public_dir: Path, files: list[Path], fingerprint: str, fingerprint_sha256: str) -> dict:
    entries = []
    for path in files:
        relative = path.relative_to(public_dir).as_posix()
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
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": "curl/8.8.0",
            "Accept": "*/*",
        },
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        return response.read()


def validate_remote(base_url: str, manifest: dict) -> None:
    for entry in manifest["files"]:
        if entry["path"] in NON_DEPLOYED_REMOTE_FILES:
            continue
        remote_hash = sha256_bytes(remote_bytes(base_url, entry["path"]))
        if remote_hash != entry["sha256"]:
            raise SystemExit(f"remote hash mismatch for {entry['path']}: {remote_hash} != {entry['sha256']}")


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate and validate Cloudflare public fingerprint manifest.")
    parser.add_argument("--out-dir", default=str(DEFAULT_OUT), help="output directory")
    parser.add_argument("--public-dir", default=None, help="optional rendered public asset directory to validate")
    parser.add_argument("--deployed-base-url", default=None, help="optional deployed Cloudflare base URL to compare")
    args = parser.parse_args()
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    public_dir = Path(args.public_dir) if args.public_dir else stage_public_dir(out_dir)
    provenance = load_provenance(public_dir)
    fingerprint = str(provenance["release_fingerprint"])
    fingerprint_sha256 = str(provenance["fingerprint_sha256"])
    files = public_files(public_dir)
    validate_public_boundary(public_dir, files)
    validate_fingerprint(public_dir, fingerprint, fingerprint_sha256)
    manifest = build_manifest(public_dir, files, fingerprint, fingerprint_sha256)
    output = out_dir / "public-cloudflare-fingerprint-manifest.json"
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    if args.deployed_base_url:
        validate_remote(args.deployed_base_url, manifest)
    print(output)
    print(manifest["manifest_sha256"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
