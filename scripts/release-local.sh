#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${1:-}"

usage() {
  echo "usage: scripts/release-local.sh <release-tag>"
}

if [ -z "${TAG}" ]; then
  usage >&2
  exit 2
fi

cd "${ROOT_DIR}"

if [ "${CCBG_RELEASE_ALLOW_DIRTY:-false}" != "true" ] && [ -n "$(git status --porcelain)" ]; then
  echo "working tree is dirty; commit changes or set CCBG_RELEASE_ALLOW_DIRTY=true" >&2
  exit 1
fi

if [ "${CCBG_RELEASE_SKIP_CHECKS:-false}" != "true" ]; then
  scripts/check-release-ready.sh
fi

OUT_DIR="${ROOT_DIR}/target/release-local/${TAG}"
rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

copy_artifacts() {
  local pattern
  shopt -s nullglob
  for pattern in "$@"; do
    for file in ${pattern}; do
      cp "${file}" "${OUT_DIR}/"
    done
  done
  shopt -u nullglob
}

echo "building Linux LXC package"
cargo build --release --locked -p gatewayd
scripts/build-lxc-package.sh --skip-build
copy_artifacts "${ROOT_DIR}/target/lxc-package/ccbg-lxc-package.tar.gz"*

if [ "${CCBG_RELEASE_BUILD_WINDOWS:-false}" = "true" ]; then
  echo "building Windows package"
  cargo zigbuild --release --locked --target x86_64-pc-windows-gnu -p gatewayd
  scripts/build-native-package.sh \
    --skip-build \
    --target x86_64-pc-windows-gnu \
    --package-name ccbg-windows-x86_64
  copy_artifacts "${ROOT_DIR}/target/native-packages/ccbg-windows-x86_64.zip"*
fi

if [ "${CCBG_RELEASE_BUILD_OPENWRT:-false}" = "true" ]; then
  echo "building OpenWrt lite package"
  cargo zigbuild --release --locked --target aarch64-unknown-linux-musl -p gatewayd -p mcp-server
  scripts/build-openwrt-lite-package.sh --skip-build --target aarch64-unknown-linux-musl
  copy_artifacts "${ROOT_DIR}/target/openwrt-lite/ccbg-openwrt-lite.tar.gz"*
fi

if [ -n "${CCBG_RELEASE_MACOS_ASSET_DIR:-}" ]; then
  echo "copying macOS assets from ${CCBG_RELEASE_MACOS_ASSET_DIR}"
  copy_artifacts \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-x86_64.tar.gz"* \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-arm64.tar.gz"*
fi

(
  cd "${OUT_DIR}"
  find . -maxdepth 1 -type f ! -name 'ccbg-checksums.txt' ! -name 'release-provenance.*' -printf '%f\n' \
    | LC_ALL=C sort \
    | xargs sha256sum > ccbg-checksums.txt
)

provenance_args=(
  python3 scripts/generate-release-provenance.py
  --release-name "CCBG ${TAG}"
  --tag "${TAG}"
  --out-dir "${OUT_DIR}"
  --build-step "scripts/check-release-ready.sh"
  --build-step "scripts/release-local.sh ${TAG}"
)

while IFS= read -r file; do
  provenance_args+=(--artifact "${OUT_DIR}/${file}")
done < <(find "${OUT_DIR}" -maxdepth 1 -type f ! -name 'release-provenance.*' -printf '%f\n' | LC_ALL=C sort)

"${provenance_args[@]}"

if [ "${CCBG_RELEASE_UPLOAD_GITHUB:-false}" = "true" ]; then
  if gh release view "${TAG}" >/dev/null 2>&1; then
    gh release upload "${TAG}" "${OUT_DIR}"/* --clobber
  else
    gh release create "${TAG}" "${OUT_DIR}"/* --title "${TAG}" --notes "CCBG local release ${TAG}"
  fi
fi

echo "${OUT_DIR}"
