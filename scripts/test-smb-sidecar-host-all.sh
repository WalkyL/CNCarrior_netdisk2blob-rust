#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
skipped_stage_count=0

run_stage() {
  local label="$1"
  shift
  echo "==> ${label}"
  "$@"
}

skip_stage() {
  local label="$1"
  local reason="$2"
  skipped_stage_count=$((skipped_stage_count + 1))
  echo "==> SKIP ${label}: ${reason}"
}

is_linux() {
  [ "$(uname -s)" = "Linux" ]
}

is_root() {
  [ "$(id -u)" -eq 0 ]
}

has_command() {
  command -v "$1" >/dev/null 2>&1
}

has_runtime_stack() {
  has_command python3 \
    && has_command rclone \
    && has_command smbd \
    && has_command smbpasswd \
    && has_command mountpoint
}

has_systemd_stack() {
  has_command systemd-run \
    && has_command systemctl \
    && has_command fusermount3 \
    && [ -e /dev/fuse ]
}

if [ "${SMB_SIDECAR_SKIP_UNIT:-0}" != "1" ]; then
  run_stage "unit" cargo test -p smb-sidecar-host --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

if is_linux; then
  run_stage "baseline integration" "${ROOT_DIR}/scripts/test-smb-sidecar-host-integration.sh"
else
  skip_stage "baseline integration" "requires Linux"
fi

if is_linux && is_root && has_runtime_stack; then
  run_stage "runtime integration" "${ROOT_DIR}/scripts/test-smb-sidecar-host-runtime.sh"
  run_stage "recovery integration" "${ROOT_DIR}/scripts/test-smb-sidecar-host-recovery.sh"
else
  if ! is_linux; then
    skip_stage "runtime integration" "requires Linux"
    skip_stage "recovery integration" "requires Linux"
  elif ! is_root; then
    skip_stage "runtime integration" "requires root"
    skip_stage "recovery integration" "requires root"
  else
    skip_stage "runtime integration" "requires python3+rclone+smbd+smbpasswd+mountpoint"
    skip_stage "recovery integration" "requires python3+rclone+smbd+smbpasswd+mountpoint"
  fi
fi

if is_linux && is_root && has_runtime_stack && has_systemd_stack; then
  run_stage "systemd running integration" "${ROOT_DIR}/scripts/test-smb-sidecar-host-systemd-running.sh"
else
  if ! is_linux; then
    skip_stage "systemd running integration" "requires Linux"
  elif ! is_root; then
    skip_stage "systemd running integration" "requires root"
  elif ! has_runtime_stack; then
    skip_stage "systemd running integration" "requires python3+rclone+smbd+smbpasswd+mountpoint"
  else
    skip_stage "systemd running integration" "requires systemd-run+systemctl+fusermount3+/dev/fuse"
  fi
fi

if [ "${SMB_SIDECAR_TEST_REQUIRE_FULL:-0}" = "1" ] && [ "${skipped_stage_count}" -gt 0 ]; then
  echo "smb-sidecar-host aggregated test run skipped ${skipped_stage_count} stage(s) under SMB_SIDECAR_TEST_REQUIRE_FULL=1" >&2
  exit 1
fi

echo "smb-sidecar-host aggregated test run completed"
