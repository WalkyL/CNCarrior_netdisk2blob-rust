#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${ROOT_DIR}/target/lxc-package"
PACKAGE_NAME="${CCBG_LXC_PACKAGE_NAME:-ccbg-lxc-package}"
TARGET="${CCBG_LXC_TARGET:-}"
BINARY_OVERRIDE="${CCBG_LXC_BINARY:-}"
HELPER_BINARY_OVERRIDE="${CCBG_LXC_SMB_SIDECAR_BINARY:-}"
CARGO_BIN="$(bash "${ROOT_DIR}/scripts/resolve-cargo.sh")"
GIT_BIN="$(bash "${ROOT_DIR}/scripts/resolve-git.sh")"
SKIP_BUILD=false

usage() {
  echo "usage: scripts/build-lxc-package.sh [--target <rust-target>] [--binary <linux-gatewayd>] [--helper-binary <linux-smb-sidecar-host>] [--skip-build]"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --target)
      if [ "$#" -lt 2 ]; then
        echo "--target requires a Rust target triple" >&2
        exit 2
      fi
      TARGET="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=true
      shift
      ;;
    --binary)
      if [ "$#" -lt 2 ]; then
        echo "--binary requires a Linux gatewayd path" >&2
        exit 2
      fi
      BINARY_OVERRIDE="$2"
      SKIP_BUILD=true
      shift 2
      ;;
    --helper-binary)
      if [ "$#" -lt 2 ]; then
        echo "--helper-binary requires a Linux smb-sidecar-host path" >&2
        exit 2
      fi
      HELPER_BINARY_OVERRIDE="$2"
      SKIP_BUILD=true
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ "${SKIP_BUILD}" != true ]; then
  if [ -n "${TARGET}" ]; then
    "${CARGO_BIN}" build --release --target "${TARGET}" -p gatewayd -p smb-sidecar-host
  else
    "${CARGO_BIN}" build --release -p gatewayd -p smb-sidecar-host
  fi
fi

if [ -n "${BINARY_OVERRIDE}" ]; then
  BINARY="${BINARY_OVERRIDE}"
elif [ -n "${TARGET}" ]; then
  BINARY="${ROOT_DIR}/target/${TARGET}/release/gatewayd"
else
  BINARY="${ROOT_DIR}/target/release/gatewayd"
fi

if [ -n "${HELPER_BINARY_OVERRIDE}" ]; then
  HELPER_BINARY="${HELPER_BINARY_OVERRIDE}"
elif [ -n "${TARGET}" ]; then
  HELPER_BINARY="${ROOT_DIR}/target/${TARGET}/release/smb-sidecar-host"
else
  HELPER_BINARY="${ROOT_DIR}/target/release/smb-sidecar-host"
fi

if [ ! -s "${BINARY}" ]; then
  echo "missing release binary: ${BINARY}" >&2
  echo "run without --skip-build or build gatewayd first" >&2
  exit 1
fi

if [ ! -s "${HELPER_BINARY}" ]; then
  echo "missing smb-sidecar-host binary: ${HELPER_BINARY}" >&2
  echo "run without --skip-build or build smb-sidecar-host first" >&2
  exit 1
fi

if command -v file >/dev/null 2>&1; then
  binary_kind="$(file -b "${BINARY}")"
  case "${binary_kind}" in
    *ELF*)
      ;;
    *)
      echo "LXC package requires a Linux ELF gatewayd binary, got: ${binary_kind}" >&2
      if [ -z "${TARGET}" ] && [ -z "${BINARY_OVERRIDE}" ]; then
        echo "on Windows, pass --target <linux-target> or --binary <linux-gatewayd>" >&2
        echo "recommended: scripts/build-linux-release-in-podman.sh --target x86_64-unknown-linux-gnu --package gatewayd --package smb-sidecar-host" >&2
      fi
      exit 1
      ;;
  esac

  helper_kind="$(file -b "${HELPER_BINARY}")"
  case "${helper_kind}" in
    *ELF*)
      ;;
    *)
      echo "LXC package requires a Linux ELF smb-sidecar-host binary, got: ${helper_kind}" >&2
      if [ -z "${TARGET}" ] && [ -z "${HELPER_BINARY_OVERRIDE}" ]; then
        echo "on Windows, pass --target <linux-target> or --helper-binary <linux-smb-sidecar-host>" >&2
      fi
      exit 1
      ;;
  esac
