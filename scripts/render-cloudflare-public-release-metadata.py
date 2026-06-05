#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

from ccbg_release_metadata import (
    public_materials_provenance_payload,
    public_materials_seed_sha256,
    public_materials_seed_text,
)


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_INPUT = ROOT / "public" / "cloudflare"
DEFAULT_OUTPUT = ROOT / "target" / "cloudflare-public-assets"
EXCLUDED_NAMES = {
    "functions",
    "worker.js",
    "wrangler.toml",
    "wrangler.worker.toml",
    ".wrangler",
    "target",
}


TEXT_SUFFIXES = {
    ".html",
    ".js",
    ".css",
    ".json",
    ".svg",
    ".md",
    ".map",
    ".txt",
}
TEXT_FILENAMES = {"_headers"}

PLACEHOLDERS: dict[str, str] = {}


def build_placeholders() -> dict[str, str]:
    metadata = public_materials_provenance_payload()
    release_notes_version = metadata["version"]
    payload_json = json.dumps(metadata, indent=2, ensure_ascii=False) + "\n"
    placeholders = {
        "__CCBG_PUBLIC_VERSION__": str(metadata["version"]),
        "__CCBG_PUBLIC_RELEASE_DATE__": str(metadata["release_date"]),
        "__CCBG_PUBLIC_RELEASE_FINGERPRINT__": str(metadata["release_fingerprint"]),
        "__CCBG_PUBLIC_FINGERPRINT_SHA256__": str(metadata["fingerprint_sha256"]),
        "__CCBG_PUBLIC_CANONICAL_REPO__": str(metadata["canonical_repo"]),
        "__CCBG_PUBLIC_PROVENANCE_JSON__": payload_json,
        "__CCBG_PUBLIC_RELEASE_NOTES_FILE__": f"v{release_notes_version}-public.md",
        "__CCBG_PUBLIC_RELEASE_NOTES_TITLE__": f"CCBG Public Frontend v{release_notes_version}",
        "__CCBG_PUBLIC_SEED_TEXT__": public_materials_seed_text(),
        "__CCBG_PUBLIC_SEED_SHA256__": public_materials_seed_sha256(),
    }
    return placeholders


def should_template(path: Path) -> bool:
    return path.name in TEXT_FILENAMES or path.suffix.lower() in TEXT_SUFFIXES


def render_text(text: str, placeholders: dict[str, str]) -> str:
    rendered = text
    for key, value in placeholders.items():
        rendered = rendered.replace(key, value)
    return rendered


def release_note_sort_key(path: Path) -> tuple[int, int, int]:
    match = re.match(r"v(\d+)\.(\d+)\.(\d+)-public\.md$", path.name)
    if not match:
        return (0, 0, 0)
    return tuple(int(part) for part in match.groups())


def copy_tree(src: Path, dst: Path, placeholders: dict[str, str]) -> None:
    for path in src.rglob("*"):
        relative = path.relative_to(src)
        if any(part in EXCLUDED_NAMES for part in relative.parts):
            continue
        target = dst / relative
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if should_template(path):
            text = path.read_text(encoding="utf-8", errors="replace")
            target.write_text(render_text(text, placeholders), encoding="utf-8")
        else:
            target.write_bytes(path.read_bytes())


def normalize_release_notes(dst: Path, placeholders: dict[str, str]) -> None:
    notes_dir = dst / "release-notes"
    if not notes_dir.is_dir():
        return
    note_files = sorted(
        notes_dir.glob("v*-public.md"),
        key=release_note_sort_key,
        reverse=True,
    )
    if not note_files:
        return
    template_candidates = []
    for path in note_files:
        text = path.read_text(encoding="utf-8", errors="replace")
        if "__CCBG_PUBLIC_" in text:
            template_candidates.append((path, text))
    if template_candidates:
        _, rendered = template_candidates[0]
    else:
        rendered = note_files[0].read_text(encoding="utf-8", errors="replace")
    target = notes_dir / placeholders["__CCBG_PUBLIC_RELEASE_NOTES_FILE__"]
    for path in note_files:
        path.unlink()
    target.write_text(rendered, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description="Render Cloudflare public assets with current release metadata.")
    parser.add_argument("--input-dir", default=str(DEFAULT_INPUT))
    parser.add_argument("--output-dir", default=str(DEFAULT_OUTPUT))
    args = parser.parse_args()

    src = Path(args.input_dir)
    dst = Path(args.output_dir)
    if dst.exists():
        for path in sorted(dst.rglob("*"), reverse=True):
            if path.is_file() or path.is_symlink():
                path.unlink()
            elif path.is_dir():
                path.rmdir()
    dst.mkdir(parents=True, exist_ok=True)
    placeholders = build_placeholders()
    copy_tree(src, dst, placeholders)
    normalize_release_notes(dst, placeholders)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
