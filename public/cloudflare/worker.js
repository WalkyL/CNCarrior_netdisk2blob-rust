// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import catalog from "./data/faq-catalog.json";

const FAQ_CATALOG = Array.isArray(catalog?.items) ? catalog.items : [];
const FAQ_CATALOG_VERSION = String(catalog?.version || "unknown");

const DEFAULT_MATCH_LIMIT = 5;
const MAX_MATCH_LIMIT = 10;

const DEFAULT_WEIGHTS = Object.freeze({
  keyword: 5,
  provider: 4,
  context: 3,
  configKey: 3,
  errorPattern: 6
});

const CORS_HEADERS = Object.freeze({
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, OPTIONS",
  "access-control-allow-headers": "content-type"
});

function normalizeTokenList(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((item) => String(item || "").trim().toLowerCase())
    .filter(Boolean);
}

function safeLower(value) {
  return String(value || "").trim().toLowerCase();
}

function parseMatchBody(body) {
  const query = safeLower(body?.query || body?.error_text || "");
  const provider = safeLower(body?.provider || "");
  const context = safeLower(body?.context || "");
  const configKeys = normalizeTokenList(body?.config_keys || body?.configKeys || []);
  const limit = Number.isFinite(Number(body?.limit))
    ? Math.max(1, Math.min(MAX_MATCH_LIMIT, Math.floor(Number(body.limit))))
    : DEFAULT_MATCH_LIMIT;
  return { query, provider, context, configKeys, limit };
}

function countMatches(haystackTokens, query) {
  if (!query) {
    return 0;
  }
  return haystackTokens.reduce((count, token) => (
    token && query.includes(token) ? count + 1 : count
  ), 0);
}

function scoreItem(item, request) {
  const keywords = normalizeTokenList(item?.keywords);
  const providers = normalizeTokenList(item?.provider);
  const contexts = normalizeTokenList(item?.context);
  const configKeys = normalizeTokenList(item?.config_keys);
  const patterns = normalizeTokenList(item?.error_patterns);

  const keywordHits = countMatches(keywords, request.query);
  const errorHits = countMatches(patterns, request.query);
  const providerHit = request.provider && providers.includes(request.provider) ? 1 : 0;
  const contextHit = request.context && contexts.includes(request.context) ? 1 : 0;
  const configHits = request.configKeys.reduce((count, key) => (
    configKeys.includes(key) ? count + 1 : count
  ), 0);

  const score = (
    keywordHits * DEFAULT_WEIGHTS.keyword +
    errorHits * DEFAULT_WEIGHTS.errorPattern +
    providerHit * DEFAULT_WEIGHTS.provider +
    contextHit * DEFAULT_WEIGHTS.context +
    configHits * DEFAULT_WEIGHTS.configKey
  );

  return {
    ...item,
    score,
    _debug: {
      keyword_hits: keywordHits,
      error_pattern_hits: errorHits,
      provider_hit: providerHit,
      context_hit: contextHit,
      config_key_hits: configHits
    }
  };
}

function jsonResponse(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body, null, 2), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      ...headers
    }
  });
}

async function handleFaqMatch(request) {
  if (request.method === "OPTIONS") {
    return new Response(null, {
      status: 204,
      headers: {
        ...CORS_HEADERS,
        "cache-control": "no-store"
      }
    });
  }
  if (request.method !== "POST") {
    return jsonResponse({ error: "method not allowed" }, 405, CORS_HEADERS);
  }

  let payload = {};
  try {
    payload = await request.json();
  } catch (_error) {
    return jsonResponse({ error: "invalid JSON body" }, 400, CORS_HEADERS);
  }

  const matchRequest = parseMatchBody(payload);
  const scored = FAQ_CATALOG
    .map((item) => scoreItem(item, matchRequest))
    .filter((item) => item.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, matchRequest.limit);

  return jsonResponse({
    version: FAQ_CATALOG_VERSION,
    weights: DEFAULT_WEIGHTS,
    query: {
      provider: matchRequest.provider || null,
      context: matchRequest.context || null,
      config_keys: matchRequest.configKeys,
      q: matchRequest.query,
      limit: matchRequest.limit
    },
    hits: scored
  }, 200, CORS_HEADERS);
}

function handleFaqCatalog(request) {
  if (request.method !== "GET") {
    return jsonResponse({ error: "method not allowed" }, 405, CORS_HEADERS);
  }
  return jsonResponse({
    version: FAQ_CATALOG_VERSION,
    count: FAQ_CATALOG.length,
    items: FAQ_CATALOG
  }, 200, { "cache-control": "public, max-age=120" });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/api/faq/catalog") {
      return handleFaqCatalog(request);
    }
    if (url.pathname === "/api/faq/match") {
      return handleFaqMatch(request);
    }
    return env.ASSETS.fetch(request);
  }
};
