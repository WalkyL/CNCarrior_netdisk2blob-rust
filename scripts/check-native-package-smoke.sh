#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/ccbg-native-package-smoke"
DIST_DIR="${ROOT_DIR}/target/native-packages"
BACKUP_DIR="${ROOT_DIR}/target/native-package-smoke-backup"
POWERSHELL_BIN="${POWERSHELL_BIN:-/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe}"
SMOKE_PACKAGES=(
  ccbg-windows-x86_64-smoke
  ccbg-macos-x86_64-smoke
  ccbg-macos-arm64-smoke
)

if [ ! -x "${POWERSHELL_BIN}" ] && [ -x "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe" ]; then
  POWERSHELL_BIN="/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
fi

require_file() {
  local path="$1"
  if [ ! -f "${path}" ]; then
    echo "missing expected file: ${path}" >&2
    exit 1
  fi
}

extract_dir() {
  local archive="$1"
  local out_dir="$2"
  rm -rf "${out_dir}"
  mkdir -p "${out_dir}"
  case "${archive}" in
    *.zip)
      if command -v unzip >/dev/null 2>&1; then
        unzip -q "${archive}" -d "${out_dir}"
      else
        local archive_win out_dir_win
        if command -v cygpath >/dev/null 2>&1; then
          archive_win="$(cygpath -w "${archive}")"
          out_dir_win="$(cygpath -w "${out_dir}")"
        elif command -v wslpath >/dev/null 2>&1; then
          archive_win="$(wslpath -w "${archive}")"
          out_dir_win="$(wslpath -w "${out_dir}")"
        else
          echo "zip extraction requires unzip or a path converter for PowerShell Expand-Archive" >&2
          exit 1
        fi
        "${POWERSHELL_BIN}" -NoProfile -ExecutionPolicy Bypass -Command \
          "Expand-Archive -LiteralPath '${archive_win}' -DestinationPath '${out_dir_win}' -Force" >/dev/null
      fi
      ;;
    *.tar.gz)
      tar -xzf "${archive}" -C "${out_dir}"
      ;;
    *)
      echo "unsupported archive type: ${archive}" >&2
      exit 1
      ;;
  esac
}

check_package_root() {
  local package_root="$1"
  local platform="$2"

  require_file "${package_root}/PACKAGE-METADATA"
  require_file "${package_root}/MANIFEST.sha256"
  require_file "${package_root}/assets/admin/index.html"

  case "${platform}" in
    windows)
      require_file "${package_root}/bin/gatewayd.exe"
      require_file "${package_root}/deploy/windows/install.ps1"
      require_file "${package_root}/deploy/windows/uninstall.ps1"
      grep -F -- 'CCBG_BROWSER_FLOW_CATALOG_DIR' "${package_root}/deploy/windows/install.ps1" >/dev/null
      grep -F -- 'CCBG_PROVIDER_BRIDGE_CATALOG_DIR' "${package_root}/deploy/windows/install.ps1" >/dev/null
      grep -F -- 'CCBG_PROVIDER_CAPABILITY_CATALOG_DIR' "${package_root}/deploy/windows/install.ps1" >/dev/null
      ;;
    macos)
      require_file "${package_root}/bin/gatewayd"
      require_file "${package_root}/deploy/macos/install.sh"
      require_file "${package_root}/deploy/macos/uninstall.sh"
      require_file "${package_root}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template"
      grep -F -- 'CCBG_BROWSER_FLOW_CATALOG_DIR' "${package_root}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template" >/dev/null
      grep -F -- 'CCBG_PROVIDER_BRIDGE_CATALOG_DIR' "${package_root}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template" >/dev/null
      grep -F -- 'CCBG_PROVIDER_CAPABILITY_CATALOG_DIR' "${package_root}/deploy/macos/online.agi2030.ccbg.gatewayd.plist.template" >/dev/null
      ;;
    *)
      echo "unsupported platform: ${platform}" >&2
      exit 1
      ;;
  esac
}

