#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-${ROOT_DIR}/target/cloudflare-public-assets}"

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

if command -v rsync >/dev/null 2>&1; then
  rsync -a --delete \
    --exclude 'functions' \
    --exclude 'worker.js' \
    --exclude 'wrangler.toml' \
    --exclude 'wrangler.worker.toml' \
    --exclude '.wrangler' \
    --exclude 'target' \
    "${ROOT_DIR}/public/cloudflare/" "${OUT_DIR}/"
else
  shopt -s dotglob nullglob
  SRC_DIR="${ROOT_DIR}/public/cloudflare"
  for path in "${SRC_DIR}"/*; do
    name="$(basename "${path}")"
    case "${name}" in
      functions|worker.js|wrangler.toml|wrangler.worker.toml|.wrangler|target)
        continue
        ;;
    esac
    cp -R "${path}" "${OUT_DIR}/"
  done
fi

echo "${OUT_DIR}"
