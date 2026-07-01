#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${CCBG_PODMAN_BUILD_IMAGE:-localhost/product-build-runner:latest}"
TARGET="${CCBG_LINUX_TARGET:-x86_64-unknown-linux-gnu}"
PACKAGES=()
CONTAINER_WORKDIR="/workspace"

if [ -n "${CCBG_LINUX_BUILD_PACKAGE:-}" ]; then
  PACKAGES=("${CCBG_LINUX_BUILD_PACKAGE}")
else
  PACKAGES=(gatewayd)
fi

usage() {
  cat <<'EOF'
usage: scripts/build-linux-release-in-podman.sh [--target <rust-target>] [--package <cargo-package>]... [--image <podman-image>]

Build a Linux release binary from a Windows host by running cargo inside the
local Podman build-runner image. The resulting ELF is written to:

  target/<rust-target>/release/<binary>

Repeat `--package` to build multiple binaries in one run.

Default image:
  localhost/product-build-runner:latest
EOF
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
    --package)
      if [ "$#" -lt 2 ]; then
        echo "--package requires a cargo package name" >&2
        exit 2
      fi
      if [ "${#PACKAGES[@]}" -eq 1 ] && [ "${PACKAGES[0]}" = "${CCBG_LINUX_BUILD_PACKAGE:-gatewayd}" ]; then
        PACKAGES=()
      fi
      PACKAGES+=("$2")
      shift 2
      ;;
    --image)
      if [ "$#" -lt 2 ]; then
        echo "--image requires a Podman image reference" >&2
        exit 2
      fi
      IMAGE="$2"
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

if ! command -v podman >/dev/null 2>&1; then
  echo "missing podman; install Podman or build the Linux ELF on another Linux-capable host" >&2
  exit 1
fi

if ! podman image exists "${IMAGE}"; then
  echo "missing Podman build image: ${IMAGE}" >&2
  echo "build or load the local build-runner image before using this script" >&2
  exit 1
fi

workspace_mount="${ROOT_DIR}"
if command -v cygpath >/dev/null 2>&1; then
  workspace_mount="$(cygpath -w "${ROOT_DIR}")"
fi

build_args=()
for package in "${PACKAGES[@]}"; do
  build_args+=( -p "${package}" )
done

podman run --rm \
  -v "${workspace_mount}:${CONTAINER_WORKDIR}" \
  -w "${CONTAINER_WORKDIR}" \
  "${IMAGE}" \
  bash -lc "cargo build --release --locked --target '${TARGET}' ${build_args[*]}"

for package in "${PACKAGES[@]}"; do
  binary_path="${ROOT_DIR}/target/${TARGET}/release/${package}"
  if [ ! -s "${binary_path}" ]; then
    echo "expected output was not produced: ${binary_path}" >&2
    exit 1
  fi

  if command -v file >/dev/null 2>&1; then
    file -b "${binary_path}"
  fi
  sha256sum "${binary_path}"
done
