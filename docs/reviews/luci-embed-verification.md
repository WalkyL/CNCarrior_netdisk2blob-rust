# Verification Evidence — Browser-Level E2E of LuCI Embed (方案 A)

This is **supplementary browser-level verification** for the LuCI-embed change
committed at `d69510b20548e3d4048844895d640b515faeeb27`. That change was already
accepted by the ADR-007 holistic review gate; its gate evidence is recorded in
[`docs/reviews/luci-embed-gatewayd-v6.md`](luci-embed-gatewayd-v6.md) (all three
high-tier reviewers — GLM-5.3, kimi-k3, astra — returned APPROVE). This document
supplies an additional browser-level end-to-end check that the earlier gate did
not include, because no OpenWRT test device was available.

- **Change verified:** LuCI iframe embed of the CCBG Admin SPA (方案 A),
  commit `d69510b20548e3d4048844895d640b515faeeb27`.
- **Verification date:** 2026-11-08
- **Method:** the real `gatewayd` binary (`target/debug/gatewayd.exe`) was
  started from the repository root, and a real headless browser (Microsoft Edge
  157) was driven via the Chrome DevTools Protocol (CDP) against the live
  admin endpoint.

## Scope statement (what this record does and does not verify)

This verification is **limited to browser-level behavior** of the iframe embed,
in the documented test environment. It does **not** verify any of the following
(these require an actual OpenWRT device):

- LuCI package installation (`opkg install` / build artifacts);
- the actual LuCI menu entry (`menu.d/*.json`);
- `rpcd` ACL integration (whether LuCI may call the gateway interfaces);
- a real uhttpd HTTP/HTTPS deployment and its interaction with cookie `Secure`.

The simulated parent pages (served over plain HTTP on loopback) substitute for
the LuCI host only insofar as they exercise the same browser mechanisms — CSP
`frame-ancestors`, cross-origin framing, and cookie scoping. The browser, not the
LuCI host, is what enforces those mechanisms.

## Environment under test

- Admin origin: `http://127.0.0.1:61081`
- Simulated LuCI parent (allowed): `http://127.0.0.1:8081`
- Simulated malicious parent (disallowed): `http://127.0.0.1:8082`
- Admin credentials: `admin` / `password` (test only, disposable)
- Transport: plain HTTP (`CCBG_ADMIN_COOKIE_SECURE=false`), matching the LAN
  deployment in `config/openwrt-lite.env`.
- The two parent origins are **cross-origin but same-site**: they share the host
  and scheme, differing only by port. Cookies are not scoped by port, so this
  arrangement exercises cross-origin framing and CSP, but is not a cross-site
  test.

## Test 1 — HTTP header assertions (real gatewayd)

The authenticated `GET /` (with a valid session cookie) returned:

```
content-security-policy = default-src 'self'; script-src 'self' 'nonce-…';
                         style-src 'self' 'unsafe-inline';
                         frame-ancestors http://127.0.0.1:8081;
                         base-uri 'none'; form-action 'self'
x-frame-options        = (empty — deliberately dropped so CSP governs)
access-control-allow-origin      = http://127.0.0.1:8081
access-control-allow-credentials = true
```

The unauthenticated `GET /` redirects (302) to `/login`. The `/login` page is
intentionally **unrestricted** (no `frame-ancestors` directive, no
`X-Frame-Options`) so the LuCI page can show the login form; only the
authenticated `/` SPA carries the `frame-ancestors` restriction.

**Negative control:** a POST to `/api/admin/login` carrying
`Origin: http://evil.example.com` (neither the configured ancestor nor the admin
origin) is rejected by the CSRF guard:

```
403 {"error":"cross-origin admin write rejected","code":"forbidden","api_version":"2026-05-26"}
```

## Test 2 — CSP frame-ancestors enforcement on the authenticated page (decisive)

The protected resource is the **authenticated** `/` SPA, which carries
`frame-ancestors http://127.0.0.1:8081`. A valid session cookie was seeded, then
each parent origin was loaded with an iframe pointing at the admin origin. The
decisive observation is whether the protected document **renders** in the iframe
and whether the browser reports a CSP block.

| Scenario | Parent origin | Protected `/` rendered? | Browser signal |
|----------|---------------|-------------------------|----------------|
| A — allowed | `http://127.0.0.1:8081` | Yes — `/?tab=dashboard`, title `Carrier Cloud Storage Gateway`, `<main>` present, 21 admin API requests fired | No block |
| B — disallowed | `http://127.0.0.1:8082` | No — no child frame rendered, zero admin API requests | `net::ERR_BLOCKED_BY_RESPONSE` |

Scenario A confirms the allowed parent embeds and renders the authenticated SPA
(including its API calls). Scenario B confirms the browser **issues** the request
to the admin origin but **refuses to render** it because of the
`frame-ancestors http://127.0.0.1:8081` directive, surfacing
`net::ERR_BLOCKED_BY_RESPONSE`. The `ERR_BLOCKED_BY_RESPONSE` error is the
browser's concrete enforcement of the CSP `frame-ancestors` directive (the
`X-Frame-Options` header is absent in this mode by design, so only CSP governs).

