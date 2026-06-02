#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_PATH="${1:-}"
R2_BUCKET_NAME="${2:-}"

if [ -z "${OUT_PATH}" ]; then
  echo "usage: scripts/render-cloudflare-worker-config.sh <out-path> [release-r2-bucket]" >&2
  exit 2
fi

mkdir -p "$(dirname "${OUT_PATH}")"
while IFS= read -r line; do
  if [[ "${line}" == main\ =* ]]; then
    printf 'main = "../public/cloudflare/worker.js"\n'
  else
    printf '%s\n' "${line}"
  fi
done < "${ROOT_DIR}/public/cloudflare/wrangler.worker.toml" > "${OUT_PATH}"

if [ -n "${R2_BUCKET_NAME}" ]; then
  {
    printf '\n[[r2_buckets]]\n'
    printf 'binding = "RELEASE_ASSETS"\n'
    printf 'bucket_name = "%s"\n' "${R2_BUCKET_NAME}"
    printf 'preview_bucket_name = "%s"\n' "${R2_BUCKET_NAME}"
  } >> "${OUT_PATH}"
fi
