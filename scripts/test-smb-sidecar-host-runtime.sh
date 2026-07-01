#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER_BIN="${SMB_SIDECAR_HOST_BIN:-${ROOT_DIR}/target/debug/smb-sidecar-host}"

if [ "$(uname -s)" != "Linux" ]; then
  echo "test-smb-sidecar-host-runtime.sh must run on Linux" >&2
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "test-smb-sidecar-host-runtime.sh must run as root" >&2
  exit 1
fi

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

require_command python3
require_command rclone
require_command smbd
require_command smbpasswd
require_command mountpoint

if [ ! -x "${HELPER_BIN}" ]; then
  echo "missing smb-sidecar-host binary: ${HELPER_BIN}" >&2
  echo "build it first with: cargo build -p smb-sidecar-host" >&2
  exit 1
fi

workspace_tmp="${SMB_SIDECAR_TEST_TMPDIR:-$(mktemp -d)}"
cleanup() {
  if [ -f "${env_file:-}" ] && [ -x "${HELPER_BIN}" ]; then
    CCBG_ENV_FILE="${env_file}" "${HELPER_BIN}" stop >/dev/null 2>&1 || true
  fi
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
CCBG_S3_ACCESS_KEY_ID=test-access
CCBG_S3_SECRET_ACCESS_KEY=test-secret
CCBG_SMB_CONFIG_ROOT=${workspace_tmp}/config
CCBG_SMB_DATA_ROOT=${workspace_tmp}/data
CCBG_SMB_MOUNT_ROOT=${workspace_tmp}/mounts
EOF

cat > "${control_plane_file}" <<'EOF'
{
  "smb_sidecar": {
    "enabled": true,
    "bind_addr": "127.0.0.1",
    "port": 1445,
    "mount_root": "",
    "config_root": "",
    "data_root": "",
    "workgroup": "WORKGROUP",
    "server_string": "CCBG SMB Sidecar Test",
    "create_mask": "0660",
    "directory_mask": "0770",
    "disable_splice": true,
    "vfs_objects": ["catia"],
    "users": [
      {
        "id": "smb-user-1",
        "username": "smbtest1",
        "password": "secret-pass-1",
        "enabled": true,
        "allowed_share_ids": []
      }
    ],
    "shares": [
      {
        "id": "root",
        "share_name": "CCBGRoot",
        "application_id": "default",
        "bucket": "root",
        "prefix": "",
        "enabled": true,
        "read_only": false,
        "browseable": true,
        "guest_ok": false,
        "valid_user_ids": ["smb-user-1"],
        "create_mask": "0660",
        "directory_mask": "0770"
      }
    ]
  },
  "applications": [
    {
      "id": "default",
      "access_key_id": "test-access",
      "secret_access_key": "test-secret",
      "enabled": true
    }
  ]
}
EOF

run_helper() {
  local action="$1"
  shift
  env CCBG_ENV_FILE="${env_file}" "$@" "${HELPER_BIN}" "${action}"
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

echo "[1/5] build debug helper"
cargo build -p smb-sidecar-host --manifest-path "${ROOT_DIR}/Cargo.toml" >/dev/null

echo "[2/5] enabled sync with forced no-fuse degrades but keeps listener ready"
run_helper sync env CCBG_SMB_SIDECAR_FORCE_NO_SYSTEMD_RUN=1 CCBG_SMB_SIDECAR_FORCE_NO_FUSE=1 >/dev/null
assert_json_field "${status_file}" "state" '"degraded"'
assert_json_field "${status_file}" "listener_ready" 'true'
assert_json_field "${status_file}" "enabled_share_count" '1'
assert_json_field "${status_file}" "mounted_share_count" '0'
assert_json_field "${status_file}" "process_count" '1'
python3 - "${status_file}" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)
assert payload["share_states"][0]["last_error"].startswith("SMB share mounts need /dev/fuse")
assert payload["processes"][0]["role"] == "smbd"
PY

echo "[3/5] second sync with same desired hash stays degraded and reuses runtime state"
first_hash="$(python3 - "${metadata_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['desired_hash'])
PY
)"
first_updated="$(python3 - "${status_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['status_updated_at_unix_ms'])
PY
)"
sleep 1
run_helper sync env CCBG_SMB_SIDECAR_FORCE_NO_SYSTEMD_RUN=1 CCBG_SMB_SIDECAR_FORCE_NO_FUSE=1 >/dev/null
second_updated="$(python3 - "${status_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['status_updated_at_unix_ms'])
PY
)"
second_hash="$(python3 - "${metadata_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['desired_hash'])
PY
)"
if [ "${second_updated}" -le "${first_updated}" ]; then
  echo "expected second sync to refresh status_updated_at_unix_ms" >&2
  exit 1
fi
if [ "${second_hash}" != "${first_hash}" ]; then
  echo "expected desired_hash to stay stable across identical sync inputs" >&2
  exit 1
fi
assert_json_field "${status_file}" "state" '"degraded"'
assert_json_field "${metadata_file}" "desired_hash" "\"${second_hash}\""

echo "[4/5] stop tears down runtime and writes stopped state"
run_helper stop >/dev/null
assert_json_field "${status_file}" "state" '"stopped"'
assert_json_field "${metadata_file}" "state" '"stopped"'

echo "[5/5] status after stop reports stopped contract"
status_output="$(run_helper status)"
python3 - <<'PY' "${status_output}"
import json
import sys
payload = json.loads(sys.argv[1])
assert payload["state"] == "stopped"
assert payload["mode"] == "host_process"
PY

echo "smb-sidecar-host runtime integration checks passed"
