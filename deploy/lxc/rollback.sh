#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

BACKUP_PATH="${1:-}"
if [ "$(id -u)" -ne 0 ]; then
  echo "rollback.sh must run as root inside the LXC guest" >&2
  exit 1
fi
if [ -z "${BACKUP_PATH}" ]; then
  BACKUP_PATH="$(ls -1t /opt/ccbg/backups/gatewayd.* 2>/dev/null | head -1 || true)"
fi
if [ -z "${BACKUP_PATH}" ] || [ ! -f "${BACKUP_PATH}" ]; then
  echo "no gatewayd backup found; pass an explicit backup path" >&2
  exit 1
fi

systemctl stop ccbg.service || true
install -m 0755 "${BACKUP_PATH}" /opt/ccbg/bin/gatewayd
systemctl start ccbg.service
systemctl --no-pager --full status ccbg.service || true
echo "rolled back gatewayd from ${BACKUP_PATH}"
