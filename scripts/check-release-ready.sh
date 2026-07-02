#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"
PYTHON_BIN="$(bash scripts/resolve-python.sh)"
CARGO_BIN="$(bash scripts/resolve-cargo.sh)"
GIT_BIN="$(bash scripts/resolve-git.sh)"

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

run "${CARGO_BIN}" fmt --all --check
run "${PYTHON_BIN}" scripts/license-check.py
run "${CARGO_BIN}" test --workspace
run "${PYTHON_BIN}" scripts/catalog-lint.py
run "${PYTHON_BIN}" scripts/check-cloudflare-public-fingerprint.py
run "${PYTHON_BIN}" scripts/check-onedrive-parking.py
run "${PYTHON_BIN}" scripts/check-onedrive-restore-checklist.py
run "${PYTHON_BIN}" scripts/check-backup-restore-drill.py \
  --drill-root target/backup-restore-drill-release \
  --write-sample
run env SMB_SIDECAR_SKIP_UNIT=1 scripts/test-smb-sidecar-host-all.sh
run scripts/check-native-package-smoke.sh
run "${PYTHON_BIN}" scripts/s3-smoke.py

if [ "${CCBG_CHECK_NATIVE_PACKAGE_SMOKE:-false}" = "true" ]; then
  run "${CARGO_BIN}" build --release --locked -p gatewayd
  run scripts/build-native-package.sh \
    --skip-build \
    --target x86_64-unknown-linux-gnu \
    --package-name ccbg-native-linux-smoke
fi

run "${GIT_BIN}" diff --check
