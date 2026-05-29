#!/usr/bin/env python3
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

from __future__ import annotations

from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXAMPLE = ROOT / "examples" / "esp32-s3-client-only"
REQUIRED = [
    EXAMPLE / "CMakeLists.txt",
    EXAMPLE / "README.md",
    EXAMPLE / "main" / "CMakeLists.txt",
    EXAMPLE / "main" / "Kconfig.projbuild",
    EXAMPLE / "main" / "ccbg_esp32s3_demo.c",
]
FORBIDDEN = [
    "rusqlite",
    "provider-onedrive",
    "provider_onedrive",
    "onedrive",
    "gatewayd",
    "replication-engine",
    "replication_engine",
]


def main() -> int:
    missing = [path for path in REQUIRED if not path.exists()]
    if missing:
        for path in missing:
            print(f"missing {path.relative_to(ROOT)}")
        return 1

    c_text = (EXAMPLE / "main" / "ccbg_esp32s3_demo.c").read_text(encoding="utf-8")
    readme_text = (EXAMPLE / "README.md").read_text(encoding="utf-8")
    implementation_text = "\n".join(
        path.read_text(encoding="utf-8", errors="replace")
        for path in REQUIRED
        if path.suffix in {".c", ".txt"} or path.name.startswith("CMakeLists")
    ).lower()

    for needle in [
        "#define CCBG_ESP32S3_IO_CHUNK_BYTES 1024u",
        "#define CCBG_ESP32S3_MAX_OBJECT_BYTES (32u * 1024u)",
        "ccbg_stm32_put_object_stream",
        "ccbg_stm32_head_object",
        "ccbg_stm32_get_object",
        "mbedtls_md_hmac",
        "esp_http_client",
    ]:
        if needle not in c_text:
            print(f"missing required ESP32-S3 demo marker: {needle}")
            return 1

    for needle in FORBIDDEN:
        if needle in implementation_text:
            print(f"forbidden host dependency/reference in ESP32-S3 example: {needle}")
            return 1

    for needle in [
        "one request in flight",
        "1024",
        "32 KiB",
        "idf.py build",
    ]:
        if needle not in readme_text:
            print(f"missing README acceptance/config text: {needle}")
            return 1

    print("esp32-s3 client-only example check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
