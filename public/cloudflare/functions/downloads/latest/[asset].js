// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import { proxyLatestReleaseAssetByName } from "../../_lib/releases.js";

async function handleReleaseDownload(context) {
  return proxyLatestReleaseAssetByName({
    request: context.request,
    env: context.env,
    ctx: context,
    assetName: context.params.asset
  });
}

export async function onRequestGet(context) {
  return handleReleaseDownload(context);
}

export async function onRequestHead(context) {
  return handleReleaseDownload(context);
}
