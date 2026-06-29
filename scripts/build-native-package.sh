#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/target/native-packages"
TARGET="${CCBG_NATIVE_TARGET:-}"
PACKAGE_NAME="${CCBG_NATIVE_PACKAGE_NAME:-}"
CARGO_BIN="$(bash "${ROOT_DIR}/scripts/resolve-cargo.sh")"
GIT_BIN="$(bash "${ROOT_DIR}/scripts/resolve-git.sh")"
SKIP_BUILD=false

usage() {
  echo "usage: scripts/build-native-package.sh --target <rust-target> [--skip-build] [--package-name <name>]"
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
    --package-name)
      if [ "$#" -lt 2 ]; then
        echo "--package-name requires a value" >&2
        exit 2
      fi
      PACKAGE_NAME="$2"
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

if [ -z "${TARGET}" ]; then
  echo "--target is required" >&2
  usage >&2
  exit 2
fi

case "${TARGET}" in
  x86_64-pc-windows-*|x86_64-*-windows-*)
    platform="windows"
    arch="x86_64"
    binary_name="gatewayd.exe"
    default_package="ccbg-windows-x86_64"
    ;;
  x86_64-apple-darwin)
    platform="macos"
    arch="x86_64"
    binary_name="gatewayd"
    default_package="ccbg-macos-x86_64"
    ;;
  aarch64-apple-darwin)
    platform="macos"
    arch="arm64"
    binary_name="gatewayd"
    default_package="ccbg-macos-arm64"
    ;;
  *)
    platform="linux"
    arch="${TARGET}"
    binary_name="gatewayd"
    default_package="ccbg-native-${TARGET}"
    ;;
esac

PACKAGE_NAME="${PACKAGE_NAME:-${default_package}}"
target_dir="${ROOT_DIR}/target/${TARGET}/release"
binary="${target_dir}/${binary_name}"

if [ "${SKIP_BUILD}" != true ]; then
  "${CARGO_BIN}" build --release --target "${TARGET}" -p gatewayd
fi

if [ ! -s "${binary}" ] && [ "${SKIP_BUILD}" = true ] && [ -s "${ROOT_DIR}/target/release/${binary_name}" ]; then
  binary="${ROOT_DIR}/target/release/${binary_name}"
fi

if [ ! -s "${binary}" ]; then
  echo "missing release binary: ${binary}" >&2
  echo "run without --skip-build or build gatewayd for ${TARGET} first" >&2
  exit 1
fi

package_root="${DIST_DIR}/${PACKAGE_NAME}"
rm -rf "${package_root}"
mkdir -p \
  "${package_root}/bin" \
  "${package_root}/assets/admin" \
  "${package_root}/config" \
  "${package_root}/deploy" \
  "${package_root}/docs"

install -m 0755 "${binary}" "${package_root}/bin/${binary_name}"
install -m 0644 "${ROOT_DIR}/crates/gatewayd/assets/admin/index.html" "${package_root}/assets/admin/index.html"
cp -R "${ROOT_DIR}/config/." "${package_root}/config/"
install -m 0644 "${ROOT_DIR}/docs/compatibility-matrix.md" "${package_root}/docs/compatibility-matrix.md"
install -m 0644 "${ROOT_DIR}/docs/release-checklist.md" "${package_root}/docs/release-checklist.md"

case "${platform}" in
  windows)
    mkdir -p "${package_root}/deploy/windows"
    install -m 0644 "${ROOT_DIR}/deploy/windows/install.ps1" "${package_root}/deploy/windows/install.ps1"
    install -m 0644 "${ROOT_DIR}/deploy/windows/uninstall.ps1" "${package_root}/deploy/windows/uninstall.ps1"
    ;;
  macos)
    mkdir -p "${package_root}/deploy/macos"
    install -m 0755 "${ROOT_DIR}/deploy/macos/install.sh" "${package_root}/deploy/macos/install.sh"
    install -m 0755 "${ROOT_DIR}/deploy/macos/uninstall.sh" "${package_root}/deploy/macos/uninstall.sh"
    install -m 0644 "${ROOT_DIR}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template" "${package_root}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template"
    ;;
  *)
    mkdir -p "${package_root}/deploy/linux"
    install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg.env" "${package_root}/deploy/linux/ccbg.env"
    ;;
esac

{
  echo "package=${PACKAGE_NAME}"
  echo "platform=${platform}"
  echo "arch=${arch}"
  echo "rust_target=${TARGET}"
  echo "built_at_unix=$(date +%s)"
  echo "gatewayd_sha256=$(sha256sum "${binary}" | awk '{print $1}')"
  echo "admin_html_sha256=$(sha256sum "${ROOT_DIR}/crates/gatewayd/assets/admin/index.html" | awk '{print $1}')"
  "${GIT_BIN}" -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null | sed 's/^/git_commit=/' || true
} > "${package_root}/PACKAGE-METADATA"

(
  cd "${package_root}"
  find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256
)

POWERSHELL_BIN="${POWERSHELL_BIN:-/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe}"

if [ "${platform}" = "windows" ] && command -v zip >/dev/null 2>&1; then
  artifact="${DIST_DIR}/${PACKAGE_NAME}.zip"
  rm -f "${artifact}"
  (
    cd "${DIST_DIR}"
    zip -qr "${artifact}" "${PACKAGE_NAME}"
  )
elif [ "${platform}" = "windows" ] && [ -x "${POWERSHELL_BIN}" ] && command -v cygpath >/dev/null 2>&1; then
  artifact="${DIST_DIR}/${PACKAGE_NAME}.zip"
  rm -f "${artifact}"
  artifact_win="$(cygpath -w "${artifact}")"
  package_root_win="$(cygpath -w "${package_root}")"
  "${POWERSHELL_BIN}" -NoProfile -ExecutionPolicy Bypass -Command \
    "Compress-Archive -Path '${package_root_win}' -DestinationPath '${artifact_win}' -Force" >/dev/null
else
  artifact="${DIST_DIR}/${PACKAGE_NAME}.tar.gz"
  tar -C "${DIST_DIR}" -czf "${artifact}" "${PACKAGE_NAME}"
fi

sha256sum "${artifact}" > "${artifact}.sha256"
echo "${artifact}"
cat "${artifact}.sha256"
