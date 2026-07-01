#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

RUST_HELPER="/opt/ccbg/bin/smb-sidecar-host"
DEBIAN_FRONTEND=noninteractive

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

ensure_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "ccbg-smb-sidecar.sh must run as root" >&2
    exit 1
  fi
}

install_missing_dependencies() {
  local missing=()
  local packages=()

  command_exists rclone || missing+=("rclone")
  command_exists smbd || missing+=("smbd")
  command_exists smbpasswd || missing+=("smbpasswd")
  command_exists fusermount || command_exists fusermount3 || missing+=("fusermount")

  if [ "${#missing[@]}" -eq 0 ]; then
    return 0
  fi

  if ! command_exists apt-get; then
    echo "missing required SMB sidecar dependencies: ${missing[*]}" >&2
    exit 1
  fi

  for item in "${missing[@]}"; do
    case "${item}" in
      rclone)
        packages+=("rclone")
        ;;
      smbd|smbpasswd)
        packages+=("samba")
        ;;
      fusermount)
        packages+=("fuse3")
        ;;
    esac
  done

  mapfile -t packages < <(printf '%s\n' "${packages[@]}" | awk 'NF && !seen[$0]++')
  if [ "${#packages[@]}" -eq 0 ]; then
    return 0
  fi

  apt-get update
  apt-get install -y "${packages[@]}"
}

main() {
  ensure_root
  local action="${1:-sync}"

  case "${action}" in
    sync|stop|status)
      install_missing_dependencies
      if [ -x "${RUST_HELPER}" ]; then
        exec "${RUST_HELPER}" "${action}"
      fi
      echo "missing SMB sidecar helper: ${RUST_HELPER}" >&2
      exit 1
      ;;
    *)
      echo "usage: $0 [sync|stop|status]" >&2
      exit 2
      ;;
  esac
}

main "$@"
