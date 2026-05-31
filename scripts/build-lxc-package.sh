#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/target/lxc-package"
PACKAGE_NAME="${CCBG_LXC_PACKAGE_NAME:-ccbg-lxc-package}"
TARGET="${CCBG_LXC_TARGET:-}"
SKIP_BUILD=false

usage() {
  echo "usage: scripts/build-lxc-package.sh [--target <rust-target>] [--skip-build]"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target)
      if [ "$#" -lt 2 ]; then
        echo "--target requires a Rust target triple" >&2
        exit 2
      fi
      TARGET="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=true
      shift
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

if [ "${SKIP_BUILD}" != true ]; then
  if [ -n "${TARGET}" ]; then
    cargo build --release --target "${TARGET}" -p gatewayd
  else
    cargo build --release -p gatewayd
  fi
fi

if [ -n "${TARGET}" ]; then
  BINARY="${ROOT_DIR}/target/${TARGET}/release/gatewayd"
else
  BINARY="${ROOT_DIR}/target/release/gatewayd"
fi

if [ ! -s "${BINARY}" ]; then
  echo "missing release binary: ${BINARY}" >&2
  echo "run without --skip-build or build gatewayd first" >&2
  exit 1
fi

rm -rf "${DIST_DIR}"
mkdir -p \
  "${DIST_DIR}/${PACKAGE_NAME}/bin" \
  "${DIST_DIR}/${PACKAGE_NAME}/assets/admin" \
  "${DIST_DIR}/${PACKAGE_NAME}/config" \
  "${DIST_DIR}/${PACKAGE_NAME}/etc" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts" \
  "${DIST_DIR}/${PACKAGE_NAME}/systemd" \
  "${DIST_DIR}/${PACKAGE_NAME}/docs"

install -m 0755 "${BINARY}" "${DIST_DIR}/${PACKAGE_NAME}/bin/gatewayd"
install -m 0644 "${ROOT_DIR}/crates/gatewayd/assets/admin/index.html" "${DIST_DIR}/${PACKAGE_NAME}/assets/admin/index.html"
cp -R "${ROOT_DIR}/config/." "${DIST_DIR}/${PACKAGE_NAME}/config/"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg.env" "${DIST_DIR}/${PACKAGE_NAME}/etc/ccbg.env"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg.service" "${DIST_DIR}/${PACKAGE_NAME}/systemd/ccbg.service"
install -m 0755 "${ROOT_DIR}/deploy/lxc/install.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/install.sh"
install -m 0755 "${ROOT_DIR}/deploy/lxc/rollback.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/rollback.sh"
install -m 0755 "${ROOT_DIR}/deploy/lxc/smoke.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/smoke.sh"
install -m 0644 "${ROOT_DIR}/docs/pve-lxc-deployment.md" "${DIST_DIR}/${PACKAGE_NAME}/docs/pve-lxc-deployment.md"

{
  echo "package=${PACKAGE_NAME}"
  echo "rust_target=${TARGET:-host}"
  echo "built_at_unix=$(date +%s)"
  echo "gatewayd_sha256=$(sha256sum "${BINARY}" | awk '{print $1}')"
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
