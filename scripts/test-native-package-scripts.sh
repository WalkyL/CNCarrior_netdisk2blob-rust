#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/ccbg-native-package-script-tests"
BUILD_SCRIPT="${ROOT_DIR}/scripts/build-native-package.sh"
SMOKE_SCRIPT="${ROOT_DIR}/scripts/check-native-package-smoke.sh"

require_file() {
  local path="$1"
  if [ ! -f "${path}" ]; then
    echo "missing expected file: ${path}" >&2
    exit 1
  fi
}

cleanup() {
  rm -rf "${WORK_DIR}"
}
trap cleanup EXIT

rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"

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

echo "[4/8] build-native-package still emits a Windows zip under restricted PATH"
mkdir -p "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release"
printf 'fake windows gatewayd\n' > "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release/gatewayd.exe"
chmod +x "${ROOT_DIR}/target/x86_64-pc-windows-gnu/release/gatewayd.exe"
expect_pass \
  "windows-zip-under-restricted-path" \
  env PATH="/usr/bin:/bin" bash "${BUILD_SCRIPT}" --skip-build --target x86_64-pc-windows-gnu --package-name ccbg-windows-nozip-smoke
require_file "${ROOT_DIR}/target/native-packages/ccbg-windows-nozip-smoke.zip"

echo "[5/8] build-native-package emits macOS tarball when fake target binary is present"
mkdir -p "${ROOT_DIR}/target/x86_64-apple-darwin/release"
printf '#!/bin/sh\nexit 0\n' > "${ROOT_DIR}/target/x86_64-apple-darwin/release/gatewayd"
chmod +x "${ROOT_DIR}/target/x86_64-apple-darwin/release/gatewayd"
expect_pass \
  "macos-tarball" \
  bash "${BUILD_SCRIPT}" --skip-build --target x86_64-apple-darwin --package-name ccbg-macos-x86_64-script-smoke

echo "[6/8] native package smoke script passes in current host configuration"
expect_pass \
  "native-smoke-pass" \
  bash "${SMOKE_SCRIPT}"

echo "[7/8] native package smoke still passes under restricted PATH on this host"
expect_pass \
  "native-smoke-restricted-path" \
  env PATH="/usr/bin:/bin" bash "${SMOKE_SCRIPT}"

echo "[8/8] native package smoke restores pre-existing target release directories"
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
