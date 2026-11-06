#!/usr/bin/env bash
# Local logic test for smoke.sh. Runs a mock HTTP server and exercises the
# rewritten smoke.sh across healthy / degraded / dead / strict scenarios.
set -uo pipefail

SMOKE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/smoke.sh"
WORK="$(mktemp -d)"
PORT=61999
readiness="200"
listbuckets="200"

# Failure accumulator: any FAIL (or mock startup failure) flips the final exit
# status so CI surfaces a broken smoke.sh instead of a silent false-green.
FAILURES=0
MOCK_PID=""

# Ensure the harness-owned mock is stopped and only our mktemp directory is
# removed, even on interrupt/termination.
cleanup() {
  if [ -n "${MOCK_PID}" ]; then
    kill "${MOCK_PID}" 2>/dev/null || true
    wait "${MOCK_PID}" 2>/dev/null || true
  fi
  if [ -n "${WORK}" ] && [ -d "${WORK}" ]; then
    rm -rf "${WORK}"
  fi
}
trap cleanup EXIT INT TERM

# This WSL distro symlinks /usr/local/bin/curl -> /mnt/c/WINDOWS/system32/curl.exe
# (Windows curl), which cannot write to WSL's /dev/null -> curl exits 23. Force
# the real Linux curl onto PATH so the test exercises smoke.sh's logic, not the
# Windows-curl artifact. The target LXC (Debian) has a genuine Linux curl.
export PATH="/usr/bin:/bin:/usr/local/sbin:/usr/sbin:/sbin"
hash -r

# Minimal python mock server that answers /healthz, /readyz, and SigV4 ListBuckets
# with configurable status codes.
cat > "${WORK}/mock.py" <<'PY'
import http.server, socketserver, sys
READY = sys.argv[2]
LB = sys.argv[3]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/healthz":
            self._send(200, b'{"status":"ok"}')
        elif self.path == "/readyz":
            self._send(int(READY), b'')
        elif self.path == "/":
            self._send(int(LB), b'<Error><Code>Test</Code></Error>')
        else:
            self._send(404, b'')
    def _send(self, code, body):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
class S(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
with S(("127.0.0.1", int(sys.argv[1])), H) as httpd:
    httpd.serve_forever()
PY

start_mock() {
  python3 "${WORK}/mock.py" "${PORT}" "${readiness}" "${listbuckets}" >"${WORK}/mock.log" 2>&1 &
  MOCK_PID=$!
  # Wait until the port actually accepts connections (up to 5s), so curl never
  # races the server bind and we surface any mock startup error.
  for _ in $(seq 1 50); do
    if python3 -c "import socket,sys; s=socket.socket(); s.settimeout(0.2); s.connect(('127.0.0.1', int(sys.argv[1]))); s.close()" "${PORT}" 2>/dev/null; then
      return 0
    fi
    if ! kill -0 "${MOCK_PID}" 2>/dev/null; then
      echo "MOCK DIED on startup:" >&2
      sed 's/^/    /' "${WORK}/mock.log" >&2
      return 1
    fi
    sleep 0.1
  done
  echo "MOCK did not bind ${PORT} within 5s" >&2
  sed 's/^/    /' "${WORK}/mock.log" >&2
  return 1
}
stop_mock() {
  kill "${MOCK_PID}" 2>/dev/null || true
  wait "${MOCK_PID}" 2>/dev/null || true
  MOCK_PID=""
}

run_case() {
  local name="$1"
  local readiness_code="$2"
  local lb_code="$3"
  local require_ready="$4"
  local expect_exit="$5"
  readiness="${readiness_code}"
  listbuckets="${lb_code}"
  if ! start_mock; then
    echo "FAIL: ${name} (mock failed to start)"
    FAILURES=$((FAILURES + 1))
    return 1
  fi
  CCBG_SMOKE_ENDPOINT="http://127.0.0.1:${PORT}" \
  CCBG_SMOKE_METRICS_ENDPOINT="http://127.0.0.1:${PORT}" \
  CCBG_SMOKE_CONTROL_API_KEY="" \
  CCBG_SMOKE_REQUIRE_READY="${require_ready}" \
  bash "${SMOKE}" >"${WORK}/out.txt" 2>&1
  local rc=$?
  stop_mock
  if [ "${rc}" -eq "${expect_exit}" ]; then
    echo "PASS: ${name} (exit=${rc})"
  else
    echo "FAIL: ${name} (exit=${rc}, expected ${expect_exit})"
    sed 's/^/    /' "${WORK}/out.txt"
    FAILURES=$((FAILURES + 1))
  fi
}

echo "=== smoke.sh local logic test ==="

# 1. Fully healthy -> exit 0
run_case "healthy (readyz=200, ListBuckets=200)" 200 200 0 0

# 2. Auth-expired degraded -> exit 0 (report degraded)
run_case "degraded (readyz=503, ListBuckets=500)" 503 500 0 0

# 3. Dead process (no listener) -> exit 1
if ! start_mock; then
  echo "FAIL: dead (no listener) mock startup" >&2
  FAILURES=$((FAILURES + 1))
fi
CCBG_SMOKE_ENDPOINT="http://127.0.0.1:1" \
CCBG_SMOKE_METRICS_ENDPOINT="http://127.0.0.1:1" \
CCBG_SMOKE_REQUIRE_READY="0" \
bash "${SMOKE}" >"${WORK}/out.txt" 2>&1
rc=$?
stop_mock
if [ "${rc}" -eq 1 ]; then echo "PASS: dead (no listener) exit=1"; else echo "FAIL: dead exit=${rc}"; sed 's/^/    /' "${WORK}/out.txt"; FAILURES=$((FAILURES + 1)); fi

# 4. Strict mode + degraded -> exit 1
run_case "strict + degraded (readyz=503, require_ready=1)" 503 500 1 1

# 5. Strict mode + healthy -> exit 0
run_case "strict + healthy (require_ready=1)" 200 200 1 0

echo "=== done ==="

# Propagate any failure so the caller/CI sees a real broken signal.
if [ "${FAILURES}" -ne 0 ]; then
  echo "RESULT: FAIL (${FAILURES} failing case(s))" >&2
  exit 1
fi
echo "RESULT: PASS"
exit 0
