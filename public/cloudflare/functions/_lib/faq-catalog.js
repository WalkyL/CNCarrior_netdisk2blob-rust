// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import catalog from "../../data/faq-catalog.json";

export const FAQ_CATALOG = Array.isArray(catalog?.items) ? catalog.items : [];
export const FAQ_CATALOG_VERSION = String(catalog?.version || "unknown");

export const DEFAULT_MATCH_LIMIT = 5;
export const MAX_MATCH_LIMIT = 10;

export const DEFAULT_WEIGHTS = Object.freeze({
  keyword: 5,
  provider: 4,
  context: 3,
  configKey: 3,
  errorPattern: 6
});

export function normalizeTokenList(value) {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((item) => String(item || "").trim().toLowerCase())
    .filter(Boolean);
}

export function safeLower(value) {
  return String(value || "").trim().toLowerCase();
}
