#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import json
import sys
from pathlib import Path


ACTIVE_PROVIDERS = ("unicom", "telecom", "mobile")
PARKING_PROVIDERS = ("onedrive",)
ACTIVE_NATIVE_CAPABILITY_STATUSES = {"active", "stable"}


def json_path(path: Path, repo_root: Path) -> str:
    return str(path.relative_to(repo_root))


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as f:
        return json.load(f)


def schema_version(payload: dict) -> int | None:
    value = payload.get("schema_version")
    if isinstance(value, int) and value > 0:
        return value
    return None


def validate_common_catalog(path: Path, payload: dict, repo_root: Path, errors: list[str]) -> None:
    label = json_path(path, repo_root)
    if schema_version(payload) is None:
        errors.append(f"{label}: missing positive integer schema_version")
    provider = payload.get("provider")
    if not isinstance(provider, str) or not provider.strip():
        errors.append(f"{label}: missing provider")


def validate_probe_catalog(path: Path, payload: dict, repo_root: Path, errors: list[str]) -> None:
    label = json_path(path, repo_root)
    items = payload.get("probe_items")
    if not isinstance(items, list) or not items:
        errors.append(f"{label}: probe_items must be a non-empty array")
        return
    seen_ids = set()
    for item in items:
        item_id = item.get("id") if isinstance(item, dict) else None
        if not isinstance(item_id, str) or not item_id.strip():
            errors.append(f"{label}: probe item missing id")
            continue
        if item_id in seen_ids:
            errors.append(f"{label}: duplicate probe item id={item_id}")
        seen_ids.add(item_id)


def validate_capability_catalog(
    path: Path, payload: dict, repo_root: Path, errors: list[str]
) -> None:
    label = json_path(path, repo_root)
    capabilities = payload.get("capabilities")
    if not isinstance(capabilities, list) or not capabilities:
        errors.append(f"{label}: capabilities must be a non-empty array")
        return
    seen_ids = set()
    for capability in capabilities:
        capability_id = capability.get("id") if isinstance(capability, dict) else None
        if not isinstance(capability_id, str) or not capability_id.strip():
            errors.append(f"{label}: capability item missing id")
            continue
        if capability_id in seen_ids:
            errors.append(f"{label}: duplicate capability id={capability_id}")
        seen_ids.add(capability_id)
        has_http_shape = (
            isinstance(capability.get("method"), str)
            and bool(capability["method"].strip())
            and isinstance(capability.get("url"), str)
            and bool(capability["url"].strip())
        )
        has_dispatcher_shape = isinstance(
            capability.get("dispatcher_operation"), str
        ) and bool(capability["dispatcher_operation"].strip())
        if not has_http_shape and not has_dispatcher_shape:
            errors.append(
                f"{label}: capability {capability_id} must include method+url or dispatcher_operation"
            )


def validate_browser_flow_catalog(
    path: Path, payload: dict, repo_root: Path, errors: list[str]
) -> None:
    label = json_path(path, repo_root)
    surface = payload.get("surface")
    if not isinstance(surface, str) or not surface.strip():
        errors.append(f"{label}: missing surface")
    flows = payload.get("flows")
    if not isinstance(flows, list) or not flows:
        errors.append(f"{label}: flows must be a non-empty array")


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    probes_dir = repo_root / "config" / "provider-probes"
    capabilities_dir = repo_root / "config" / "provider-capabilities"
    browser_flows_dir = repo_root / "config" / "browser-flows"

    errors = []
    summary = []

    probes_by_provider = {}
    for path in sorted(probes_dir.glob("*.json")):
        payload = load_json(path)
        validate_common_catalog(path, payload, repo_root, errors)
        validate_probe_catalog(path, payload, repo_root, errors)
        provider = payload.get("provider")
        if not provider:
            continue
        if provider in probes_by_provider:
            errors.append(f"duplicate probe catalog for provider={provider}")
            continue
        probes_by_provider[provider] = (path, payload)

    capabilities_by_provider = {}
    for path in sorted(capabilities_dir.glob("*.json")):
        payload = load_json(path)
        validate_common_catalog(path, payload, repo_root, errors)
        validate_capability_catalog(path, payload, repo_root, errors)
        provider = payload.get("provider")
        if not provider:
            continue
        capabilities_by_provider.setdefault(provider, []).append((path, payload))

    browser_flows_by_provider = {}
    for path in sorted(browser_flows_dir.glob("*.json")):
        payload = load_json(path)
        validate_common_catalog(path, payload, repo_root, errors)
        validate_browser_flow_catalog(path, payload, repo_root, errors)
        provider = payload.get("provider")
        if not provider:
            continue
        browser_flows_by_provider.setdefault(provider, []).append((path, payload))

    for provider in ACTIVE_PROVIDERS:
        probe_entry = probes_by_provider.get(provider)
        if not probe_entry:
            errors.append(f"{provider}: missing provider probe catalog")
            continue
        probe_path, probe_catalog = probe_entry
        items = probe_catalog.get("probe_items", [])
        confirmed_count = sum(1 for item in items if item.get("status") == "confirmed")
        probe_status = str(probe_catalog.get("probe_catalog_status", "")).strip().lower()
        if confirmed_count == 0:
            errors.append(
                f"{provider}: probe catalog must contain at least one confirmed probe item"
            )

        if provider not in browser_flows_by_provider:
            errors.append(f"{provider}: missing browser flow catalog")

        capability_entries = capabilities_by_provider.get(provider, [])
        if not capability_entries:
            errors.append(f"{provider}: missing native capability catalog")
        native_statuses = sorted(
            {
                str(catalog.get("native_capability_status", "")).strip().lower()
                or "active"
                for _, catalog in capability_entries
            }
        )
        if any(status not in ACTIVE_NATIVE_CAPABILITY_STATUSES for status in native_statuses):
            errors.append(
                f"{provider}: native capability catalog must be stable for active provider "
                f"(expected statuses in {sorted(ACTIVE_NATIVE_CAPABILITY_STATUSES)}, got {native_statuses})"
            )

        summary.append(
            {
                "provider": provider,
                "probe_catalog": json_path(probe_path, repo_root),
                "probe_confirmed_count": confirmed_count,
                "probe_catalog_status": probe_status or None,
                "browser_flow_catalog_count": len(browser_flows_by_provider.get(provider, [])),
                "native_capability_catalog_count": len(capability_entries),
                "native_capability_status": native_statuses or None,
            }
        )

    for provider in PARKING_PROVIDERS:
        probe_entry = probes_by_provider.get(provider)
        if not probe_entry:
            errors.append(f"{provider}: missing parking provider probe catalog")
            continue
        _, probe_catalog = probe_entry
        lifecycle_status = str(probe_catalog.get("provider_lifecycle_status", "")).strip().lower()
        if lifecycle_status != "parking":
            errors.append(
                f"{provider}: expected provider_lifecycle_status=parking, got {lifecycle_status or '<missing>'}"
            )
        summary.append(
            {
                "provider": provider,
                "probe_catalog_status": probe_catalog.get("probe_catalog_status"),
                "provider_lifecycle_status": probe_catalog.get("provider_lifecycle_status"),
                "browser_flow_catalog_count": len(browser_flows_by_provider.get(provider, [])),
                "native_capability_catalog_count": len(capabilities_by_provider.get(provider, [])),
            }
        )

    print("catalog-lint summary:")
    for item in summary:
        print(json.dumps(item, ensure_ascii=False, sort_keys=True))

    if errors:
        print("\ncatalog-lint errors:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1

    print("\ncatalog-lint: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
