#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/ccbg-native-package-script-tests"
BUILD_SCRIPT="${ROOT_DIR}/scripts/build-native-package.sh"
SMOKE_SCRIPT="${ROOT_DIR}/scripts/check-native-package-smoke.sh"
BACKUP_DIR="${ROOT_DIR}/target/native-package-script-tests-backup"
SCRIPT_SMOKE_PACKAGES=(
  ccbg-windows-nozip-smoke
  ccbg-macos-x86_64-script-smoke
)

require_file() {
  local path="$1"
  if [ ! -f "${path}" ]; then
    echo "missing expected file: ${path}" >&2
    exit 1
  fi
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

cleanup() {
  restore_target_release_dir "x86_64-pc-windows-gnu"
  restore_target_release_dir "x86_64-apple-darwin"
  restore_target_release_dir "aarch64-unknown-linux-musl"
  for package_name in "${SCRIPT_SMOKE_PACKAGES[@]}"; do
    rm -rf "${ROOT_DIR}/target/native-packages/${package_name}" \
           "${ROOT_DIR}/target/native-packages/${package_name}.zip" \
           "${ROOT_DIR}/target/native-packages/${package_name}.zip.sha256" \
           "${ROOT_DIR}/target/native-packages/${package_name}.tar.gz" \
           "${ROOT_DIR}/target/native-packages/${package_name}.tar.gz.sha256"
  done
  rm -rf "${BACKUP_DIR}"
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"

backup_target_release_dir "x86_64-pc-windows-gnu"
backup_target_release_dir "x86_64-apple-darwin"
backup_target_release_dir "aarch64-unknown-linux-musl"

expect_fail() {
  local label="$1"
  local expected="$2"
  shift 2
  local log_file="${WORK_DIR}/$(printf '%s' "${label}" | tr ' /:' '___').log"
  if "$@" >"${log_file}" 2>&1; then
    echo "expected failure: ${label}" >&2
    cat "${log_file}" >&2
    exit 1
  fi
  if ! grep -F -- "${expected}" "${log_file}" >/dev/null; then
    echo "missing expected error text for: ${label}" >&2
    cat "${log_file}" >&2
    exit 1
  fi
}

expect_pass() {
  local label="$1"
  shift
  if ! "$@" >/dev/null 2>&1; then
    echo "expected success: ${label}" >&2
    exit 1
  fi
}

echo "[1/8] build-native-package requires --target"
expect_fail \
  "missing-target" \
  "--target is required" \
  bash "${BUILD_SCRIPT}"

echo "[2/8] build-native-package rejects unknown arguments"
expect_fail \
  "unknown-arg" \
  "unknown argument: --bad-flag" \
  bash "${BUILD_SCRIPT}" --bad-flag

echo "[3/8] build-native-package fails when --skip-build has no target binary"
expect_fail \
  "missing-binary" \
  "missing release binary:" \
  bash "${BUILD_SCRIPT}" --skip-build --target aarch64-unknown-linux-musl --package-name ccbg-linux-missing-binary-smoke

echo "[4/9] build-native-package does not fall back to host binary for a different target"
mkdir -p "${ROOT_DIR}/target/release"
printf '#!/bin/sh\nexit 0\n' > "${ROOT_DIR}/target/release/gatewayd"
chmod +x "${ROOT_DIR}/target/release/gatewayd"
rm -rf "${ROOT_DIR}/target/x86_64-apple-darwin/release"
expect_fail \
  "no-host-fallback" \
  "missing release binary:" \
  bash "${BUILD_SCRIPT}" --skip-build --target x86_64-apple-darwin --package-name ccbg-macos-host-fallback-smoke

echo "[5/9] build-native-package still emits a Windows zip under restricted PATH"
mkdir -p "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release"
printf 'fake windows gatewayd\n' > "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release/gatewayd.exe"
chmod +x "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release/gatewayd.exe"
expect_pass \
  "windows-zip-under-restricted-path" \
  env PATH="/usr/bin:/bin" bash "${BUILD_SCRIPT}" --skip-build --target x86_64-pc-windows-gnu --package-name ccbg-windows-nozip-smoke
require_file "${ROOT_DIR}/target/native-packages/ccbg-windows-nozip-smoke.zip"

echo "[6/9] build-native-package emits macOS tarball when fake target binary is present"
mkdir -p "${ROOT_DIR}/target/x86_64-apple-darwin/release"
printf '#!/bin/sh\nexit 0\n' > "${ROOT_DIR}/target/x86_64-apple-darwin/release/gatewayd"
chmod +x "${ROOT_DIR}/target/x86_64-apple-darwin/release/gatewayd"
expect_pass \
  "macos-tarball" \
  bash "${BUILD_SCRIPT}" --skip-build --target x86_64-apple-darwin --package-name ccbg-macos-x86_64-script-smoke

echo "[7/9] native package smoke script passes in current host configuration"
expect_pass \
  "native-smoke-pass" \
  bash "${SMOKE_SCRIPT}"

echo "[8/9] native package smoke still passes under restricted PATH on this host"
expect_pass \
  "native-smoke-restricted-path" \
  env PATH="/usr/bin:/bin" bash "${SMOKE_SCRIPT}"

echo "[9/9] native package smoke restores pre-existing target release directories"
release_probe_dir="${ROOT_DIR}/target/x86_64-apple-darwin/release"
mkdir -p "${release_probe_dir}"
printf 'sentinel\n' > "${release_probe_dir}/sentinel-before-smoke.txt"
expect_pass \
  "native-smoke-restores-target" \
  bash "${SMOKE_SCRIPT}"
if [ ! -f "${release_probe_dir}/sentinel-before-smoke.txt" ]; then
  echo "native package smoke did not restore target release contents" >&2
  exit 1
fi

echo "native package script edge checks passed"
