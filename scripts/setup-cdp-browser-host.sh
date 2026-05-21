#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  setup-cdp-browser-host.sh <host-ip>

Environment overrides:
  HOST_IP=<browser-host-lan-ip>
  PORT=9222
  BROWSER=auto|edge|chrome|/absolute/path/to/browser
  PROFILE_DIR=/tmp/ccbg-cdp
  BROWSER_LOG=/tmp/ccbg-cdp-browser.log
  SOCAT_LOG=/tmp/ccbg-cdp-socat.log
  DISPLAY=:0
  XAUTHORITY=/home/you/.Xauthority

This script is safe to re-run.
Run it on the machine that actually has the browser.
Do not run it on the soft router.
EOF
}

HOST_IP="${1:-${HOST_IP:-}}"
PORT="${PORT:-9222}"
BROWSER="${BROWSER:-auto}"
PROFILE_DIR="${PROFILE_DIR:-/tmp/ccbg-cdp}"
BROWSER_LOG="${BROWSER_LOG:-/tmp/ccbg-cdp-browser.log}"
SOCAT_LOG="${SOCAT_LOG:-/tmp/ccbg-cdp-socat.log}"
SOCAT_PID_FILE="${SOCAT_PID_FILE:-/tmp/ccbg-cdp-socat-${PORT}.pid}"
OS_NAME="$(uname -s)"
LOCAL_URL="http://127.0.0.1:${PORT}/json/version"
LAN_URL="http://${HOST_IP}:${PORT}/json/version"

if [[ -z "${HOST_IP}" ]]; then
  usage >&2
  exit 64
fi

case "${HOST_IP}" in
  127.*|localhost)
    echo "HOST_IP must be the browser host LAN IP, not localhost or 127.0.0.1" >&2
    exit 64
    ;;
esac

if ! [[ "${PORT}" =~ ^[0-9]+$ ]]; then
  echo "PORT must be numeric" >&2
  exit 64
fi

require_command() {
  local name="$1"
  if ! command -v "${name}" >/dev/null 2>&1; then
    echo "Missing required command: ${name}" >&2
    exit 127
  fi
}

run_as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
    return
  fi
  if command -v sudo >/dev/null 2>&1; then
    sudo "$@"
    return
  fi
  echo "This step needs root privileges. Install sudo or rerun as root." >&2
  exit 126
}

probe() {
  local url="$1"
  curl --silent --show-error --fail --max-time 3 "${url}" >/dev/null
}

wait_for_probe() {
  local url="$1"
  local label="$2"
  local attempt
  for attempt in $(seq 1 20); do
    if probe "${url}"; then
      return 0
    fi
    sleep 1
  done
  echo "${label} did not become reachable: ${url}" >&2
  return 1
}

linux_browser_candidates() {
  case "${BROWSER}" in
    auto)
      printf '%s\n' \
        microsoft-edge \
        microsoft-edge-stable \
        msedge \
        google-chrome \
        google-chrome-stable \
        chromium \
        chromium-browser
      ;;
    edge)
      printf '%s\n' microsoft-edge microsoft-edge-stable msedge
      ;;
    chrome)
      printf '%s\n' google-chrome google-chrome-stable chromium chromium-browser
      ;;
    *)
      if [[ "${BROWSER}" == */* ]]; then
        printf '%s\n' "${BROWSER}"
      fi
      ;;
  esac
}

linux_browser_paths() {
  case "${BROWSER}" in
    auto)
      printf '%s\n' \
        /opt/microsoft/msedge/msedge \
        /usr/bin/microsoft-edge \
        /usr/bin/microsoft-edge-stable \
        /usr/bin/msedge \
        /snap/bin/microsoft-edge
      printf '%s\n' \
        /usr/bin/google-chrome \
        /usr/bin/google-chrome-stable \
        /usr/bin/chromium \
        /usr/bin/chromium-browser \
        /snap/bin/chromium
      ;;
    edge)
      printf '%s\n' \
        /opt/microsoft/msedge/msedge \
        /usr/bin/microsoft-edge \
        /usr/bin/microsoft-edge-stable \
        /usr/bin/msedge \
        /snap/bin/microsoft-edge
      ;;
    chrome)
      printf '%s\n' \
        /usr/bin/google-chrome \
        /usr/bin/google-chrome-stable \
        /usr/bin/chromium \
        /usr/bin/chromium-browser \
        /snap/bin/chromium
      ;;
    *)
      if [[ "${BROWSER}" == */* ]]; then
        printf '%s\n' "${BROWSER}"
      fi
      ;;
  esac
}

find_linux_browser() {
  local candidate
  while IFS= read -r candidate; do
    if [[ -n "${candidate}" ]] && command -v "${candidate}" >/dev/null 2>&1; then
      command -v "${candidate}"
      return 0
    fi
  done < <(linux_browser_candidates)

  while IFS= read -r candidate; do
    if [[ -n "${candidate}" ]] && [[ -x "${candidate}" ]]; then
      printf '%s\n' "${candidate}"
      return 0
    fi
  done < <(linux_browser_paths)

  return 1
}