backup_target_release_dir() {
  local target="$1"
  local backup_path="${BACKUP_DIR}/${target}"
  local release_dir="${ROOT_DIR}/target/${target}/release"
  rm -rf "${backup_path}"
  mkdir -p "$(dirname "${backup_path}")"
  if [ -d "${release_dir}" ]; then
    cp -R "${release_dir}" "${backup_path}"
  fi
}

restore_target_release_dir() {
  local target="$1"
  local backup_path="${BACKUP_DIR}/${target}"
  local release_dir="${ROOT_DIR}/target/${target}/release"
  rm -rf "${release_dir}"
  if [ -d "${backup_path}" ]; then
    mkdir -p "$(dirname "${release_dir}")"
    cp -R "${backup_path}" "${release_dir}"
  fi
}

write_fake_binary() {
  local target="$1"
  local binary_name="$2"
  local content="$3"
  local release_dir="${ROOT_DIR}/target/${target}/release"
  mkdir -p "${release_dir}"
  printf '%s' "${content}" > "${release_dir}/${binary_name}"
  chmod +x "${release_dir}/${binary_name}"
}

cleanup() {
  restore_target_release_dir "x86_64-pc-windows-gnu"
  restore_target_release_dir "x86_64-apple-darwin"
  restore_target_release_dir "aarch64-apple-darwin"
  for package_name in "${SMOKE_PACKAGES[@]}"; do
    rm -rf "${DIST_DIR}/${package_name}" \
           "${DIST_DIR}/${package_name}.zip" \
           "${DIST_DIR}/${package_name}.zip.sha256" \
           "${DIST_DIR}/${package_name}.tar.gz" \
           "${DIST_DIR}/${package_name}.tar.gz.sha256"
  done
  rm -rf "${WORK_DIR}" "${BACKUP_DIR}"
}
trap cleanup EXIT

if ! command -v zip >/dev/null 2>&1; then
  if [ ! -x "${POWERSHELL_BIN}" ] || { ! command -v cygpath >/dev/null 2>&1 && ! command -v wslpath >/dev/null 2>&1; }; then
    echo "native package smoke requires Windows zip packaging support" >&2
    echo "install zip or run from a host where PowerShell Compress-Archive + path conversion are available" >&2
    exit 1
  fi
fi

rm -rf "${WORK_DIR}" "${BACKUP_DIR}"
mkdir -p "${WORK_DIR}"

backup_target_release_dir "x86_64-pc-windows-gnu"
backup_target_release_dir "x86_64-apple-darwin"
backup_target_release_dir "aarch64-apple-darwin"

write_fake_binary "x86_64-pc-windows-gnu" "gatewayd.exe" 'fake windows gatewayd'
write_fake_binary "x86_64-apple-darwin" "gatewayd" '#!/bin/sh
exit 0
'
write_fake_binary "aarch64-apple-darwin" "gatewayd" '#!/bin/sh
exit 0
'

run_build() {
  local target="$1"
  local package_name="$2"
  local archive_ext="$3"
  local unpacked_dir="$4"
  local platform="$5"

  CCBG_NATIVE_TARGET="${target}" \
  CCBG_NATIVE_PACKAGE_NAME="${package_name}" \
  bash "${ROOT_DIR}/scripts/build-native-package.sh" --skip-build --target "${target}" --package-name "${package_name}" >/dev/null

  require_file "${DIST_DIR}/${package_name}.${archive_ext}"
  require_file "${DIST_DIR}/${package_name}.${archive_ext}.sha256"
  extract_dir "${DIST_DIR}/${package_name}.${archive_ext}" "${WORK_DIR}/${unpacked_dir}"
  check_package_root "${WORK_DIR}/${unpacked_dir}/${package_name}" "${platform}"
}

run_build "x86_64-pc-windows-gnu" "ccbg-windows-x86_64-smoke" "zip" "windows" "windows"
run_build "x86_64-apple-darwin" "ccbg-macos-x86_64-smoke" "tar.gz" "macos-x86_64" "macos"
run_build "aarch64-apple-darwin" "ccbg-macos-arm64-smoke" "tar.gz" "macos-arm64" "macos"

echo "native package smoke checks passed"
