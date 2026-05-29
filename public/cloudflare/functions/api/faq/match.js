// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import {
  DEFAULT_MATCH_LIMIT,
  DEFAULT_WEIGHTS,
  FAQ_CATALOG,
  FAQ_CATALOG_VERSION,
  MAX_MATCH_LIMIT,
  normalizeTokenList,
  safeLower
} from "../../_lib/faq-catalog.js";

function parseBody(body) {
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

function toResponse(body, status = 200) {
  return new Response(JSON.stringify(body, null, 2), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "access-control-allow-origin": "*",
      "access-control-allow-methods": "GET, POST, OPTIONS",
      "access-control-allow-headers": "content-type"
    }
  });
}

export async function onRequestOptions() {
  return new Response(null, {
    status: 204,
    headers: {
      "access-control-allow-origin": "*",
      "access-control-allow-methods": "GET, POST, OPTIONS",
      "access-control-allow-headers": "content-type",
      "cache-control": "no-store"
    }
  });
}

export async function onRequestPost(context) {
  let payload = {};
  try {
    payload = await context.request.json();
  } catch (_error) {
    return toResponse({ error: "invalid JSON body" }, 400);
  }
  const request = parseBody(payload);
  const scored = FAQ_CATALOG
    .map((item) => scoreItem(item, request))
    .filter((item) => item.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, request.limit);

  return toResponse({
    version: FAQ_CATALOG_VERSION,
    weights: DEFAULT_WEIGHTS,
    query: {
      provider: request.provider || null,
      context: request.context || null,
      config_keys: request.configKeys,
      q: request.query,
      limit: request.limit
    },
    hits: scored
  });
}
