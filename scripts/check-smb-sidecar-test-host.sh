#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

mode="${1:-strict}"

usage() {
  cat <<'EOF'
usage: scripts/check-smb-sidecar-test-host.sh [strict|runtime|baseline]

Modes:
  baseline  - Linux host suitable for baseline smb-sidecar-host integration checks
  runtime   - Linux root host with smb/rclone runtime dependencies for runtime/recovery checks
  strict    - Linux root host with runtime deps, systemd-run, and /dev/fuse for full strict-mode coverage
EOF
}

case "${mode}" in
  strict|runtime|baseline)
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    echo "unknown mode: ${mode}" >&2
    usage >&2
    exit 2
    ;;
esac

failures=0

check_linux() {
  if [ "$(uname -s)" != "Linux" ]; then
    echo "FAIL: host must be Linux" >&2
    failures=$((failures + 1))
  else
    echo "OK: Linux host"
  fi
}

check_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "FAIL: host must run as root" >&2
    failures=$((failures + 1))
  else
    echo "OK: root user"
  fi
}

check_command() {
  local name="$1"
  if command -v "${name}" >/dev/null 2>&1; then
    echo "OK: command ${name}"
  else
    echo "FAIL: missing command ${name}" >&2
    failures=$((failures + 1))
  fi
}

check_path_exists() {
  local path="$1"
  if [ -e "${path}" ]; then
    echo "OK: ${path} present"
  else
    echo "FAIL: missing required path ${path}" >&2
    failures=$((failures + 1))
  fi
}

check_linux

case "${mode}" in
  baseline)
    ;;
  runtime|strict)
    check_root
    check_command python3
    check_command rclone
    check_command smbd
    check_command smbpasswd
    check_command mountpoint
    ;;
esac

if [ "${mode}" = "strict" ]; then
  check_command systemd-run
  check_command systemctl
  check_command fusermount3
  check_path_exists /dev/fuse
fi

if [ "${failures}" -gt 0 ]; then
  echo "smb-sidecar-host test host check failed: ${failures} requirement(s) missing" >&2
  exit 1
fi

echo "smb-sidecar-host test host check passed for mode=${mode}"
