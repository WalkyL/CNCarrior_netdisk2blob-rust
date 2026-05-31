#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

for candidate in \
  python3 \
  python \
  /c/Users/walky/AppData/Local/Programs/Python/Python313/python
do
  if command -v "${candidate}" >/dev/null 2>&1 && "${candidate}" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)' >/dev/null 2>&1; then
    command -v "${candidate}"
    exit 0
  fi
done

echo "missing Python >= 3.10; install Python on the build host or add it to PATH" >&2
exit 1
