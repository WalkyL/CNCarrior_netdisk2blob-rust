#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

PACKAGE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TIMESTAMP="$(date +%Y%m%d%H%M%S)"
INSTALL_PROFILE="${CCBG_INSTALL_PROFILE:-s3-only}"
PROFILE_EXPLICIT=false

usage() {
  cat <<'EOF'
usage: scripts/install.sh [--s3-only|--enable-smb-sidecar]

Profiles:
  --s3-only              Install and start gatewayd only. This is the default.
  --enable-smb-sidecar   Install SMB sidecar dependencies, enable SMB in the
                         Admin/control-plane defaults, install sidecar units,
                         and run one reconcile pass.

Environment:
  CCBG_INSTALL_PROFILE=s3-only|enable-smb-sidecar
EOF
}

parse_args() {
  if [ -n "${CCBG_INSTALL_PROFILE:-}" ]; then
    PROFILE_EXPLICIT=true
  fi
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --s3-only)
        INSTALL_PROFILE="s3-only"
        PROFILE_EXPLICIT=true
        shift
        ;;
      --enable-smb-sidecar|--with-smb-sidecar)
        INSTALL_PROFILE="enable-smb-sidecar"
        PROFILE_EXPLICIT=true
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        echo "unknown argument: $1" >&2
        usage >&2
        exit 2
        ;;
    esac
  done
  case "${INSTALL_PROFILE}" in
    s3-only|enable-smb-sidecar)
      ;;
    *)
      echo "invalid CCBG_INSTALL_PROFILE: ${INSTALL_PROFILE}" >&2
      usage >&2
      exit 2
      ;;
  esac
}

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "install.sh must run as root inside the LXC guest" >&2
    exit 1
  fi
}

install_user() {
  if ! getent group ccbg >/dev/null; then
    groupadd --system ccbg
  fi
  if ! id ccbg >/dev/null 2>&1; then
    useradd --system --gid ccbg --home-dir /var/lib/ccbg --shell /usr/sbin/nologin ccbg
  fi
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

set_env_value() {
  local file="$1"
  local key="$2"
  local value="$3"
  if grep -q "^${key}=" "${file}"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "${file}"
  else
    printf '\n%s=%s\n' "${key}" "${value}" >> "${file}"
  fi
}

env_value() {
  local file="$1"
  local key="$2"
  local fallback="$3"
  if [ -f "${file}" ] && grep -q "^${key}=" "${file}"; then
    grep "^${key}=" "${file}" | tail -n 1 | cut -d= -f2-
  else
    printf '%s\n' "${fallback}"
  fi
}

install_smb_dependencies() {
  local missing=()
  local packages=()

  command_exists python3 || missing+=("python3")
  command_exists rclone || missing+=("rclone")
  command_exists smbd || missing+=("smbd")
  command_exists smbpasswd || missing+=("smbpasswd")
  command_exists fusermount || command_exists fusermount3 || missing+=("fusermount")

  if [ "${#missing[@]}" -gt 0 ] && ! command_exists apt-get; then
    echo "missing SMB sidecar dependencies and apt-get is unavailable: ${missing[*]}" >&2
    exit 1
  fi

  if [ "${#missing[@]}" -gt 0 ]; then
    for item in "${missing[@]}"; do
      case "${item}" in
        python3)
          packages+=("python3")
          ;;
        rclone)
          packages+=("rclone")
          ;;
        smbd|smbpasswd)
          packages+=("samba")
          ;;
        fusermount)
          packages+=("fuse3")
          ;;
      esac
    done
  fi
  if [ "${INSTALL_PROFILE}" = "enable-smb-sidecar" ]; then
    packages+=("samba-vfs-modules")
  fi

  mapfile -t packages < <(printf '%s\n' "${packages[@]}" | awk 'NF && !seen[$0]++')
  if [ "${#packages[@]}" -gt 0 ]; then
    apt-get update
    apt-get install -y --no-install-recommends "${packages[@]}"
  fi
  # The distro Samba service would bind :445 independently of the CCBG
  # sidecar. Keep smbd/nmbd installed, but let ccbg-smb-sidecar own runtime.
  systemctl disable --now smbd.service nmbd.service smb.service nmb.service 2>/dev/null || true
  if [ -f /etc/fuse.conf ]; then
    if grep -q '^#user_allow_other' /etc/fuse.conf; then
      sed -i 's/^#user_allow_other/user_allow_other/' /etc/fuse.conf
    elif ! grep -q '^user_allow_other' /etc/fuse.conf; then
      printf '\nuser_allow_other\n' >> /etc/fuse.conf
    fi
  fi
  if [ ! -e /dev/fuse ]; then
    cat >&2 <<'EOF'