fi

rm -rf "${DIST_DIR}"
mkdir -p \
  "${DIST_DIR}/${PACKAGE_NAME}/bin" \
  "${DIST_DIR}/${PACKAGE_NAME}/assets/admin" \
  "${DIST_DIR}/${PACKAGE_NAME}/config" \
  "${DIST_DIR}/${PACKAGE_NAME}/etc" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts" \
  "${DIST_DIR}/${PACKAGE_NAME}/systemd" \
  "${DIST_DIR}/${PACKAGE_NAME}/docs"

install -m 0755 "${BINARY}" "${DIST_DIR}/${PACKAGE_NAME}/bin/gatewayd"
install -m 0644 "${ROOT_DIR}/crates/gatewayd/assets/admin/index.html" "${DIST_DIR}/${PACKAGE_NAME}/assets/admin/index.html"
cp -R "${ROOT_DIR}/config/." "${DIST_DIR}/${PACKAGE_NAME}/config/"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg.env" "${DIST_DIR}/${PACKAGE_NAME}/etc/ccbg.env"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg.service" "${DIST_DIR}/${PACKAGE_NAME}/systemd/ccbg.service"
install -m 0755 "${ROOT_DIR}/deploy/lxc/install.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/install.sh"
install -m 0755 "${ROOT_DIR}/deploy/lxc/rollback.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/rollback.sh"
install -m 0755 "${ROOT_DIR}/deploy/lxc/smoke.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/smoke.sh"
install -m 0755 "${ROOT_DIR}/deploy/lxc/ccbg-smb-sidecar.sh" "${DIST_DIR}/${PACKAGE_NAME}/scripts/ccbg-smb-sidecar.sh"
install -m 0755 "${HELPER_BINARY}" "${DIST_DIR}/${PACKAGE_NAME}/bin/smb-sidecar-host"
install -m 0644 "${ROOT_DIR}/docs/pve-lxc-deployment.md" "${DIST_DIR}/${PACKAGE_NAME}/docs/pve-lxc-deployment.md"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg-smb-sidecar-sync.service" "${DIST_DIR}/${PACKAGE_NAME}/systemd/ccbg-smb-sidecar-sync.service"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg-smb-sidecar.path" "${DIST_DIR}/${PACKAGE_NAME}/systemd/ccbg-smb-sidecar.path"
install -m 0644 "${ROOT_DIR}/deploy/lxc/ccbg-smb-sidecar.timer" "${DIST_DIR}/${PACKAGE_NAME}/systemd/ccbg-smb-sidecar.timer"

# The release build host is Windows. Normalize packaged shell scripts to LF so
# Linux guests don't see `/usr/bin/env: 'bash\r'`.
sed -i 's/\r$//' \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts/install.sh" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts/rollback.sh" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts/smoke.sh" \
  "${DIST_DIR}/${PACKAGE_NAME}/scripts/ccbg-smb-sidecar.sh"

{
  echo "package=${PACKAGE_NAME}"
  echo "rust_target=${TARGET:-host}"
  echo "built_at_unix=$(date +%s)"
  echo "gatewayd_sha256=$(sha256sum "${BINARY}" | awk '{print $1}')"
  echo "smb_sidecar_host_sha256=$(sha256sum "${HELPER_BINARY}" | awk '{print $1}')"
  "${GIT_BIN}" -C "${ROOT_DIR}" rev-parse HEAD 2>/dev/null | sed 's/^/git_commit=/' || true
} > "${DIST_DIR}/${PACKAGE_NAME}/PACKAGE-METADATA"

(
  cd "${DIST_DIR}/${PACKAGE_NAME}"
  find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256
)

tarball="${DIST_DIR}/${PACKAGE_NAME}.tar.gz"
tar --sort=name --owner=0 --group=0 --numeric-owner -C "${DIST_DIR}" -czf "${tarball}" "${PACKAGE_NAME}"
sha256sum "${tarball}" > "${tarball}.sha256"
echo "${tarball}"
cat "${tarball}.sha256"
