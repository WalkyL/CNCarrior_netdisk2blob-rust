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

if [ "$START_SERVICE" -eq 1 ]; then
	/etc/init.d/ccbg enable
	/etc/init.d/ccbg restart
fi

echo "ccbg OpenWRT lite install complete"
