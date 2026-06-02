// SPDX-License-Identifier: LicenseRef-CCBG-Public-Materials
// Copyright (c) 2026 walky

import { buildLatestReleasePageUrl } from "../../_lib/releases.js";

function redirectToLatestRelease(context) {
  return Response.redirect(buildLatestReleasePageUrl(context.env), 302);
}

export async function onRequestGet(context) {
  return redirectToLatestRelease(context);
}

export async function onRequestHead(context) {
  return redirectToLatestRelease(context);
}