warning: /dev/fuse is not available in this guest.
SMB sidecar can still start managed smbd and listen on 0.0.0.0:445, but
rclone-backed shares such as CCBGRoot cannot mount until the LXC/container
exposes /dev/fuse. On PVE, enable FUSE for this container before adding SMB
shares or users that should mount cloud-drive contents.
EOF
  elif [ ! -w /dev/fuse ]; then
    echo "warning: /dev/fuse exists but is not writable; rclone-backed SMB shares may fail to mount" >&2
  fi
}

install_tree() {
  install -d -m 0755 /opt/ccbg/bin /opt/ccbg/assets/admin /opt/ccbg/scripts /etc/ccbg /etc/ccbg/config /var/lib/ccbg /var/lib/ccbg/body-spool /var/lib/ccbg/provider-credentials /var/lib/ccbg/smb-sidecar /var/log/ccbg /opt/ccbg/backups
  if [ -x /opt/ccbg/bin/gatewayd ]; then
    old_sha="$(sha256sum /opt/ccbg/bin/gatewayd | awk '{print $1}')"
    cp /opt/ccbg/bin/gatewayd "/opt/ccbg/backups/gatewayd.${old_sha}.${TIMESTAMP}"
  fi
  install -m 0755 "${PACKAGE_ROOT}/bin/gatewayd" /opt/ccbg/bin/gatewayd
  install -m 0644 "${PACKAGE_ROOT}/assets/admin/index.html" /opt/ccbg/assets/admin/index.html
  cp -R "${PACKAGE_ROOT}/config/." /etc/ccbg/config/
  install -m 0644 "${PACKAGE_ROOT}/systemd/ccbg.service" /etc/systemd/system/ccbg.service

  if [ ! -f /etc/ccbg/ccbg.env ]; then
    install -m 0640 "${PACKAGE_ROOT}/etc/ccbg.env" /etc/ccbg/ccbg.env
  else
    install -m 0640 "${PACKAGE_ROOT}/etc/ccbg.env" "/etc/ccbg/ccbg.env.package-${TIMESTAMP}"
    echo "kept existing /etc/ccbg/ccbg.env; new sample written to /etc/ccbg/ccbg.env.package-${TIMESTAMP}"
  fi

  chown -R ccbg:ccbg /var/lib/ccbg /var/log/ccbg
  chown -R root:root /opt/ccbg/scripts
  chown -R root:ccbg /etc/ccbg
  chmod 0750 /var/lib/ccbg /var/lib/ccbg/provider-credentials /var/lib/ccbg/body-spool
  chmod 0750 /var/log/ccbg
}

configure_smb_profile() {
  case "${INSTALL_PROFILE}" in
    enable-smb-sidecar)
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_ENABLED true
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_BIND_ADDR 0.0.0.0
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_PORT 445
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_MOUNT_ROOT /mnt/ccbg/smb/mounts
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_CONFIG_ROOT /var/lib/ccbg/smb-sidecar/config
      set_env_value /etc/ccbg/ccbg.env CCBG_SMB_DATA_ROOT /var/lib/ccbg/smb-sidecar/data
      ;;
    s3-only)
      if [ "${PROFILE_EXPLICIT}" = true ]; then
        set_env_value /etc/ccbg/ccbg.env CCBG_SMB_ENABLED false
      fi
      ;;
  esac
}

