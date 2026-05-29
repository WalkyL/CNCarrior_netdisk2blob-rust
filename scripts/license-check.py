#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

"""Repository license, NOTICE, SPDX, and dependency metadata checks."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path


COMMERCIAL_LICENSE = "LicenseRef-CCBG-Commercial"
PUBLIC_LICENSE = "LicenseRef-CCBG-Public-Materials"
COPYRIGHT = "Copyright (c) 2026 walky"

REQUIRED_LEGAL_FILES = {
    "LICENSE": COMMERCIAL_LICENSE,
    "COMMERCIAL-LICENSE.md": COMMERCIAL_LICENSE,
    "NOTICE": COMMERCIAL_LICENSE,
    "PROVENANCE.md": COMMERCIAL_LICENSE,
    "TRADEMARKS.md": COMMERCIAL_LICENSE,
    "PUBLIC-MATERIALS-LICENSE.md": PUBLIC_LICENSE,
}

SOURCE_EXTENSIONS = {
    ".c",
    ".h",
    ".rs",
    ".py",
    ".sh",
    ".ps1",
    ".js",
    ".css",
    ".html",
}

SKIP_DIRS = {
    ".git",
    "target",
    "__pycache__",
    ".pytest_cache",
}


@dataclass
class Finding:
    path: str
    message: str

    def format(self) -> str:
        return f"{self.path}: {self.message}"


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def rel(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def spdx_id_for_path(path: Path) -> str:
    parts = path.as_posix().split("/")
    if len(parts) >= 2 and parts[0] == "public" and parts[1] == "cloudflare":
        return PUBLIC_LICENSE
    return COMMERCIAL_LICENSE


def first_text_window(path: Path, max_bytes: int = 8192) -> str:
    with path.open("rb") as handle:
        return handle.read(max_bytes).decode("utf-8", errors="replace")


def has_expected_spdx(path: Path, expected: str) -> bool:
    return f"SPDX-License-Identifier: {expected}" in first_text_window(path)


def iter_repo_files(root: Path):
    for current_root, dirnames, filenames in os.walk(root):
        dirnames[:] = [
            dirname for dirname in dirnames if dirname not in SKIP_DIRS
        ]
        current = Path(current_root)
        for filename in filenames:
            yield current / filename


def check_required_legal_files(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for relative, expected_license in REQUIRED_LEGAL_FILES.items():
        path = root / relative
        if not path.exists():
            findings.append(Finding(relative, "required legal/provenance file is missing"))
            continue
        text = read_text(path)
        if f"SPDX-License-Identifier: {expected_license}" not in text:
            findings.append(Finding(relative, f"missing SPDX identifier {expected_license}"))
        if COPYRIGHT not in text:
            findings.append(Finding(relative, f"missing {COPYRIGHT}"))

    license_text = read_text(root / "LICENSE") if (root / "LICENSE").exists() else ""
    if "COMMERCIAL-LICENSE.md" not in license_text:
        findings.append(Finding("LICENSE", "does not point to COMMERCIAL-LICENSE.md"))
    if "not distributed under the MIT License" not in license_text:
        findings.append(Finding("LICENSE", "does not explicitly reject repository-wide MIT"))

    notice_text = read_text(root / "NOTICE") if (root / "NOTICE").exists() else ""
    for required in [
        "Carrier Cloud Blob Gateway",
        "carrier-cloud-blob-gateway",
        "Release fingerprint:",
        "COMMERCIAL-LICENSE.md",
        "PUBLIC-MATERIALS-LICENSE.md",
    ]:
        if required not in notice_text:
            findings.append(Finding("NOTICE", f"missing required notice text: {required}"))
    return findings


def check_source_spdx(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in iter_repo_files(root):
        relative = rel(root, path)
        if path.suffix not in SOURCE_EXTENSIONS:
            continue
        expected = spdx_id_for_path(Path(relative))
        if not has_expected_spdx(path, expected):
            findings.append(Finding(relative, f"missing SPDX identifier {expected}"))
        if path.suffix != ".html" and COPYRIGHT not in first_text_window(path):
            findings.append(Finding(relative, f"missing {COPYRIGHT}"))
    return findings


def check_public_materials(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    public_root = root / "public" / "cloudflare"
    if not public_root.exists():
        return findings
    for path in iter_repo_files(public_root):
        relative = rel(root, path)
        if path.name == "app.js.map":
            text = first_text_window(path)
            if PUBLIC_LICENSE not in text or "release_fingerprint=" not in text:
                findings.append(Finding(relative, "source map lacks public license/fingerprint"))
            continue
        if path.suffix == ".json":
            text = read_text(path)
            if PUBLIC_LICENSE not in text:
                findings.append(Finding(relative, f"missing {PUBLIC_LICENSE}"))
            if "release_fingerprint" not in text:
                findings.append(Finding(relative, "missing release_fingerprint"))
    return findings


def check_cargo_manifests(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in iter_repo_files(root):
        if path.name != "Cargo.toml":
            continue
        relative = rel(root, path)
        text = read_text(path)
        license_lines = [
            line.strip()
            for line in text.splitlines()
            if line.strip().startswith("license")
        ]
        if any(line.startswith("license =") for line in license_lines):
            findings.append(Finding(relative, "uses inline license; use license-file instead"))
        if "MIT" in text:
            findings.append(Finding(relative, "mentions MIT in Cargo manifest"))
        if "license-file" not in text:
            findings.append(Finding(relative, "missing license-file declaration"))
    root_manifest = read_text(root / "Cargo.toml") if (root / "Cargo.toml").exists() else ""
    if 'license-file = "COMMERCIAL-LICENSE.md"' not in root_manifest:
        findings.append(Finding("Cargo.toml", "workspace license-file must be COMMERCIAL-LICENSE.md"))
    return findings


def check_dependency_license_metadata(root: Path) -> list[Finding]:
    try:
        completed = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=root,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except FileNotFoundError:
        return [Finding("cargo metadata", "cargo executable not found")]
    except subprocess.CalledProcessError as error:
        return [
            Finding(
                "cargo metadata",
                f"failed with exit code {error.returncode}: {error.stderr.strip()}",
            )
        ]

    metadata = json.loads(completed.stdout)
    findings: list[Finding] = []
    for package in metadata.get("packages", []):
        source = package.get("source")
        if not source:
            continue
        if package.get("license") or package.get("license_file"):
            continue
        name = package.get("name", "<unknown>")
        version = package.get("version", "<unknown>")
        findings.append(
            Finding(
                f"dependency:{name}@{version}",
                "registry dependency lacks license metadata",
            )
        )
    return findings


def run_checks(root: Path, *, cargo_metadata: bool) -> list[Finding]:
    findings: list[Finding] = []
    findings.extend(check_required_legal_files(root))
    findings.extend(check_source_spdx(root))
    findings.extend(check_public_materials(root))
    findings.extend(check_cargo_manifests(root))
    if cargo_metadata:
        findings.extend(check_dependency_license_metadata(root))
    return findings


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def minimal_legal_fixture(root: Path) -> None:
    for relative, license_id in REQUIRED_LEGAL_FILES.items():
        body = f"SPDX-License-Identifier: {license_id}\n\n{COPYRIGHT}\n"
        if relative == "LICENSE":
            body += "This repository is not distributed under the MIT License.\nSee COMMERCIAL-LICENSE.md.\n"
        if relative == "NOTICE":
            body += (
                "Carrier Cloud Blob Gateway\n"
                "carrier-cloud-blob-gateway\n"
                "Release fingerprint: test\n"
                "COMMERCIAL-LICENSE.md\n"
                "PUBLIC-MATERIALS-LICENSE.md\n"
            )
        write(root / relative, body)
    write(
        root / "Cargo.toml",
        '[workspace]\nmembers = ["crates/demo"]\n\n[workspace.package]\nlicense-file = "COMMERCIAL-LICENSE.md"\n',
    )
    write(
        root / "crates" / "demo" / "Cargo.toml",
        '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2024"\nlicense-file.workspace = true\n',
    )
    write(
        root / "crates" / "demo" / "src" / "lib.rs",
        f"// SPDX-License-Identifier: {COMMERCIAL_LICENSE}\n// {COPYRIGHT}\n",
    )
    write(
        root / "public" / "cloudflare" / "index.html",
        f"<!doctype html>\n<!-- SPDX-License-Identifier: {PUBLIC_LICENSE} -->\n",
    )
    write(
        root / "public" / "cloudflare" / "manifest.json",
        json.dumps({"license_id": PUBLIC_LICENSE, "release_fingerprint": "test"}),
    )


def run_self_test() -> int:
    with tempfile.TemporaryDirectory(prefix="ccbg-license-check-") as raw:
        fixture = Path(raw)
        minimal_legal_fixture(fixture)
        findings = run_checks(fixture, cargo_metadata=False)
        if findings:
            print("self-test passing fixture unexpectedly failed:", file=sys.stderr)
            for finding in findings:
                print(finding.format(), file=sys.stderr)
            return 1

        (fixture / "crates" / "demo" / "src" / "lib.rs").write_text(
            "// missing SPDX\n", encoding="utf-8"
        )
        findings = run_checks(fixture, cargo_metadata=False)
        if not any("missing SPDX" in finding.message for finding in findings):
            print("self-test did not catch missing SPDX", file=sys.stderr)
            return 1

        minimal_legal_fixture(fixture)
        (fixture / "crates" / "demo" / "Cargo.toml").write_text(
            '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2024"\nlicense = "MIT"\n',
            encoding="utf-8",
        )
        findings = run_checks(fixture, cargo_metadata=False)
        if not any("inline license" in finding.message or "MIT" in finding.message for finding in findings):
            print("self-test did not catch inline MIT Cargo license", file=sys.stderr)
            return 1

    print("license-check self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run built-in negative/positive fixtures")
    parser.add_argument(
        "--skip-cargo-metadata",
        action="store_true",
        help="skip registry dependency license metadata checks",
    )
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()

    root = repo_root()
    findings = run_checks(root, cargo_metadata=not args.skip_cargo_metadata)
    if findings:
        print("license-check failed:", file=sys.stderr)
        for finding in findings:
            print(f"  - {finding.format()}", file=sys.stderr)
        return 1
    print("license-check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
