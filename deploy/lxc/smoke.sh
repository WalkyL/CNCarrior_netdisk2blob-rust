#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
#
# Deploy-acceptance smoke for a CCBG LXC guest.
#
# Two dimensions are checked separately so that provider auth expiry (readyz=503,
# data plane degraded) is NOT conflated with a broken deployment:
#
#   Liveness  (hard gate): the process is up and its health/metrics/S3 listeners
#             answer. Any connection failure or missing listener fails the smoke.
#   Readiness (report):     readyz and ListBuckets status are reported. 200 =
#             ready, 503/500 = degraded (e.g. provider auth expired) which is a
#             valid runtime state, not a deployment failure.
#
# Set CCBG_SMOKE_REQUIRE_READY=1 to also require readyz=200 AND ListBuckets=200
# (strict mode for validating a fully healthy environment).
set -euo pipefail

ENV_FILE="${CCBG_ENV_FILE:-/etc/ccbg/ccbg.env}"
if [ -f "${ENV_FILE}" ]; then
  set -a
  # shellcheck disable=SC1090
  source "${ENV_FILE}"
  set +a
fi

endpoint="${CCBG_SMOKE_ENDPOINT:-http://127.0.0.1:61080}"
metrics_endpoint="${CCBG_SMOKE_METRICS_ENDPOINT:-http://127.0.0.1:61083}"
access_key="${CCBG_SMOKE_ACCESS_KEY_ID:-${CCBG_S3_ACCESS_KEY_ID:-ccbg}}"
secret_key="${CCBG_SMOKE_SECRET_ACCESS_KEY:-${CCBG_S3_SECRET_ACCESS_KEY:-change-me}}"
region="${CCBG_SMOKE_REGION:-${CCBG_S3_REGION:-us-east-1}}"
control_api_key="${CCBG_SMOKE_CONTROL_API_KEY:-${CCBG_CONTROL_API_KEY:-}}"
require_ready="${CCBG_SMOKE_REQUIRE_READY:-0}"

failed=0
degraded=0

# --- Liveness: /healthz must answer 200 (process alive) -----------------------
healthz_status="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 "${endpoint}/healthz" 2>/dev/null || echo "000")"
if [ "${healthz_status}" != "200" ]; then
  echo "LIVENESS FAIL: /healthz did not return 200 (status=${healthz_status}) endpoint=${endpoint}/healthz" >&2
  failed=1
else
  echo "LIVENESS OK: /healthz 200 (process alive)"
fi

# --- Readiness: /readyz is reported; 503 is a valid degraded state ------------
# Only attach the x-api-key header when a control API key is configured; an
# empty key must omit the header (an empty x-api-key: is not the same as no
# header and can be rejected as 401 by some servers).
readyz_args=(--max-time 5)
if [ -n "${control_api_key}" ]; then
  readyz_args+=(-H "x-api-key: ${control_api_key}")
fi
readyz_status="$(curl -sS -o /dev/null -w '%{http_code}' "${readyz_args[@]}" "${metrics_endpoint}/readyz" 2>/dev/null || echo "000")"
case "${readyz_status}" in
  200)
    echo "READINESS: ready (readyz 200)"
    ;;
  503)
    echo "READINESS: degraded (readyz 503 = data plane not-ready, e.g. provider auth expired; process alive)"
    degraded=1
    if [ "${require_ready}" = "1" ]; then
      echo "READINESS FAIL: CCBG_SMOKE_REQUIRE_READY=1 requested but readyz=503" >&2
      failed=1
    fi
    ;;
  401)
    cat >&2 <<'EOF'
READINESS FAIL: metrics readyz rejected the smoke probe with 401 Unauthorized.
Set CCBG_CONTROL_API_KEY in /etc/ccbg/ccbg.env, or override it for this run with
CCBG_SMOKE_CONTROL_API_KEY=<control-api-key>, so smoke.sh can probe /readyz with x-api-key.
EOF
    failed=1
    ;;
  *)
    echo "LIVENESS FAIL: metrics /readyz did not answer (status=${readyz_status}) endpoint=${metrics_endpoint}/readyz" >&2
    failed=1
    ;;