macos_app_name() {
  case "${BROWSER}" in
    chrome)
      printf '%s\n' "Google Chrome"
      ;;
    edge|auto)
      printf '%s\n' "Microsoft Edge"
      ;;
    *)
      printf '%s\n' "Microsoft Edge"
      ;;
  esac
}

launch_browser_if_needed() {
  if probe "${LOCAL_URL}"; then
    echo "Loopback CDP already reachable on ${LOCAL_URL}"
    return
  fi

  mkdir -p "${PROFILE_DIR}"

  case "${OS_NAME}" in
    Darwin)
      local app_name
      app_name="$(macos_app_name)"
      echo "Starting ${app_name} with a dedicated CDP profile"
      open -na "${app_name}" --args \
        "--remote-debugging-port=${PORT}" \
        "--user-data-dir=${PROFILE_DIR}" \
        --no-first-run \
        --no-default-browser-check
      ;;
    Linux)
      local browser_bin
      if ! browser_bin="$(find_linux_browser)"; then
        echo "No usable browser binary found for BROWSER=${BROWSER}" >&2
        exit 127
      fi
      if [[ -z "${DISPLAY:-}" ]]; then
        echo "DISPLAY is not set. Run this on the browser host desktop session, or export DISPLAY and XAUTHORITY first." >&2
        exit 126
      fi
      echo "Starting ${browser_bin} with a dedicated CDP profile"
      nohup "${browser_bin}" \
        "--remote-debugging-port=${PORT}" \
        "--user-data-dir=${PROFILE_DIR}" \
        --no-first-run \
        --no-default-browser-check \
        >"${BROWSER_LOG}" 2>&1 &
      ;;
    *)
      echo "Unsupported OS for this script: ${OS_NAME}" >&2
      exit 65
      ;;
  esac

  wait_for_probe "${LOCAL_URL}" "Loopback CDP"
}

install_socat_if_missing() {
  if command -v socat >/dev/null 2>&1; then
    return
  fi

  case "${OS_NAME}" in
    Darwin)
      if ! command -v brew >/dev/null 2>&1; then
        echo "socat is missing and Homebrew is not installed. Install socat manually first." >&2
        exit 127
      fi
      brew install socat
      ;;
    Linux)
      if command -v apt-get >/dev/null 2>&1; then
        run_as_root apt-get update
        run_as_root apt-get install -y socat
      elif command -v dnf >/dev/null 2>&1; then
        run_as_root dnf install -y socat
      elif command -v yum >/dev/null 2>&1; then
        run_as_root yum install -y socat
      elif command -v pacman >/dev/null 2>&1; then
        run_as_root pacman -Sy --noconfirm socat
      elif command -v apk >/dev/null 2>&1; then
        run_as_root apk add --no-cache socat
      else
        echo "socat is missing and no supported package manager was found. Install socat manually first." >&2
        exit 127
      fi
      ;;
    *)
      echo "Unsupported OS for socat installation: ${OS_NAME}" >&2
      exit 65
      ;;
  esac
}

stop_managed_bridge_if_any() {
  if [[ -f "${SOCAT_PID_FILE}" ]]; then
    local pid
    pid="$(cat "${SOCAT_PID_FILE}" 2>/dev/null || true)"
    if [[ -n "${pid}" ]] && kill -0 "${pid}" >/dev/null 2>&1; then
      kill "${pid}" >/dev/null 2>&1 || true
      wait "${pid}" 2>/dev/null || true
    fi
    rm -f "${SOCAT_PID_FILE}"
  fi

  if command -v pgrep >/dev/null 2>&1; then
    local existing_pids
    local pid
    existing_pids="$(pgrep -f "socat .*TCP-LISTEN:${PORT},bind=${HOST_IP},reuseaddr,fork.*TCP:127.0.0.1:${PORT}" || true)"
    if [[ -n "${existing_pids}" ]]; then
      while IFS= read -r pid; do
        [[ -n "${pid}" ]] || continue
        kill "${pid}" >/dev/null 2>&1 || true
      done <<< "${existing_pids}"
      sleep 1
    fi
  fi
}

start_bridge() {
  install_socat_if_missing
  stop_managed_bridge_if_any

  echo "Recreating LAN bridge ${HOST_IP}:${PORT} -> 127.0.0.1:${PORT}"
  nohup socat \
    "TCP-LISTEN:${PORT},bind=${HOST_IP},reuseaddr,fork" \
    "TCP:127.0.0.1:${PORT}" \
    >"${SOCAT_LOG}" 2>&1 &
  echo "$!" > "${SOCAT_PID_FILE}"

  wait_for_probe "${LAN_URL}" "LAN CDP bridge"
}

require_command curl

echo "1) Ensuring loopback CDP is up on ${LOCAL_URL}"
launch_browser_if_needed

echo "2) Ensuring LAN bridge is up on ${LAN_URL}"
start_bridge

echo "3) Verifying both endpoints"
probe "${LOCAL_URL}"
probe "${LAN_URL}"

cat <<EOF
CDP browser host is ready.

Loopback:
  ${LOCAL_URL}

LAN:
  ${LAN_URL}

Use this LAN URL in carrier-cloud-blob-gateway Admin -> Browser / CDP.
Do not enter localhost or 127.0.0.1 there.
EOF
