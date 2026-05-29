#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

heavy_deps_regex='(^| )(rusqlite|reqwest|tower-http|axum)( |$)'
profiles=(full-host lite-host esp-client esp-relay)

for profile in "${profiles[@]}"; do
  cargo test -p ccbg-platform-profiles --no-default-features --features "${profile}"
done

cargo check -p gatewayd --no-default-features --features full-host
cargo check -p gatewayd --no-default-features --features lite-host

for profile in esp-client esp-relay; do
  tree_output="$(cargo tree -p ccbg-platform-profiles --no-default-features --features "${profile}")"
  if printf '%s\n' "${tree_output}" | rg -q "${heavy_deps_regex}"; then
    printf '%s\n' "${tree_output}" >&2
    echo "ESP profile ${profile} unexpectedly pulls a heavy host dependency" >&2
    exit 1
  fi
done

echo "feature profile matrix passed"
