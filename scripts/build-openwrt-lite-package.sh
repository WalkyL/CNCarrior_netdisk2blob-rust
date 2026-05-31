#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/target/openwrt-lite"
PACKAGE_NAME="${CCBG_OPENWRT_PACKAGE_NAME:-ccbg-openwrt-lite}"
TARGET="${CCBG_OPENWRT_TARGET:-}"
SKIP_BUILD=false

usage() {
  echo "usage: scripts/build-openwrt-lite-package.sh [--skip-build] [--target <rust-target>]"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-build)
      SKIP_BUILD=true
      shift
      ;;
    --target)
      if [ "$#" -lt 2 ]; then
        echo "--target requires a rust target triple" >&2
        exit 2
      fi
      TARGET="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

target_args=()
target_dir="${ROOT_DIR}/target/release"
if [ -n "${TARGET}" ]; then
  target_args=(--target "${TARGET}")
  target_dir="${ROOT_DIR}/target/${TARGET}/release"
fi

if [ "${SKIP_BUILD}" != true ]; then
  cargo build --release "${target_args[@]}" -p gatewayd -p mcp-server
fi

gatewayd_binary="${target_dir}/gatewayd"
mcp_binary="${target_dir}/mcp-server"
if [ ! -s "${gatewayd_binary}" ]; then
  echo "missing release binary: ${gatewayd_binary}" >&2
  echo "run without --skip-build or build gatewayd first" >&2
  exit 1
fi
if [ ! -s "${mcp_binary}" ]; then
  echo "missing release binary: ${mcp_binary}" >&2
  echo "run without --skip-build or build mcp-server first" >&2
  exit 1
fi

rm -rf "${DIST_DIR}"
mkdir -p \
  "${DIST_DIR}/${PACKAGE_NAME}/bin" \
  "${DIST_DIR}/${PACKAGE_NAME}/assets/admin" \
  "${DIST_DIR}/${PACKAGE_NAME}/config" \
  "${DIST_DIR}/${PACKAGE_NAME}/etc" \
  "${DIST_DIR}/${PACKAGE_NAME}/init.d" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts" \
  "${DIST_DIR}/${PACKAGE_NAME}/docs"

install -m 0755 "${gatewayd_binary}" "${DIST_DIR}/${PACKAGE_NAME}/bin/gatewayd"
install -m 0755 "${mcp_binary}" "${DIST_DIR}/${PACKAGE_NAME}/bin/mcp-server"
install -m 0644 "${ROOT_DIR}/crates/gatewayd/assets/admin/index.html" "${DIST_DIR}/${PACKAGE_NAME}/assets/admin/index.html"
cp -R "${ROOT_DIR}/config/." "${DIST_DIR}/${PACKAGE_NAME}/config/"
install -m 0644 "${ROOT_DIR}/config/openwrt-lite.env" "${DIST_DIR}/${PACKAGE_NAME}/etc/openwrt-lite.env"
install -m 0755 "${ROOT_DIR}/deploy/openwrt/ccbg.init" "${DIST_DIR}/${PACKAGE_NAME}/init.d/ccbg"
install -m 0755 "${ROOT_DIR}/deploy/openwrt/install.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/install.sh"
install -m 0755 "${ROOT_DIR}/deploy/openwrt/smoke.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/smoke.sh"
install -m 0644 "${ROOT_DIR}/docs/openwrt-host-profile.md" "${DIST_DIR}/${PACKAGE_NAME}/docs/openwrt-host-profile.md"
install -m 0644 "${ROOT_DIR}/docs/openwrt-lite-deployment.md" "${DIST_DIR}/${PACKAGE_NAME}/docs/openwrt-lite-deployment.md"

{
  echo "package=${PACKAGE_NAME}"
  echo "rust_target=${TARGET:-host}"
  echo "built_at_unix=$(date +%s)"
  echo "gatewayd_sha256=$(sha256sum "${gatewayd_binary}" | awk '{print $1}')"
  echo "mcp_server_sha256=$(sha256sum "${mcp_binary}" | awk '{print $1}')"
  git -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null | sed 's/^/git_commit=/' || true
} > "${DIST_DIR}/${PACKAGE_NAME}/PACKAGE-METADATA"

(
  cd "${DIST_DIR}/${PACKAGE_NAME}"
  find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256
)

tarball="${DIST_DIR}/${PACKAGE_NAME}.tar.gz"
tar -C "${DIST_DIR}" -czf "${tarball}" "${PACKAGE_NAME}"
sha256sum "${tarball}" > "${tarball}.sha256"
echo "${tarball}"
cat "${tarball}.sha256"
