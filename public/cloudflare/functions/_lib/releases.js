// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

const DEFAULT_RELEASE_REPO = "WalkyL/CNCarrior_netdisk2blob-rust";
const RELEASE_CACHE_TTL_MS = 10 * 60 * 1000;
const RELEASE_DOWNLOAD_CACHE_CONTROL = "public, max-age=300";

let latestReleaseCache = null;

function buildGitHubReleaseHeaders(env, accept) {
  const headers = new Headers({
    accept,
    "user-agent": "ccbg-public-release-proxy/2026-06-01"
  });
  const token = String(env?.GITHUB_RELEASE_TOKEN || "").trim();
  if (token) {
    headers.set("authorization", `Bearer ${token}`);
  }
  return headers;
}

function normalizeReleaseRepo(value) {
  const raw = String(value || "").trim();
  if (!raw) {
    return DEFAULT_RELEASE_REPO;
  }
  return /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(raw) ? raw : DEFAULT_RELEASE_REPO;
}

export function currentReleaseRepo(env) {
  return normalizeReleaseRepo(env?.PUBLIC_RELEASE_REPO);
}

export function buildLatestReleasePageUrl(env) {
  return buildLatestReleaseUrlForRepo(currentReleaseRepo(env));
}

function buildLatestReleaseUrlForRepo(repo) {
  return `https://github.com/${repo}/releases/latest`;
}

function buildLatestReleaseApiUrlForRepo(repo) {
  return `https://api.github.com/repos/${repo}/releases/latest`;
}

function buildLatestReleaseDownloadUrl(repo, name) {
  return `${buildLatestReleaseUrlForRepo(repo)}/download/${encodeURIComponent(String(name || "").trim())}`;
}

function buildLatestReleaseR2Key(name) {
  return `latest/${String(name || "").trim()}`;
}

function isSafeReleaseAssetName(name) {
  return !!name && !/[\\/]/.test(name);
}

