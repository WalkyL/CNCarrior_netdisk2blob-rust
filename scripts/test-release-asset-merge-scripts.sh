#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${TMPDIR:-/tmp}/ccbg-release-asset-merge-tests"
RELEASE_LOCAL_SCRIPT="${ROOT_DIR}/scripts/release-local.sh"
DOWNLOAD_SCRIPT="${ROOT_DIR}/scripts/download-build-runner-release-assets.sh"

cleanup() {
  rm -rf "${WORK_DIR}"
  rm -rf "${ROOT_DIR}/target/release-local/test-release-asset-merge" \
         "${ROOT_DIR}/target/download-build-runner-assets-test"
}
trap cleanup EXIT

rm -rf "${WORK_DIR}"
mkdir -p "${WORK_DIR}"

expect_fail() {
  local label="$1"
  local expected="$2"
  shift 2
  local log_file="${WORK_DIR}/$(printf '%s' "${label}" | tr ' /:' '___').log"
  if "$@" >"${log_file}" 2>&1; then
    echo "expected failure: ${label}" >&2
    cat "${log_file}" >&2
    exit 1
  fi
  if ! grep -F -- "${expected}" "${log_file}" >/dev/null; then
    echo "missing expected error text for: ${label}" >&2
    cat "${log_file}" >&2
    exit 1
  fi
}

expect_pass() {
  local label="$1"
  shift
  local log_file="${WORK_DIR}/$(printf '%s' "${label}" | tr ' /:' '___').log"
  if ! "$@" >"${log_file}" 2>&1; then
    echo "expected success: ${label}" >&2
    cat "${log_file}" >&2
    exit 1
  fi
}

require_file() {
  local path="$1"
  if [ ! -f "${path}" ]; then
    echo "missing expected file: ${path}" >&2
    exit 1
  fi
}

make_fake_tarball_pair() {
  local out_dir="$1"
  local file_name="$2"
  local payload_name="$3"
  local payload_content="$4"
  local artifact_path="${out_dir}/${file_name}"
  local stage_dir="${out_dir}/stage-${file_name}"
  mkdir -p "${stage_dir}"
  printf '%s\n' "${payload_content}" > "${stage_dir}/${payload_name}"
  tar -czf "${artifact_path}" -C "${stage_dir}" "${payload_name}"
  local actual
  actual="$(sha256sum "${artifact_path}" | awk '{print $1}')"
  printf '%s  /tmp/original-build-host/%s\n' "${actual}" "${file_name}" > "${artifact_path}.sha256"
  rm -rf "${stage_dir}"
}

echo "[1/4] release-local fails when external artifact checksum does not match"
bad_dir="${WORK_DIR}/bad-lxc"
mkdir -p "${bad_dir}"
make_fake_tarball_pair "${bad_dir}" "ccbg-lxc-package.tar.gz" "payload.txt" "bad-lxc"
printf '0000000000000000000000000000000000000000000000000000000000000000  /tmp/original-build-host/ccbg-lxc-package.tar.gz\n' > "${bad_dir}/ccbg-lxc-package.tar.gz.sha256"
expect_fail \
  "release-local-checksum-mismatch" \
  "checksum mismatch for" \
  env CCBG_RELEASE_ALLOW_DIRTY=true CCBG_RELEASE_SKIP_CHECKS=true CCBG_RELEASE_LXC_ASSET_DIR="${bad_dir}" bash "${RELEASE_LOCAL_SCRIPT}" test-release-asset-merge

echo "[2/4] release-local copies external artifact and normalizes checksum sidecar"
good_dir="${WORK_DIR}/good-lxc"
mkdir -p "${good_dir}"
make_fake_tarball_pair "${good_dir}" "ccbg-lxc-package.tar.gz" "payload.txt" "good-lxc"
expect_pass \
  "release-local-normalizes-sidecar" \
  env CCBG_RELEASE_ALLOW_DIRTY=true CCBG_RELEASE_SKIP_CHECKS=true CCBG_RELEASE_LXC_ASSET_DIR="${good_dir}" bash "${RELEASE_LOCAL_SCRIPT}" test-release-asset-merge
require_file "${ROOT_DIR}/target/release-local/test-release-asset-merge/ccbg-lxc-package.tar.gz"
require_file "${ROOT_DIR}/target/release-local/test-release-asset-merge/ccbg-lxc-package.tar.gz.sha256"
if ! grep -F -- 'ccbg-lxc-package.tar.gz' "${ROOT_DIR}/target/release-local/test-release-asset-merge/ccbg-lxc-package.tar.gz.sha256" >/dev/null; then
  echo "normalized sidecar did not reference local artifact filename" >&2
  exit 1
fi
if grep -F -- '/tmp/original-build-host/' "${ROOT_DIR}/target/release-local/test-release-asset-merge/ccbg-lxc-package.tar.gz.sha256" >/dev/null; then
  echo "normalized sidecar still references build-host path" >&2
  exit 1
fi

echo "[3/4] download-build-runner-release-assets fails on ambiguous artifact matches"
fake_gh_dir="${WORK_DIR}/fake-gh-bin"
mkdir -p "${fake_gh_dir}"
cat > "${fake_gh_dir}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1" = "run" ] && [ "$2" = "download" ]; then
  dest=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-D" ]; then
      dest="$2"
      shift 2
      continue
    fi
    shift
  done
  mkdir -p "${dest}/one" "${dest}/two"
  printf 'a' > "${dest}/one/ccbg-lxc-package.tar.gz"
  printf 'b' > "${dest}/two/ccbg-lxc-package.tar.gz"
  printf 'c' > "${dest}/ccbg-lxc-package.tar.gz.sha256"
  exit 0
fi
echo "unexpected gh invocation: $*" >&2
exit 1
EOF
chmod +x "${fake_gh_dir}/gh"
expect_fail \
  "download-assets-ambiguous-match" \
  "ambiguous artifact file matching ccbg-lxc-package.tar.gz" \
  env PATH="${fake_gh_dir}:$PATH" bash "${DOWNLOAD_SCRIPT}" --run-id 123456 --skip-macos --out-dir "${ROOT_DIR}/target/download-build-runner-assets-test"

echo "[4/4] download-build-runner-release-assets succeeds on a single exact match"
cat > "${fake_gh_dir}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "$1" = "run" ] && [ "$2" = "download" ]; then
  dest=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-D" ]; then
      dest="$2"
      shift 2
      continue
    fi
    shift
  done
  mkdir -p "${dest}"
  printf 'artifact' > "${dest}/ccbg-lxc-package.tar.gz"
  printf 'sidecar' > "${dest}/ccbg-lxc-package.tar.gz.sha256"
  exit 0
fi
echo "unexpected gh invocation: $*" >&2
exit 1
EOF
chmod +x "${fake_gh_dir}/gh"
expect_pass \
  "download-assets-single-match" \
  env PATH="${fake_gh_dir}:$PATH" bash "${DOWNLOAD_SCRIPT}" --run-id 123456 --skip-macos --out-dir "${ROOT_DIR}/target/download-build-runner-assets-test"
require_file "${ROOT_DIR}/target/download-build-runner-assets-test/release-inputs/lxc/ccbg-lxc-package.tar.gz"
require_file "${ROOT_DIR}/target/download-build-runner-assets-test/release-inputs/lxc/ccbg-lxc-package.tar.gz.sha256"

echo "release asset merge script checks passed"