patch_control_plane_smb_enabled() {
  local enabled="$1"
  local control_plane_file
  local bind_addr
  local port
  local mount_root
  local config_root
  local data_root
  if ! command_exists python3; then
    echo "python3 is unavailable; skipped existing control-plane SMB patch" >&2
    return 0
  fi
  control_plane_file="$(env_value /etc/ccbg/ccbg.env CCBG_CONTROL_PLANE_FILE /var/lib/ccbg/control-plane.json)"
  if [ ! -f "${control_plane_file}" ]; then
    return 0
  fi
  bind_addr="$(env_value /etc/ccbg/ccbg.env CCBG_SMB_BIND_ADDR 0.0.0.0)"
  port="$(env_value /etc/ccbg/ccbg.env CCBG_SMB_PORT 445)"
  mount_root="$(env_value /etc/ccbg/ccbg.env CCBG_SMB_MOUNT_ROOT /mnt/ccbg/smb/mounts)"
  config_root="$(env_value /etc/ccbg/ccbg.env CCBG_SMB_CONFIG_ROOT /var/lib/ccbg/smb-sidecar/config)"
  data_root="$(env_value /etc/ccbg/ccbg.env CCBG_SMB_DATA_ROOT /var/lib/ccbg/smb-sidecar/data)"
  python3 - "${control_plane_file}" "${enabled}" "${bind_addr}" "${port}" "${mount_root}" "${config_root}" "${data_root}" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
enabled = sys.argv[2].lower() == "true"
bind_addr = sys.argv[3]
port = int(sys.argv[4])
mount_root = sys.argv[5]
config_root = sys.argv[6]
data_root = sys.argv[7]
payload = json.loads(path.read_text(encoding="utf-8"))
smb = payload.get("smb_sidecar")
if not isinstance(smb, dict):
    smb = {}
payload["smb_sidecar"] = smb
smb["enabled"] = enabled
if enabled:
    smb["bind_addr"] = bind_addr
    smb["port"] = port
    smb["mount_root"] = mount_root
    smb["config_root"] = config_root
    smb["data_root"] = data_root
path.write_text(json.dumps(payload, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")
PY
}

install_smb_sidecar_components() {
  local control_plane_file
  install_smb_dependencies
  install -d -m 0755 /opt/ccbg/bin
  install -m 0755 "${PACKAGE_ROOT}/bin/smb-sidecar-host" /opt/ccbg/bin/smb-sidecar-host
  install -m 0755 "${PACKAGE_ROOT}/scripts/ccbg-smb-sidecar.sh" /opt/ccbg/scripts/ccbg-smb-sidecar.sh
  install -m 0644 "${PACKAGE_ROOT}/systemd/ccbg-smb-sidecar-sync.service" /etc/systemd/system/ccbg-smb-sidecar-sync.service
  install -m 0644 "${PACKAGE_ROOT}/systemd/ccbg-smb-sidecar.timer" /etc/systemd/system/ccbg-smb-sidecar.timer
  control_plane_file="$(env_value /etc/ccbg/ccbg.env CCBG_CONTROL_PLANE_FILE /var/lib/ccbg/control-plane.json)"
  cat > /etc/systemd/system/ccbg-smb-sidecar.path <<EOF
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

[Unit]
Description=Watch CCBG control plane for SMB sidecar changes

[Path]
PathModified=${control_plane_file}
PathChanged=/etc/ccbg/ccbg.env
Unit=ccbg-smb-sidecar-sync.service

[Install]
WantedBy=multi-user.target
EOF
  chown root:root /opt/ccbg/bin/smb-sidecar-host /opt/ccbg/scripts/ccbg-smb-sidecar.sh
}

start_service() {
  systemctl daemon-reload
  systemctl enable ccbg.service
  systemctl restart ccbg.service
  case "${INSTALL_PROFILE}" in
    enable-smb-sidecar)
      systemctl enable --now ccbg-smb-sidecar.path ccbg-smb-sidecar.timer
      systemctl start ccbg-smb-sidecar-sync.service || true
      systemctl --no-pager --full status ccbg-smb-sidecar-sync.service || true
      ;;
    s3-only)
      if [ "${PROFILE_EXPLICIT}" = true ]; then
        if [ -x /opt/ccbg/bin/smb-sidecar-host ]; then
          /opt/ccbg/bin/smb-sidecar-host stop 2>/dev/null || true
        fi
        systemctl disable --now ccbg-smb-sidecar.path ccbg-smb-sidecar.timer 2>/dev/null || true
        systemctl stop ccbg-smb-sidecar-sync.service 2>/dev/null || true
      fi
      ;;
  esac
  systemctl --no-pager --full status ccbg.service || true
}

parse_args "$@"
require_root
install_user
install_tree
case "${INSTALL_PROFILE}" in
  enable-smb-sidecar)
    install_smb_sidecar_components
    configure_smb_profile
    patch_control_plane_smb_enabled true
    ;;
  s3-only)
    configure_smb_profile
    if [ "${PROFILE_EXPLICIT}" = true ]; then
      patch_control_plane_smb_enabled false
    fi
    ;;
esac
start_service
echo "ccbg LXC install complete (${INSTALL_PROFILE})"
