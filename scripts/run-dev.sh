#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f .env.local ]; then
  set -a
  # shellcheck disable=SC1091
  . ./.env.local
  set +a
fi

export CCBG_PRIMARY_PROVIDER="${CCBG_PRIMARY_PROVIDER:-${CCBG_PROVIDER:-unicom}}"
export CCBG_BIND_ADDR="${CCBG_BIND_ADDR:-127.0.0.1:61080}"
export RUST_LOG="${RUST_LOG:-info,gatewayd=debug,provider_unicom=debug,provider_telecom=debug,provider_mobile=debug,provider_onedrive=debug,policy_engine=debug,replication_engine=debug}"

cargo run -p gatewayd
