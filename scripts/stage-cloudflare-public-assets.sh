#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-${ROOT_DIR}/target/cloudflare-public-assets}"

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

python "${ROOT_DIR}/scripts/render-cloudflare-public-release-metadata.py" \
  --input-dir "${ROOT_DIR}/public/cloudflare" \
  --output-dir "${OUT_DIR}"

echo "${OUT_DIR}"
