#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

PACKAGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TIMESTAMP="$(date +%Y%m%d%H%M%S)"

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "install.sh must run as root inside the LXC guest" >&2
    exit 1
  fi
}

install_user() {
  if ! getent group ccbg >/dev/null; then
    groupadd --system ccbg
  fi
  if ! id ccbg >/dev/null 2>&1; then
    useradd --system --gid ccbg --home-dir /var/lib/ccbg --shell /usr/sbin/nologin ccbg
  fi
}

install_tree() {
  install -d -m 0755 /opt/ccbg/bin /opt/ccbg/assets/admin /etc/ccbg /etc/ccbg/config /var/lib/ccbg /var/lib/ccbg/body-spool /var/lib/ccbg/provider-credentials /var/log/ccbg /opt/ccbg/backups
  if [ -x /opt/ccbg/bin/gatewayd ]; then
    old_sha="$(sha256sum /opt/ccbg/bin/gatewayd | awk '{print $1}')"
    cp /opt/ccbg/bin/gatewayd "/opt/ccbg/backups/gatewayd.${old_sha}.${TIMESTAMP}"
  fi
  install -m 0755 "${PACKAGE_ROOT}/bin/gatewayd" /opt/ccbg/bin/gatewayd
  install -m 0644 "${PACKAGE_ROOT}/assets/admin/index.html" /opt/ccbg/assets/admin/index.html
  cp -R "${PACKAGE_ROOT}/config/." /etc/ccbg/config/
  install -m 0644 "${PACKAGE_ROOT}/systemd/ccbg.service" /etc/systemd/system/ccbg.service

  if [ ! -f /etc/ccbg/ccbg.env ]; then
    install -m 0640 "${PACKAGE_ROOT}/etc/ccbg.env" /etc/ccbg/ccbg.env
  else
    install -m 0640 "${PACKAGE_ROOT}/etc/ccbg.env" "/etc/ccbg/ccbg.env.package-${TIMESTAMP}"
    echo "kept existing /etc/ccbg/ccbg.env; new sample written to /etc/ccbg/ccbg.env.package-${TIMESTAMP}"
  fi

  chown -R ccbg:ccbg /var/lib/ccbg /var/log/ccbg
  chown -R root:ccbg /etc/ccbg
  chmod 0750 /var/lib/ccbg /var/lib/ccbg/provider-credentials /var/lib/ccbg/body-spool
  chmod 0750 /var/log/ccbg
}

start_service() {
  systemctl daemon-reload
  systemctl enable --now ccbg.service
  systemctl --no-pager --full status ccbg.service || true
}

require_root
install_user
install_tree
start_service
echo "ccbg LXC install complete"