This is the correct mechanism to observe: `frame-ancestors` is evaluated when
the browser processes the protected response, so it does not suppress the initial
request — it refuses to render the response. An attempt to assert enforcement
via "zero requests issued" would be incorrect; the rendering refusal and the
`ERR_BLOCKED_BY_RESPONSE` signal are the valid evidence.

## Test 3 — Real login inside the allowed iframe

Driving the login form **inside** the cross-origin iframe (via a CDP isolated
world on the child frame):

1. The child frame loaded `CCBG Admin Login`; `#admin-login-form` was present.
2. Filled `admin` / `password`, clicked `#admin-login-submit`.
3. The SPA navigated to `/` and rendered the authenticated app:
   - `location.pathname` = `/`
   - `document.title` = `Carrier Cloud Storage Gateway`
   - `<main>` element present.

**Origin of the login request.** The login page is served at the admin origin,
so the SPA's `fetch` to `/api/admin/login` is a **same-origin** request: the
browser sends `Origin: http://127.0.0.1:61081` (the admin origin), not the
embedding parent's origin. The CSRF guard's **same-origin fallback** (Host header
matches the Origin authority, with the transport scheme from
`admin_cookie_secure`) accepts it. This was confirmed directly: a POST with
`Origin: http://127.0.0.1:61081` returns 200, and the configured-ancestor origin
also returns 200. The guard does not need the parent origin for the login POST
because the login page is same-origin with the admin endpoint.

Therefore, Test 3 establishes **successful authentication within the allowed
iframe**; it does not establish that the CSRF guard accepted the parent origin
(as opposed to the same-origin fallback), nor does it demonstrate cookie transfer
from the parent into the iframe. Those claims are deliberately not made here.

## Cookie attributes (independent HTTP check)

After login, the session cookie is:

```
ccbg_admin_session  HttpOnly=True  Secure=False
```

`Secure=False` is correct for the plain-HTTP LAN deployment
(`CCBG_ADMIN_COOKIE_SECURE=false`); `HttpOnly=True` means the empty
`document.cookie` observed in the browser is expected and is the intended
hardening.

## Conclusion

At the browser level, in the documented environment:

1. The allowed LuCI parent origin embeds, loads, and renders the authenticated
   Admin SPA.
2. A disallowed origin cannot render the protected SPA: the browser issues the
   request but refuses to render it, reporting `ERR_BLOCKED_BY_RESPONSE`, which
   is the CSP `frame-ancestors` enforcement.
3. A real login inside the allowed iframe authenticates and renders the SPA.

This verification is **browser-level only**. LuCI package installation, the
LuCI menu entry, `rpcd` ACL integration, and real uhttpd HTTPS remain unverified
and require an actual OpenWRT device.

No Rust code was modified; this is a verification-only record against the
already-gated commit `d69510b`.

## Artifacts

- `.cache/e2e-run/cdp_authenticated.py` — Test 2 (authenticated CSP enforcement)
- `.cache/e2e-run/cdp_login3.py` — Test 3 (login inside iframe)
- `.cache/e2e-run/cdp_origin.py` — Test 3 (login request Origin capture)
- This record — the consolidated evidence

## Review evidence (ADR-007 gate for this document)

This document is itself a `docs/` addition that the ADR-007 gate detector
classifies as material (it references `CCBG_*` and security keywords), so it was
holistically reviewed by all three high-tier reviewers on the same prompt hash
`59750e0e6ae8c634`:

| Reviewer | Verdict | Findings |
|----------|---------|----------|
| GLM-5.3  | APPROVE | 0 |
| kimi-k3  | APPROVE | 0 |
| astra    | APPROVE | 0 |

Raw verdict JSON and the prompt are retained in `.cache/reviews/`
(`luci-embed-verification-{astra,glm53,kimi}.json`, `...-prompt.md`).

## Reproducibility

- **gatewayd launch** (from the repository root):
  `CCBG_ADMIN_BIND_ADDR=127.0.0.1:61081 CCBG_ADMIN_MODE=terminal
  CCBG_ADMIN_COOKIE_SECURE=false
  CCBG_ADMIN_FRAME_ANCESTORS=http://127.0.0.1:8081
  CCBG_ADMIN_USERNAME=admin CCBG_ADMIN_PASSWORD=password
  CCBG_CONTROL_API_KEY=<test-key> CCBG_ONEDRIVE_ENABLED=false
  CCBG_METADATA_DB_PATH=.cache/e2e-run/data/ccbg.db
  CCBG_CONTROL_PLANE_FILE=.cache/e2e-run/data/control-plane.json
  CCBG_CREDENTIALS_DIR=.cache/e2e-run/data/provider-credentials
  CCBG_INSTANCE_ID_FILE=.cache/e2e-run/data/instance-id
  target/debug/gatewayd.exe`
- **browser invocation**: Edge headless with
  `--remote-debugging-port=9223 --user-data-dir=.cache/e2e-run/edge-profile`.
- **parent page** (`.cache/e2e-run/parent.html`): an `<iframe id="ccbg"
  src="http://127.0.0.1:61081/">` served from the parent origin (8081 allowed,
  8082 disallowed).
- **session state per scenario**: Test 2 seeds the `ccbg_admin_session` cookie
  on the admin origin before navigation (via CDP `Network.setCookie`); Test 3
  drives the login form. Authentication state must be established per scenario
  so the protected `/` (not the unrestricted `/login`) is what gets framed.
