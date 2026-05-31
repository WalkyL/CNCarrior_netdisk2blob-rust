#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE_DIR="${ROOT_DIR}/examples/esp32-s3-relay-lite-poc"
OUT_DIR="${ROOT_DIR}/target/esp32-s3-relay-lite-poc"
CC_BIN="${CC:-gcc}"
read -r -a CC_CMD <<< "${CC_BIN}"

mkdir -p "${OUT_DIR}"

"${CC_CMD[@]}" \
  -std=c99 \
  -Wall \
  -Wextra \
  -Werror \
  -pedantic \
  -I"${EXAMPLE_DIR}" \
  "${EXAMPLE_DIR}/relay_lite_poc.c" \
  "${EXAMPLE_DIR}/example_main.c" \
  -o "${OUT_DIR}/relay-lite-poc"

"${OUT_DIR}/relay-lite-poc" >/dev/null

if command -v rg >/dev/null 2>&1; then
  forbidden_matches="$(rg -n "onedrive|rusqlite|gatewayd|replication-engine|replication_engine" "${EXAMPLE_DIR}" -g '*.c' -g '*.h' || true)"
else
  forbidden_matches="$(grep -RInE "onedrive|rusqlite|gatewayd|replication-engine|replication_engine" "${EXAMPLE_DIR}" --include='*.c' --include='*.h' || true)"
fi

if [ -n "${forbidden_matches}" ]; then
  printf '%s\n' "${forbidden_matches}" >&2
  echo "relay-lite PoC contains a forbidden host dependency/reference" >&2
  exit 1
fi

echo "esp32-s3 relay-lite PoC check passed"
