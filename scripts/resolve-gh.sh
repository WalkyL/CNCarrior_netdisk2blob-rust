#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

for candidate in \
  gh \
  /c/Program\ Files/GitHub\ CLI/gh.exe
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

echo "missing GitHub CLI; install gh or add it to PATH" >&2
exit 1
