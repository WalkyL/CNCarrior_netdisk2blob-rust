#!/bin/sh
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -eu

PACKAGE_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
TIMESTAMP="$(date +%Y%m%d%H%M%S)"
START_SERVICE=1

for arg in "$@"; do
	case "$arg" in
		--no-start)
			START_SERVICE=0
			;;
		-h|--help)
			echo "usage: install.sh [--no-start]"
			exit 0
			;;
		*)
			echo "unknown argument: $arg" >&2
			exit 2
			;;
	esac
done

if [ "$(id -u)" -ne 0 ]; then
	echo "install.sh must run as root on the OpenWRT host" >&2
	exit 1
fi

install -d -m 0755 /etc/ccbg /etc/ccbg/config /etc/ccbg/scripts /usr/lib/ccbg/assets/admin /overlay/ccbg /overlay/ccbg/provider-credentials /tmp/ccbg-spool
install -m 0755 "$PACKAGE_ROOT/bin/gatewayd" /usr/bin/gatewayd
if [ -x "$PACKAGE_ROOT/bin/mcp-server" ]; then
	install -m 0755 "$PACKAGE_ROOT/bin/mcp-server" /usr/bin/mcp-server
fi
install -m 0644 "$PACKAGE_ROOT/assets/admin/index.html" /usr/lib/ccbg/assets/admin/index.html
cp -R "$PACKAGE_ROOT/config/." /etc/ccbg/config/
install -m 0755 "$PACKAGE_ROOT/scripts/smoke.sh" /etc/ccbg/scripts/smoke.sh
install -m 0755 "$PACKAGE_ROOT/init.d/ccbg" /etc/init.d/ccbg

if [ ! -f /etc/ccbg/openwrt-lite.env ]; then
	install -m 0600 "$PACKAGE_ROOT/etc/openwrt-lite.env" /etc/ccbg/openwrt-lite.env
else
	install -m 0600 "$PACKAGE_ROOT/etc/openwrt-lite.env" "/etc/ccbg/openwrt-lite.env.package-$TIMESTAMP"
	echo "kept existing /etc/ccbg/openwrt-lite.env; new sample written to /etc/ccbg/openwrt-lite.env.package-$TIMESTAMP"
fi

chmod 0700 /overlay/ccbg /overlay/ccbg/provider-credentials /tmp/ccbg-spool

# Install the LuCI plugin (menu + ACL + JS view). This does NOT restart
# uhttpd or rpcd: LuCI refreshes its menu/ACL cache on log-out / log-in
# (official LuCI behavior). gatewayd restart is handled below per user's
# accepted boundary.
LUCI_PLUGIN_ROOT="${PACKAGE_ROOT}/luci-app-ccbg"
if [ -d "${LUCI_PLUGIN_ROOT}" ]; then
	install -d -m 0755 \
		/usr/share/luci/menu.d \
		/usr/share/rpcd/acl.d \
		/www/luci-static/resources/view/ccbg
	if [ -f "${LUCI_PLUGIN_ROOT}/root/usr/share/luci/menu.d/luci-app-ccbg.json" ]; then
		install -m 0644 "${LUCI_PLUGIN_ROOT}/root/usr/share/luci/menu.d/luci-app-ccbg.json" /usr/share/luci/menu.d/luci-app-ccbg.json
	fi
	if [ -f "${LUCI_PLUGIN_ROOT}/root/usr/share/rpcd/acl.d/luci-app-ccbg.json" ]; then
		install -m 0644 "${LUCI_PLUGIN_ROOT}/root/usr/share/rpcd/acl.d/luci-app-ccbg.json" /usr/share/rpcd/acl.d/luci-app-ccbg.json
	fi
	if [ -f "${LUCI_PLUGIN_ROOT}/htdocs/luci-static/resources/view/ccbg/admin.js" ]; then
		install -m 0644 "${LUCI_PLUGIN_ROOT}/htdocs/luci-static/resources/view/ccbg/admin.js" /www/luci-static/resources/view/ccbg/admin.js
	fi
	echo "ccbg LuCI plugin files installed (log out / log in to LuCI to refresh)"
fi

if [ "$START_SERVICE" -eq 1 ]; then
	/etc/init.d/ccbg enable
	/etc/init.d/ccbg restart
fi

echo "ccbg OpenWRT lite install complete"
