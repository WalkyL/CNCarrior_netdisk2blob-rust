#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run cargo fmt --all --check
run python3 scripts/license-check.py
run cargo test --workspace
run python3 scripts/catalog-lint.py
run python3 scripts/check-cloudflare-public-fingerprint.py
run python3 scripts/check-onedrive-parking.py
run python3 scripts/check-onedrive-restore-checklist.py
run python3 scripts/check-backup-restore-drill.py \
  --drill-root target/backup-restore-drill-release \
  --write-sample
run python3 scripts/s3-smoke.py

if [ "${CCBG_CHECK_NATIVE_PACKAGE_SMOKE:-false}" = "true" ]; then
  run cargo build --release --locked -p gatewayd
  run scripts/build-native-package.sh \
    --skip-build \
    --target x86_64-unknown-linux-gnu \
    --package-name ccbg-native-linux-smoke
fi

run git diff --check
