#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
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

curl -fsS --max-time 5 "${endpoint}/healthz" >/dev/null

readyz_args=(--max-time 5)
if [ -n "${control_api_key}" ]; then
  readyz_args+=(-H "x-api-key: ${control_api_key}")
fi
readyz_status="$(curl -sS -o /dev/null -w '%{http_code}' "${readyz_args[@]}" "${metrics_endpoint}/readyz")"
case "${readyz_status}" in
  200)
    ;;
  401)
    cat >&2 <<'EOF'
metrics readyz rejected the smoke probe with 401 Unauthorized.
Set CCBG_CONTROL_API_KEY in /etc/ccbg/ccbg.env, or override it for this run with
CCBG_SMOKE_CONTROL_API_KEY=<control-api-key>, so smoke.sh can probe /readyz with x-api-key.
EOF
    exit 1
    ;;
  *)
    echo "metrics readyz probe failed: status=${readyz_status} endpoint=${metrics_endpoint}/readyz" >&2
    exit 1
    ;;
esac

python3 - "${endpoint}" "${access_key}" "${secret_key}" "${region}" <<'PY'
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
conn = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=10)
try:
    conn.request("GET", "/", headers=headers)
    response = conn.getresponse()
    body = response.read().decode("utf-8", errors="replace")
finally:
    conn.close()
if response.status != 200:
    raise SystemExit(f"ListBuckets failed: status={response.status} body={body[:300]}")
PY

echo "ccbg LXC smoke passed: ${endpoint}"
