// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import { FAQ_CATALOG, FAQ_CATALOG_VERSION } from "../../_lib/faq-catalog.js";

export async function onRequestGet() {
  return new Response(JSON.stringify({
    version: FAQ_CATALOG_VERSION,
    count: FAQ_CATALOG.length,
    items: FAQ_CATALOG
  }, null, 2), {
    status: 200,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "public, max-age=120"
    }
  });
}
