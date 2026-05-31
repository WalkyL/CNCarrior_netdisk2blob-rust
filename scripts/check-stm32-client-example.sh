#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXAMPLE_DIR="${ROOT_DIR}/examples/stm32-client-only"
OUT_DIR="${ROOT_DIR}/target/stm32-client-example"
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
  "${EXAMPLE_DIR}/ccbg_stm32_client.c" \
  "${EXAMPLE_DIR}/example_main.c" \
  -o "${OUT_DIR}/stm32-client-example"

"${OUT_DIR}/stm32-client-example" >/dev/null

echo "stm32 client-only example check passed"
