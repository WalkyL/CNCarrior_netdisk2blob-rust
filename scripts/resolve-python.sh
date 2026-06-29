#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

is_usable_python() {
  local candidate="$1"
  "${candidate}" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)' >/dev/null 2>&1
}

emit_if_usable() {
  local candidate="$1"
  if [ -z "${candidate}" ]; then
    return 1
  fi
  if is_usable_python "${candidate}"; then
    printf '%s\n' "${candidate}"
    exit 0
  fi
  return 1
}

for candidate in python3 python; do
  if command -v "${candidate}" >/dev/null 2>&1; then
    if emit_if_usable "$(command -v "${candidate}")"; then
      exit 0
    fi
  fi
done

for candidate in \
  /c/Python*/python \
  /c/Python*/python.exe \
  /c/Users/*/AppData/Local/Programs/Python/Python*/python \
  /c/Users/*/AppData/Local/Programs/Python/Python*/python.exe \
  /mnt/c/Python*/python.exe \
  /mnt/c/Python*/python \
  /mnt/c/Users/*/AppData/Local/Programs/Python/Python*/python.exe \
  /mnt/c/Users/*/AppData/Local/Programs/Python/Python*/python \
  /c/Python*/python.exe \
  /c/Users/*/AppData/Local/Programs/Python/Python*/python.exe
do
  if [ -x "${candidate}" ]; then
    if emit_if_usable "${candidate}"; then
      exit 0
    fi
  fi
done

echo "missing Python >= 3.10; install Python on the build host or add it to PATH" >&2
exit 1
