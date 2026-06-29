#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

for candidate in \
  cargo \
  cargo.exe \
  /mnt/d/Rust/cargo/bin/cargo.exe \
  /mnt/c/Users/*/.cargo/bin/cargo.exe \
  /c/Users/*/.cargo/bin/cargo.exe
do
  if command -v "${candidate}" >/dev/null 2>&1; then
    command -v "${candidate}"
    exit 0
  fi
  if [ -x "${candidate}" ]; then
    printf '%s\n' "${candidate}"
    exit 0
  fi
done

echo "missing cargo; install Rust or add cargo to PATH" >&2
exit 1
