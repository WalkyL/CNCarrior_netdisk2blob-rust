#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

DEFAULT_ENV_FILES = [
    ROOT / "config" / "example.env",
    ROOT / "config" / "openwrt-lite.env",
    ROOT / "deploy" / "lxc" / "ccbg.env",
]

DOC_FILES = [
    ROOT / "README.md",
    ROOT / "docs" / "architecture.md",
    ROOT / "docs" / "auth-step-by-step.md",
    ROOT / "docs" / "router-deployment-guide.md",
    ROOT / "docs" / "openwrt-lite-deployment.md",
    ROOT / "docs" / "pve-lxc-deployment.md",
]

FORBIDDEN_DOC_PATTERNS = [
    r"默认建议始终包含\s+onedrive",
    r"OneDrive\s*默认备份",
    r"onedrive\s*只放在\s*`?CCBG_SYNC_TARGETS`?",
    r"OneDrive\s*仍建议保持.*异步备份",
    r"把\s*`?onedrive`?\s*放在\s*`?CCBG_FALLBACK_READ_ORDER`?",
    r"CCBG_SYNC_TARGETS=onedrive",
    r"CCBG_FALLBACK_READ_ORDER=onedrive",
    r"CCBG_ONEDRIVE_REPLICATION_ENABLED=true",
    r"default backup target.*onedrive",
]


def parse_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise SystemExit(f"{path.relative_to(ROOT)}:{line_number}: invalid env line")
        key, value = line.split("=", 1)
        values[key.strip()] = value.strip().strip("'\"")
    return values


def assert_not_contains_onedrive(path: Path, key: str, value: str) -> None:
    items = [item.strip().lower() for item in value.split(",") if item.strip()]
    if "onedrive" in items:
        raise SystemExit(f"{path.relative_to(ROOT)}: {key} must not contain onedrive by default")


def validate_env_files() -> None:
    for path in DEFAULT_ENV_FILES:
        if not path.is_file():
            raise SystemExit(f"default env file missing: {path.relative_to(ROOT)}")
        values = parse_env(path)
        for key in ("CCBG_ONEDRIVE_ENABLED", "CCBG_ONEDRIVE_REPLICATION_ENABLED"):
            if values.get(key) != "false":
                raise SystemExit(f"{path.relative_to(ROOT)}: {key} must default to false")
        for key in ("CCBG_SYNC_TARGETS", "CCBG_FALLBACK_READ_ORDER"):
            assert_not_contains_onedrive(path, key, values.get(key, ""))


def validate_docs() -> None:
    compiled = [re.compile(pattern, re.IGNORECASE | re.DOTALL) for pattern in FORBIDDEN_DOC_PATTERNS]
    for path in DOC_FILES:
        if not path.is_file():
            raise SystemExit(f"doc file missing: {path.relative_to(ROOT)}")
        text = path.read_text(encoding="utf-8", errors="replace")
        for pattern, regex in zip(FORBIDDEN_DOC_PATTERNS, compiled):
            if regex.search(text):
                raise SystemExit(f"{path.relative_to(ROOT)} contains forbidden OneDrive default wording: {pattern}")


def main() -> int:
    validate_env_files()
    validate_docs()
    print("OneDrive parking defaults verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
