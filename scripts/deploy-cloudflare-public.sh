#!/usr/bin/env bash
# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${1:-}"

usage() {
  echo "usage: scripts/deploy-cloudflare-public.sh <test|production>"
}

if [ "${TARGET}" != "test" ] && [ "${TARGET}" != "production" ]; then
  usage >&2
  exit 2
fi

cd "${ROOT_DIR}"
PYTHON_BIN="$(bash scripts/resolve-python.sh)"

export CLOUDFLARE_API_TOKEN="${CLOUDFLARE_API_TOKEN:-${CF_API_TOKEN:-}}"
export CLOUDFLARE_ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-${CF_ACCOUNT_ID:-}}"

if [ -z "${CLOUDFLARE_API_TOKEN}" ]; then
  echo "missing CLOUDFLARE_API_TOKEN or CF_API_TOKEN" >&2
  exit 1
fi
if [ -z "${CLOUDFLARE_ACCOUNT_ID}" ]; then
  echo "missing CLOUDFLARE_ACCOUNT_ID or CF_ACCOUNT_ID" >&2
  exit 1
fi

case "${TARGET}" in
  test)
    CCBG_CF_WORKER_NAME="${CCBG_CF_TEST_WORKER:-ccbg-public-test}"
    CCBG_CF_DOMAIN="${CCBG_CF_TEST_DOMAIN:-}"
    ;;
  production)
    CCBG_CF_WORKER_NAME="${CCBG_CF_PROD_WORKER:-ccbg-public}"
    CCBG_CF_DOMAIN="${CCBG_CF_PROD_DOMAIN:-carrier-disk-gateway.agi2030.online}"
    ;;
esac

OUT_DIR="${ROOT_DIR}/target/cloudflare-public-assets"
scripts/stage-cloudflare-public-assets.sh "${OUT_DIR}"

deploy_args=(
  npx wrangler@latest deploy
  -c public/cloudflare/wrangler.worker.toml
  --name "${CCBG_CF_WORKER_NAME}"
  --assets "${OUT_DIR}"
)

if [ "${CCBG_CF_BIND_DOMAIN_ON_DEPLOY:-false}" = "true" ] && [ -n "${CCBG_CF_DOMAIN}" ]; then
  deploy_args+=(--domain "${CCBG_CF_DOMAIN}")
fi

"${deploy_args[@]}"

if [ "${CCBG_CF_SMOKE_DOMAIN_ON_DEPLOY:-false}" = "true" ] && [ -n "${CCBG_CF_DOMAIN}" ]; then
  "${PYTHON_BIN}" scripts/check-cloudflare-public-fingerprint.py \
    --deployed-base-url "https://${CCBG_CF_DOMAIN}"
fi