esac

# --- S3 data plane: SigV4 ListBuckets must answer; 4xx/5xx is a live handler --
set +e
listbuckets_result="$(python3 - "${endpoint}" "${access_key}" "${secret_key}" "${region}" 2>&1 <<'PY'
import datetime as dt
import hashlib
import hmac
import http.client
import sys
import urllib.parse

endpoint, access_key, secret_key, region = sys.argv[1:5]
parsed = urllib.parse.urlparse(endpoint)
host = parsed.netloc
now = dt.datetime.now(dt.timezone.utc)
amz_date = now.strftime("%Y%m%dT%H%M%SZ")
datestamp = now.strftime("%Y%m%d")
payload_hash = hashlib.sha256(b"").hexdigest()
canonical_request = "\n".join([
    "GET",
    "/",
    "",
    f"host:{host}\n" f"x-amz-content-sha256:{payload_hash}\n" f"x-amz-date:{amz_date}\n",
    "host;x-amz-content-sha256;x-amz-date",
    payload_hash,
])
scope = f"{datestamp}/{region}/s3/aws4_request"
string_to_sign = "\n".join([
    "AWS4-HMAC-SHA256",
    amz_date,
    scope,
    hashlib.sha256(canonical_request.encode()).hexdigest(),
])
k_date = hmac.new(("AWS4" + secret_key).encode(), datestamp.encode(), hashlib.sha256).digest()
k_region = hmac.new(k_date, region.encode(), hashlib.sha256).digest()
k_service = hmac.new(k_region, b"s3", hashlib.sha256).digest()
k_signing = hmac.new(k_service, b"aws4_request", hashlib.sha256).digest()
signature = hmac.new(k_signing, string_to_sign.encode(), hashlib.sha256).hexdigest()
headers = {
    "host": host,
    "x-amz-content-sha256": payload_hash,
    "x-amz-date": amz_date,
    "Authorization": (
        "AWS4-HMAC-SHA256 "
        f"Credential={access_key}/{scope}, "
        "SignedHeaders=host;x-amz-content-sha256;x-amz-date, "
        f"Signature={signature}"
    ),
}
try:
    conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=10)
    conn.request("GET", "/", headers=headers)
    response = conn.getresponse()
    body = response.read().decode("utf-8", errors="replace")
    conn.close()
except Exception as error:  # noqa: BLE001 - report any transport failure as unreachable
    print(f"UNREACHABLE {error}", file=sys.stderr)
    sys.exit(2)
print(f"{response.status} {body[:200]}")
PY
)"
listbuckets_rc=$?
set -e
if [ "${listbuckets_rc}" = "2" ]; then
  echo "LIVENESS FAIL: S3 data plane did not answer ListBuckets (${listbuckets_result})" >&2
  failed=1
else
  listbuckets_status="$(echo "${listbuckets_result}" | awk '{print $1}')"
  if [ "${listbuckets_status}" = "200" ]; then
    echo "DATA PLANE: ListBuckets 200 (ready)"
  elif [ "${listbuckets_status}" = "403" ] || [ "${listbuckets_status}" = "500" ]; then
    echo "DATA PLANE: degraded (ListBuckets ${listbuckets_status}; live handler, e.g. wrong key or provider auth expired)"
    degraded=1
    if [ "${require_ready}" = "1" ]; then
      echo "DATA PLANE FAIL: CCBG_SMOKE_REQUIRE_READY=1 requested but ListBuckets=${listbuckets_status}" >&2
      failed=1
    fi
  else
    echo "DATA PLANE: unexpected (ListBuckets ${listbuckets_status})"
    failed=1
  fi
fi

if [ "${failed}" = "1" ]; then
  echo "ccbg LXC smoke FAILED: liveness ${endpoint}" >&2
  exit 1
fi

if [ "${degraded}" = "1" ]; then
  echo "ccbg LXC smoke PASSED (deployed & alive; data plane degraded): ${endpoint}"
else
  echo "ccbg LXC smoke passed: ${endpoint}"
fi
