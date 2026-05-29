#!/bin/sh
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -eu

ENV_FILE="${CCBG_ENV_FILE:-/etc/ccbg/openwrt-lite.env}"
if [ -f "$ENV_FILE" ]; then
	set -a
	# shellcheck disable=SC1090
	. "$ENV_FILE"
	set +a
fi

endpoint="${CCBG_SMOKE_ENDPOINT:-http://127.0.0.1:61080}"
metrics_endpoint="${CCBG_SMOKE_METRICS_ENDPOINT:-http://127.0.0.1:61083}"
api_key="${CCBG_SMOKE_CONTROL_API_KEY:-${CCBG_CONTROL_API_KEY:-}}"

fetch() {
	url="$1"
	header="$2"
	if command -v curl >/dev/null 2>&1; then
		if [ -n "$header" ]; then
			curl -fsS --max-time 5 -H "$header" "$url" >/dev/null
		else
			curl -fsS --max-time 5 "$url" >/dev/null
		fi
	else
		if [ -n "$header" ]; then
			wget -q -T 5 --header "$header" -O /dev/null "$url"
		else
			wget -q -T 5 -O /dev/null "$url"
		fi
	fi
}

fetch "$endpoint/healthz" ""
if [ -n "$api_key" ]; then
	fetch "$metrics_endpoint/readyz" "x-api-key: $api_key"
else
	echo "skipping metrics readyz smoke because CCBG_CONTROL_API_KEY is empty"
fi

if command -v gatewayd >/dev/null 2>&1; then
	gatewayd --version >/dev/null
fi

if command -v mcp-server >/dev/null 2>&1; then
	printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}\n' | mcp-server | grep -q '"protocolVersion":"2025-03-26"'
fi

echo "ccbg OpenWRT lite smoke passed: $endpoint"