function normalizeUrl(value) {
  const raw = String(value || "").trim();
  if (!raw) {
    return null;
  }
  try {
    const url = new URL(raw);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}

function normalizeGitHubLatestReleaseAsset(asset) {
  const name = String(asset?.name || "").trim();
  if (!isSafeReleaseAssetName(name)) {
    return null;
  }
  return {
    name,
    api_url: normalizeUrl(asset?.url),
    browser_download_url: normalizeUrl(asset?.browser_download_url)
  };
}

async function selectLatestReleaseAssets(env) {
  const now = Date.now();
  const repo = currentReleaseRepo(env);
  if (latestReleaseCache && latestReleaseCache.expires_at > now && latestReleaseCache.repo === repo) {
    return latestReleaseCache.assets;
  }

  try {
    const response = await fetch(buildLatestReleaseApiUrlForRepo(repo), {
      headers: buildGitHubReleaseHeaders(env, "application/vnd.github+json")
    });
    if (!response.ok) {
      throw new Error(`github_release_http_${response.status}`);
    }
    const payload = await response.json();
    const assets = (Array.isArray(payload?.assets) ? payload.assets : [])
      .map((asset) => normalizeGitHubLatestReleaseAsset(asset))
      .filter(Boolean);
    latestReleaseCache = {
      expires_at: now + RELEASE_CACHE_TTL_MS,
      repo,
      assets
    };
    return assets;
  } catch {
    latestReleaseCache = {
      expires_at: now + RELEASE_CACHE_TTL_MS,
      repo,
      assets: []
    };
    return [];
  }
}

async function findLatestReleaseAssetByName(env, assetName) {
  const wanted = String(assetName || "").trim();
  if (!isSafeReleaseAssetName(wanted)) {
    return null;
  }
  const assets = await selectLatestReleaseAssets(env);
  return assets.find((asset) => asset.name === wanted) || null;
}

async function tryReleaseAssetR2Download(env, assetName, method) {
  if (!env?.RELEASE_ASSETS) {
    return null;
  }
  const normalizedName = String(assetName || "").trim();
  if (!isSafeReleaseAssetName(normalizedName)) {
    return null;
  }
  const key = buildLatestReleaseR2Key(normalizedName);
  const object = method === "HEAD"
    ? await env.RELEASE_ASSETS.head(key)
    : await env.RELEASE_ASSETS.get(key);
  if (!object) {
    return null;
  }
  const headers = new Headers();
  object.writeHttpMetadata(headers);
  headers.set("accept-ranges", "bytes");
  headers.set("cache-control", headers.get("cache-control") || RELEASE_DOWNLOAD_CACHE_CONTROL);
  headers.set("content-length", String(object.size));
  headers.set("etag", object.httpEtag);
  headers.set("x-ccbg-release-source", "r2");
  if (!headers.has("content-type")) {
    headers.set("content-type", "application/octet-stream");
  }
  if (!headers.has("content-disposition")) {
    headers.set("content-disposition", `attachment; filename="${normalizedName.replace(/"/g, "")}"`);
  }
  const body = method === "HEAD" ? null : object.body;
  return new Response(body, {
    status: 200,
    headers
  });
}

async function proxyBinaryDownload(url, env, options = {}) {
  const requestHeaders = buildGitHubReleaseHeaders(env, options.accept || "application/octet-stream");
  const range = String(options.range || "").trim();
  if (range) {
    requestHeaders.set("range", range);
  }
  const upstream = await fetch(url, {
    method: options.method || "GET",
    redirect: "follow",
    headers: requestHeaders
  });
  const headers = new Headers(upstream.headers);
  headers.set("cache-control", RELEASE_DOWNLOAD_CACHE_CONTROL);
  headers.set("x-ccbg-release-source", options.source || "github");
  if (options.filename && !headers.has("content-disposition")) {
    headers.set("content-disposition", `attachment; filename="${options.filename.replace(/"/g, "")}"`);
  }
  return new Response(upstream.body, {
    status: upstream.status,
    headers
  });
}

async function tryEdgeCacheMatch(request) {
  if (typeof caches === "undefined" || !caches.default) {
    return null;
  }
  try {
    return await caches.default.match(request);
  } catch {
    return null;
  }
}

function storeEdgeCache(request, response, ctx) {
  if (typeof caches === "undefined" || !caches.default || !ctx?.waitUntil) {
    return;
  }
  ctx.waitUntil(caches.default.put(request, response.clone()).catch(() => {}));
}

export async function proxyLatestReleaseAssetByName({ request, env, ctx, assetName }) {
  const normalizedName = String(assetName || "").trim();
  if (!isSafeReleaseAssetName(normalizedName)) {
    return new Response("missing release asset name", {
      status: 400,
      headers: {
        "content-type": "text/plain; charset=utf-8",
        "cache-control": "no-store"
      }
    });
  }

  const method = request.method === "HEAD" ? "HEAD" : "GET";
  const rangeHeader = String(request.headers.get("range") || "").trim();
  const isCacheableEdgeGet = method === "GET" && !rangeHeader;

  if (isCacheableEdgeGet) {
    const cached = await tryEdgeCacheMatch(request);
    if (cached) {
      return cached;
    }
  }

  if (!rangeHeader) {
    const cachedR2 = await tryReleaseAssetR2Download(env, normalizedName, method);
    if (cachedR2) {
      if (isCacheableEdgeGet && cachedR2.ok) {
        storeEdgeCache(request, cachedR2, ctx);
      }
      return cachedR2;
    }
  }

  const asset = await findLatestReleaseAssetByName(env, normalizedName);
  const hasReleaseToken = Boolean(String(env?.GITHUB_RELEASE_TOKEN || "").trim());

  let response;
  if (asset?.api_url && hasReleaseToken) {
    response = await proxyBinaryDownload(asset.api_url, env, {
      accept: "application/octet-stream",
      filename: asset.name,
      method,
      range: rangeHeader,
      source: "github-api"
    });
  } else if (asset?.browser_download_url) {
    response = await proxyBinaryDownload(asset.browser_download_url, env, {
      accept: "application/octet-stream",
      filename: asset.name,
      method,
      range: rangeHeader,
      source: "github-browser"
    });
  } else {
    response = await proxyBinaryDownload(buildLatestReleaseDownloadUrl(currentReleaseRepo(env), normalizedName), env, {
      accept: "application/octet-stream",
      filename: normalizedName,
      method,
      range: rangeHeader,
      source: "github-latest"
    });
    const contentType = String(response.headers.get("content-type") || "").toLowerCase();
    if (!hasReleaseToken && (response.status >= 400 || contentType.includes("text/html"))) {
      return new Response(
        "release asset lookup could not be resolved through Cloudflare; configure PUBLIC_RELEASE_REPO for the public mirror, or set GITHUB_RELEASE_TOKEN when the release source stays private",
        {
          status: 503,
          headers: {
            "content-type": "text/plain; charset=utf-8",
            "cache-control": "no-store"
          }
        }
      );
    }
  }

  if (isCacheableEdgeGet && response.ok) {
    storeEdgeCache(request, response, ctx);
  }
  return response;
}
