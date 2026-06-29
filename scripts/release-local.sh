#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${1:-}"
RELEASE_HOST_LABEL="${CCBG_RELEASE_HOST_LABEL:-local release host}"

usage() {
  echo "usage: scripts/release-local.sh <release-tag>"
}

if [ -z "${TAG}" ]; then
  usage >&2
  exit 2
fi

cd "${ROOT_DIR}"
PYTHON_BIN="$(bash scripts/resolve-python.sh)"

resolve_release_fingerprint() {
  if [ -n "${CCBG_RELEASE_FINGERPRINT:-}" ]; then
    printf '%s\n' "${CCBG_RELEASE_FINGERPRINT}"
    return 0
  fi
  "${PYTHON_BIN}" - <<'PY'
from pathlib import Path
import re

text = Path("crates/gatewayd/src/main.rs").read_text(encoding="utf-8", errors="replace")
match = re.search(r'const DEFAULT_RELEASE_FINGERPRINT: &str = "([^"]+)";', text)
if not match:
    raise SystemExit("failed to locate DEFAULT_RELEASE_FINGERPRINT in crates/gatewayd/src/main.rs")
print(match.group(1))
PY
}

if [ "${CCBG_RELEASE_ALLOW_DIRTY:-false}" != "true" ] && [ -n "$(git status --porcelain)" ]; then
  echo "working tree is dirty; commit changes or set CCBG_RELEASE_ALLOW_DIRTY=true" >&2
  exit 1
fi

if [ "${CCBG_RELEASE_SKIP_CHECKS:-false}" != "true" ]; then
  scripts/check-release-ready.sh
fi

OUT_DIR="${ROOT_DIR}/target/release-local/${TAG}"
LINUX_TARGET="${CCBG_RELEASE_LINUX_TARGET:-x86_64-unknown-linux-gnu}"
WINDOWS_TARGET="${CCBG_RELEASE_WINDOWS_TARGET:-x86_64-pc-windows-gnu}"
OPENWRT_TARGET="${CCBG_RELEASE_OPENWRT_TARGET:-aarch64-unknown-linux-musl}"
MACOS_X86_TARGET="${CCBG_RELEASE_MACOS_X86_TARGET:-x86_64-apple-darwin}"
MACOS_ARM64_TARGET="${CCBG_RELEASE_MACOS_ARM64_TARGET:-aarch64-apple-darwin}"
HOST_TRIPLE="$(rustc -vV | awk '/^host: / {print $2}')"

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

copy_artifacts() {
  local pattern
  shopt -s nullglob
  for pattern in "$@"; do
    for file in ${pattern}; do
      cp "${file}" "${OUT_DIR}/"
    done
  done
  shopt -u nullglob
}

copy_required_artifacts() {
  local file
  for file in "$@"; do
    if [ ! -e "${file}" ]; then
      echo "missing required artifact: ${file}" >&2
      exit 1
    fi
    cp "${file}" "${OUT_DIR}/"
  done
}

build_gatewayd() {
  local target="$1"
  if [ "${target}" = "${HOST_TRIPLE}" ]; then
    cargo build --release --locked --target "${target}" -p gatewayd
  else
    cargo zigbuild --release --locked --target "${target}" -p gatewayd
  fi
}

build_gatewayd_and_mcp() {
  local target="$1"
  if [ "${target}" = "${HOST_TRIPLE}" ]; then
    cargo build --release --locked --target "${target}" -p gatewayd -p mcp-server
  else
    cargo zigbuild --release --locked --target "${target}" -p gatewayd -p mcp-server
  fi
}

if [ -n "${CCBG_RELEASE_LXC_ASSET_DIR:-}" ]; then
  echo "copying external Linux LXC assets from ${CCBG_RELEASE_LXC_ASSET_DIR}"
  copy_required_artifacts \
    "${CCBG_RELEASE_LXC_ASSET_DIR}/ccbg-lxc-package.tar.gz" \
    "${CCBG_RELEASE_LXC_ASSET_DIR}/ccbg-lxc-package.tar.gz.sha256"
else
  echo "building Linux LXC package on ${RELEASE_HOST_LABEL} for ${LINUX_TARGET}"
  build_gatewayd "${LINUX_TARGET}"
  scripts/build-lxc-package.sh --skip-build --target "${LINUX_TARGET}"
  copy_required_artifacts \
    "${ROOT_DIR}/target/lxc-package/ccbg-lxc-package.tar.gz" \
    "${ROOT_DIR}/target/lxc-package/ccbg-lxc-package.tar.gz.sha256"
fi

