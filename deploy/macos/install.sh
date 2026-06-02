#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

PACKAGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LABEL="online.agi2030.ccbg.gatewayd"
INSTALL_DIR="${CCBG_INSTALL_DIR:-${HOME}/.local/ccbg}"
CONFIG_DIR="${CCBG_CONFIG_DIR:-${HOME}/Library/Application Support/ccbg/config}"
DATA_DIR="${CCBG_DATA_DIR:-${HOME}/Library/Application Support/ccbg/data}"
LOG_DIR="${CCBG_LOG_DIR:-${HOME}/Library/Logs/ccbg}"
PLIST_DIR="${HOME}/Library/LaunchAgents"
PLIST_PATH="${PLIST_DIR}/${LABEL}.plist"

if [ ! -x "${PACKAGE_ROOT}/bin/gatewayd" ]; then
  echo "missing ${PACKAGE_ROOT}/bin/gatewayd" >&2
  exit 1
fi

mkdir -p "${INSTALL_DIR}/bin" "${INSTALL_DIR}/assets/admin" "${CONFIG_DIR}" "${DATA_DIR}" "${LOG_DIR}" "${PLIST_DIR}"
install -m 0755 "${PACKAGE_ROOT}/bin/gatewayd" "${INSTALL_DIR}/bin/gatewayd"
install -m 0644 "${PACKAGE_ROOT}/assets/admin/index.html" "${INSTALL_DIR}/assets/admin/index.html"
cp -R "${PACKAGE_ROOT}/config/." "${CONFIG_DIR}/"

sed \
  -e "s#__INSTALL_DIR__#${INSTALL_DIR}#g" \
  -e "s#__CONFIG_DIR__#${CONFIG_DIR}#g" \
  -e "s#__DATA_DIR__#${DATA_DIR}#g" \
  -e "s#__LOG_DIR__#${LOG_DIR}#g" \
  "${PACKAGE_ROOT}/deploy/macos/${LABEL}.plist.template" > "${PLIST_PATH}"

launchctl bootout "gui/${UID}" "${PLIST_PATH}" >/dev/null 2>&1 || true
launchctl bootstrap "gui/${UID}" "${PLIST_PATH}"
launchctl enable "gui/${UID}/${LABEL}"
launchctl kickstart -k "gui/${UID}/${LABEL}"

echo "installed ${LABEL}"
echo "health: curl -fsS http://127.0.0.1:61080/healthz"
echo "admin:  http://<this-host-ip>:61081/"
