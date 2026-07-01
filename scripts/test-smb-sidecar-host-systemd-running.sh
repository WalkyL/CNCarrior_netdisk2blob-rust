#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER_BIN="${SMB_SIDECAR_HOST_BIN:-${ROOT_DIR}/target/debug/smb-sidecar-host}"
UNIT_PREFIX="${SMB_SIDECAR_TEST_UNIT_PREFIX:-ccbg-smb-sidecar-itest-$$}"

if [ "$(uname -s)" != "Linux" ]; then
  echo "test-smb-sidecar-host-systemd-running.sh must run on Linux" >&2
  exit 1
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "test-smb-sidecar-host-systemd-running.sh must run as root" >&2
  exit 1
fi

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

require_command systemd-run
require_command systemctl
require_command python3
require_command rclone
require_command smbd
require_command smbpasswd
require_command mountpoint
require_command fusermount3

if [ ! -x "${HELPER_BIN}" ]; then
  echo "missing smb-sidecar-host binary: ${HELPER_BIN}" >&2
  echo "build it first with: cargo build -p smb-sidecar-host" >&2
  exit 1
fi

if [ ! -e /dev/fuse ]; then
  echo "/dev/fuse is required for the real-running systemd integration test" >&2
  exit 1
fi

workspace_tmp="${SMB_SIDECAR_TEST_TMPDIR:-$(mktemp -d)}"
cleanup() {
  if [ -f "${env_file:-}" ] && [ -x "${HELPER_BIN}" ]; then
    env \
      CCBG_ENV_FILE="${env_file}" \
      CCBG_SMB_SIDECAR_UNIT_PREFIX="${UNIT_PREFIX}" \
      "${HELPER_BIN}" stop >/dev/null 2>&1 || true
  fi
  systemctl list-units "${UNIT_PREFIX}-*.service" --no-legend 2>/dev/null | awk '{print $1}' | while read -r unit; do
    [ -n "${unit}" ] || continue
    systemctl stop "${unit}" >/dev/null 2>&1 || true
    systemctl reset-failed "${unit}" >/dev/null 2>&1 || true
  done
  rm -rf "${workspace_tmp}"
}
trap cleanup EXIT

control_plane_file="${workspace_tmp}/control-plane.json"
env_file="${workspace_tmp}/ccbg.env"
state_root="${workspace_tmp}/smb-sidecar"
status_file="${state_root}/status.json"
metadata_file="${state_root}/managed-runtime.json"
mount_root="${workspace_tmp}/mounts"

cat > "${env_file}" <<EOF
CCBG_CONTROL_PLANE_FILE=${control_plane_file}
CCBG_BIND_ADDR=127.0.0.1:61080
CCBG_S3_REGION=us-east-1
CCBG_S3_ACCESS_KEY_ID=test-access
CCBG_S3_SECRET_ACCESS_KEY=test-secret
CCBG_SMB_CONFIG_ROOT=${workspace_tmp}/config
CCBG_SMB_DATA_ROOT=${workspace_tmp}/data
CCBG_SMB_MOUNT_ROOT=${mount_root}
EOF

cat > "${control_plane_file}" <<'EOF'
{
  "smb_sidecar": {
    "enabled": true,
    "bind_addr": "127.0.0.1",
    "port": 1446,
    "mount_root": "",
    "config_root": "",
    "data_root": "",
    "workgroup": "WORKGROUP",
    "server_string": "CCBG SMB Sidecar Systemd Test",
    "create_mask": "0660",
    "directory_mask": "0770",
    "disable_splice": true,
    "vfs_objects": ["catia"],
    "users": [
      {
        "id": "smb-user-1",
        "username": "smbtest2",
        "password": "secret-pass-2",
        "enabled": true,
        "allowed_share_ids": []
      }
    ],
    "shares": []
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
  env \
    CCBG_ENV_FILE="${env_file}" \
    CCBG_SMB_SIDECAR_UNIT_PREFIX="${UNIT_PREFIX}" \
    "$@" \
    "${HELPER_BIN}" "${action}"
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

echo "[2/6] enabled sync with systemd-run available reaches running state"
run_helper sync >/dev/null
assert_json_field "${status_file}" "state" '"running"'
assert_json_field "${status_file}" "listener_ready" 'true'
assert_json_field "${status_file}" "enabled_share_count" '0'
assert_json_field "${status_file}" "mounted_share_count" '0'
assert_json_field "${status_file}" "process_count" '1'
python3 - "${status_file}" "${UNIT_PREFIX}" <<'PY'
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as handle:
    payload = json.load(handle)
prefix = sys.argv[2]
processes = payload["processes"]
assert len(processes) == 1
assert processes[0]["role"] == "smbd"
assert processes[0]["unit_name"] == f"{prefix}-smbd.service"
assert payload["share_states"] == []
assert payload["last_error"] is None
PY

echo "[3/6] systemd transient unit exists and is active"
systemctl is-active "${UNIT_PREFIX}-smbd.service" >/dev/null

echo "[4/6] second sync stays running and preserves desired hash"
first_hash="$(python3 - "${metadata_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['desired_hash'])
PY
)"
run_helper sync >/dev/null
second_hash="$(python3 - "${metadata_file}" <<'PY'
import json, sys
with open(sys.argv[1], 'r', encoding='utf-8') as handle:
    payload = json.load(handle)
print(payload['desired_hash'])
PY
)"
if [ "${second_hash}" != "${first_hash}" ]; then
  echo "expected desired_hash to stay stable across identical sync inputs" >&2
  exit 1
fi
assert_json_field "${status_file}" "state" '"running"'

echo "[5/6] stop tears down running systemd unit"
run_helper stop >/dev/null
assert_json_field "${status_file}" "state" '"stopped"'
if systemctl is-active "${UNIT_PREFIX}-smbd.service" >/dev/null 2>&1; then
  echo "expected ${UNIT_PREFIX}-smbd.service to be stopped" >&2
  exit 1
fi

echo "[6/6] status after stop reports stopped contract"
status_output="$(run_helper status)"
python3 - <<'PY' "${status_output}"
import json
import sys
payload = json.loads(sys.argv[1])
assert payload["state"] == "stopped"
assert payload["mode"] == "host_process"
PY

echo "smb-sidecar-host systemd running integration checks passed"
