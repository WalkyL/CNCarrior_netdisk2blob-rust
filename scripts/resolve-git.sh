#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

for candidate in \
  git \
  git.exe \
  /mnt/d/Git/cmd/git.exe \
  /mnt/c/Program\ Files/Git/cmd/git.exe \
  /c/Program\ Files/Git/cmd/git.exe
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

echo "missing git; install Git or add git to PATH" >&2
exit 1
