#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER_BIN="${SMB_SIDECAR_HOST_BIN:-${ROOT_DIR}/target/debug/smb-sidecar-host}"

if [ "$(uname -s)" != "Linux" ]; then
  echo "test-smb-sidecar-host-integration.sh must run on Linux" >&2
  exit 1
fi

workspace_tmp="${SMB_SIDECAR_TEST_TMPDIR:-$(mktemp -d)}"
cleanup() {
  rm -rf "${workspace_tmp}"
}
trap cleanup EXIT

control_plane_file="${workspace_tmp}/control-plane.json"
env_file="${workspace_tmp}/ccbg.env"
state_root="${workspace_tmp}/smb-sidecar"
status_file="${state_root}/status.json"
metadata_file="${state_root}/managed-runtime.json"

cat > "${env_file}" <<EOF
CCBG_CONTROL_PLANE_FILE=${control_plane_file}
CCBG_BIND_ADDR=127.0.0.1:61080
CCBG_S3_REGION=us-east-1
CCBG_SMB_CONFIG_ROOT=${workspace_tmp}/config
CCBG_SMB_DATA_ROOT=${workspace_tmp}/data
EOF

run_helper() {
  CCBG_ENV_FILE="${env_file}" "${HELPER_BIN}" "$@"
}

assert_json_field() {
  local file="$1"
  local expression="$2"
  local expected="$3"
  python3 - "$file" "$expression" "$expected" <<'PY'
import json
import sys

path, expression, expected = sys.argv[1:4]
with open(path, "r", encoding="utf-8") as handle:
    payload = json.load(handle)
value = payload
for part in expression.split("."):
    if part:
        value = value[part]
actual = json.dumps(value, ensure_ascii=True)
if actual != expected:
    raise SystemExit(f"assertion failed: {expression} expected {expected} got {actual}")
PY
}

echo "[1/6] build debug helper"
cargo build -p smb-sidecar-host --manifest-path "${ROOT_DIR}/Cargo.toml" >/dev/null

echo "[2/6] disabled sync writes disabled status"
printf '{}\n' > "${control_plane_file}"
run_helper sync >/dev/null
test -f "${status_file}"
test -f "${metadata_file}"
assert_json_field "${status_file}" "state" '"disabled"'
assert_json_field "${metadata_file}" "state" '"disabled"'

echo "[3/6] status prints fallback JSON when status file is missing"
rm -f "${status_file}" "${metadata_file}"
status_output="$(run_helper status)"
python3 - <<'PY' "${status_output}" "${state_root}"
import json
import sys
payload = json.loads(sys.argv[1])
runtime_root = sys.argv[2]
assert payload["state"] == "unknown"
assert payload["mode"] == "host_process"
assert payload["auto_managed"] is True
assert payload["runtime_root"] == runtime_root
PY

echo "[4/6] stop writes stopped status and metadata"
run_helper stop >/dev/null
assert_json_field "${status_file}" "state" '"stopped"'
assert_json_field "${metadata_file}" "state" '"stopped"'

echo "[5/6] usage exits non-zero for unsupported action"
if run_helper invalid >/dev/null 2>&1; then
  echo "expected invalid action to fail" >&2
  exit 1
fi

echo "[6/6] packaged/runtime paths no longer depend on python helper"
if grep -R "ccbg-smb-sidecar.py" \
  "${ROOT_DIR}/deploy/lxc/ccbg-smb-sidecar.sh" \
  "${ROOT_DIR}/deploy/lxc/install.sh" \
  "${ROOT_DIR}/scripts/build-lxc-package.sh" >/dev/null; then
  echo "python helper reference still present in active packaging/runtime scripts" >&2
  exit 1
fi

echo "smb-sidecar-host integration checks passed"
