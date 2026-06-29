#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

for candidate in \
  rustc \
  rustc.exe \
  /mnt/d/Rust/cargo/bin/rustc.exe \
  /mnt/c/Users/*/.cargo/bin/rustc.exe \
  /c/Users/*/.cargo/bin/rustc.exe
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

echo "missing rustc; install Rust or add rustc to PATH" >&2
exit 1