if [ "${CCBG_RELEASE_BUILD_WINDOWS:-false}" = "true" ]; then
  echo "building Windows package on ${RELEASE_HOST_LABEL} for ${WINDOWS_TARGET}"
  build_gatewayd "${WINDOWS_TARGET}"
  scripts/build-native-package.sh \
    --skip-build \
    --target "${WINDOWS_TARGET}" \
    --package-name ccbg-windows-x86_64
  copy_artifacts "${ROOT_DIR}/target/native-packages/ccbg-windows-x86_64.zip"*
fi

if [ "${CCBG_RELEASE_BUILD_OPENWRT:-false}" = "true" ]; then
  echo "building OpenWrt lite package on ${RELEASE_HOST_LABEL} for ${OPENWRT_TARGET}"
  build_gatewayd_and_mcp "${OPENWRT_TARGET}"
  scripts/build-openwrt-lite-package.sh --skip-build --target "${OPENWRT_TARGET}"
  copy_artifacts "${ROOT_DIR}/target/openwrt-lite/ccbg-openwrt-lite.tar.gz"*
fi

if [ "${CCBG_RELEASE_BUILD_MACOS:-false}" = "true" ] && [ "${CCBG_RELEASE_ALLOW_LOCAL_MACOS_BUILD:-false}" != "true" ]; then
  cat >&2 <<'EOF'
macOS release artifacts are not built on the local/LAN release host right now.
Use the GitHub Actions release-assets workflow on the configured self-hosted
build-runner container, download its artifacts, and pass them back with
CCBG_RELEASE_MACOS_ASSET_DIR=/path/to/macos-assets.
Set CCBG_RELEASE_ALLOW_LOCAL_MACOS_BUILD=true only after a documented local/LAN
macOS builder or Darwin cross toolchain exists.
EOF
  exit 1
fi

if [ "${CCBG_RELEASE_BUILD_MACOS:-false}" = "true" ]; then
  echo "building community macOS x86_64 package locally for ${MACOS_X86_TARGET}"
  build_gatewayd "${MACOS_X86_TARGET}"
  scripts/build-native-package.sh \
    --skip-build \
    --target "${MACOS_X86_TARGET}" \
    --package-name ccbg-macos-x86_64
  copy_artifacts "${ROOT_DIR}/target/native-packages/ccbg-macos-x86_64.tar.gz"*

  echo "building community macOS arm64 package locally for ${MACOS_ARM64_TARGET}"
  build_gatewayd "${MACOS_ARM64_TARGET}"
  scripts/build-native-package.sh \
    --skip-build \
    --target "${MACOS_ARM64_TARGET}" \
    --package-name ccbg-macos-arm64
  copy_artifacts "${ROOT_DIR}/target/native-packages/ccbg-macos-arm64.tar.gz"*
fi

if [ -n "${CCBG_RELEASE_MACOS_ASSET_DIR:-}" ]; then
  echo "copying external macOS assets from ${CCBG_RELEASE_MACOS_ASSET_DIR}"
  copy_required_artifacts \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-x86_64.tar.gz" \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-x86_64.tar.gz.sha256" \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-arm64.tar.gz" \
    "${CCBG_RELEASE_MACOS_ASSET_DIR}/ccbg-macos-arm64.tar.gz.sha256"
fi

(
  cd "${OUT_DIR}"
  find . -maxdepth 1 -type f ! -name 'ccbg-checksums.txt' ! -name 'release-provenance.*' -printf '%f\n' \
    | LC_ALL=C sort \
    | xargs sha256sum > ccbg-checksums.txt
)

provenance_args=(
  "${PYTHON_BIN}" scripts/generate-release-provenance.py
  --release-name "CCBG ${TAG}"
  --tag "${TAG}"
  --fingerprint "$(resolve_release_fingerprint)"
  --out-dir "${OUT_DIR}"
  --build-step "scripts/check-release-ready.sh"
  --build-step "scripts/release-local.sh ${TAG}"
)

while IFS= read -r file; do
  provenance_args+=(--artifact "${OUT_DIR}/${file}")
done < <(find "${OUT_DIR}" -maxdepth 1 -type f ! -name 'release-provenance.*' -printf '%f\n' | LC_ALL=C sort)

"${provenance_args[@]}"

if [ "${CCBG_RELEASE_UPLOAD_GITHUB:-false}" = "true" ]; then
  GH_BIN="$(bash scripts/resolve-gh.sh)"
  if "${GH_BIN}" release view "${TAG}" >/dev/null 2>&1; then
    "${GH_BIN}" release upload "${TAG}" "${OUT_DIR}"/* --clobber
  else
    "${GH_BIN}" release create "${TAG}" "${OUT_DIR}"/* --title "${TAG}" --notes "CCBG local release ${TAG}"
  fi
fi

echo "${OUT_DIR}"
