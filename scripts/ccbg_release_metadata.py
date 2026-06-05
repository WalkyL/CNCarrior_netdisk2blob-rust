#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
WORKSPACE_CARGO = ROOT / "Cargo.toml"
GATEWAY_MAIN = ROOT / "crates" / "gatewayd" / "src" / "main.rs"


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def workspace_version() -> str:
    text = _read_text(WORKSPACE_CARGO)
    match = re.search(r'(?m)^version = "([^"]+)"\s*$', text)
    if not match:
        raise SystemExit("failed to locate workspace version in Cargo.toml")
    return match.group(1)


def gateway_release_constants() -> dict[str, str]:
    text = _read_text(GATEWAY_MAIN)
    keys = {
        "release_date": "CCBG_RELEASE_DATE",
        "release_channel": "CCBG_RELEASE_CHANNEL",
        "canonical_repo": "CCBG_CANONICAL_REPO",
        "fingerprint": "DEFAULT_RELEASE_FINGERPRINT",
        "fingerprint_sha256": "DEFAULT_RELEASE_FINGERPRINT_SHA256",
    }
    result: dict[str, str] = {}
    for out_key, const_name in keys.items():
        match = re.search(
            rf'const {re.escape(const_name)}: &str =\s*"([^"]+)";',
            text,
            re.MULTILINE,
        )
        if not match:
            raise SystemExit(f"failed to locate {const_name} in crates/gatewayd/src/main.rs")
        result[out_key] = match.group(1)
    return result


def public_materials_metadata() -> dict[str, str]:
    constants = gateway_release_constants()
    version = workspace_version()
    return {
        "service": "carrier-cloud-blob-gateway-public",
        "project": "Carrier Cloud Blob Gateway",
        "short_name": "CCBG",
        "version": version,
        "release_channel": "public-materials",
        "release_date": constants["release_date"],
        "release_fingerprint": constants["fingerprint"],
        "fingerprint_sha256": constants["fingerprint_sha256"],
        "canonical_repo": constants["canonical_repo"],
        "copyright": "Copyright (c) 2026 walky",
        "license_id": "LicenseRef-CCBG-Public-Materials",
        "core_license_id": "LicenseRef-CCBG-Commercial",
    }


def public_materials_seed_text() -> str:
    metadata = public_materials_metadata()
    return (
        "carrier-cloud-blob-gateway|2026|walky|"
        f"v{metadata['version']}|commercial-core|personal-source-review-v1"
    )


def public_materials_seed_sha256() -> str:
    return hashlib.sha256(public_materials_seed_text().encode("utf-8")).hexdigest()


def public_materials_provenance_payload() -> dict[str, object]:
    metadata = public_materials_metadata()
    payload: dict[str, object] = dict(metadata)
    payload["personal_source_review"] = {
        "eligible_after_days": 90,
        "scope": "named individual, non-commercial personal source review",
        "commercial_use_allowed": False,
        "redistribution_allowed": False,
        "hosted_service_allowed": False,
    }
    return payload


def _main() -> int:
    print(json.dumps(public_materials_provenance_payload(), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
