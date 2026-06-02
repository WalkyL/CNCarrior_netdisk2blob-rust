#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUCKET_NAME="${1:-}"
PREFIX="${CCBG_CF_RELEASE_CACHE_PREFIX:-latest}"

usage() {
  echo "usage: scripts/sync-cloudflare-release-cache.sh <bucket-name>"
}

if [ -z "${BUCKET_NAME}" ]; then
  usage >&2
  exit 2
fi

export CLOUDFLARE_API_TOKEN="${CLOUDFLARE_API_TOKEN:-${CF_API_TOKEN:-}}"
export CLOUDFLARE_ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-${CF_ACCOUNT_ID:-}}"

if [ -z "${CLOUDFLARE_API_TOKEN}" ]; then
  echo "missing CLOUDFLARE_API_TOKEN or CF_API_TOKEN" >&2
  exit 1
fi
if [ -z "${CLOUDFLARE_ACCOUNT_ID}" ]; then
  echo "missing CLOUDFLARE_ACCOUNT_ID or CF_ACCOUNT_ID" >&2
  exit 1
fi

cd "${ROOT_DIR}"

WRANGLER_CMD=(npx wrangler@latest)

asset_names=(
  "ccbg-lxc-package.tar.gz"
  "ccbg-windows-x86_64.zip"
  "ccbg-macos-x86_64.tar.gz"
  "ccbg-macos-arm64.tar.gz"
  "ccbg-openwrt-lite.tar.gz"
)
asset_paths=(
  "target/lxc-package/ccbg-lxc-package.tar.gz"
  "target/native-packages/ccbg-windows-x86_64.zip"
  "target/native-packages/ccbg-macos-x86_64.tar.gz"
  "target/native-packages/ccbg-macos-arm64.tar.gz"
  "target/openwrt-lite/ccbg-openwrt-lite.tar.gz"
)

for path in "${asset_paths[@]}"; do
  if [ ! -s "${path}" ]; then
    echo "missing release asset: ${path}" >&2
    exit 1
  fi
done

for idx in "${!asset_names[@]}"; do
  name="${asset_names[$idx]}"
  path="${asset_paths[$idx]}"
  key="${PREFIX}/${name}"
  echo "uploading ${path} -> r2://${BUCKET_NAME}/${key}"
  "${WRANGLER_CMD[@]}" r2 object put "${BUCKET_NAME}/${key}" \
    --remote \
    --file="${path}" \
    --content-type="application/octet-stream" \
    --cache-control="public, max-age=300" \
    --content-disposition="attachment; filename=\"${name}\""
done
