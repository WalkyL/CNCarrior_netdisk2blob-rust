# Immediate Audit Fixes — Design Spec

**Date:** 2026-07-13
**Source:** Comprehensive code audit (2026-07-13)
**Scope:** 4 immediate priority items from audit roadmap

---

## 1. Secure Admin Session Cookie

### Current State
```rust
// main.rs:7695-7703
fn admin_session_cookie_header(session_id: &str, ttl_seconds: u64) -> String {
    format!(
        "{ADMIN_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl_seconds}"
    )
}

fn expire_cookie_header(cookie_name: &str) -> String {
    format!("{cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}
```

### Design
- Add `; Secure` flag to both functions
- Cookie will only be transmitted over HTTPS (or via proxy terminating TLS)
- Matches existing `SameSite=Strict` and `HttpOnly` flags

### Risk Assessment
- **Low**: Current deployments use HTTPS termination. `SameSite=Strict` already prevents CSRF on cross-site requests.
- If any deployment runs plain-HTTP admin, this will break login — but such deployments are already insecure.

---

## 2. CSP Header with Nonce for Admin Console

### Current State
- Admin HTML served by `admin_index()` handler (`main.rs:13213-13215`)
- HTML is one large inline `<script>` block + inline styles (`assets/admin/index.html`, 20,878 lines)
- No CSP header, no `X-Frame-Options`, no `X-Content-Type-Options`, no HSTS

### Design
1. **Generate nonce per request** in `render_admin_index_html_from_template_source` or handler
2. **Inject nonce** into:
   - Main `<script nonce="...">` tag
   - Any `<style nonce="...">` tags (if present)
3. **Add CSP header** in response:
   ```
   Content-Security-Policy:
     default-src 'self';
     script-src 'self' 'nonce-{NONCE}';
     style-src 'self' 'unsafe-inline';
     frame-ancestors 'none';
     base-uri 'none';
     form-action 'self';
   ```
4. **Also add** defensive headers:
   - `X-Frame-Options: DENY`
   - `X-Content-Type-Options: nosniff`
   - `Referrer-Policy: strict-origin-when-cross-origin`

### Implementation Approach
- Modify `admin_index()` handler to generate nonce via `random_urlsafe_token(32)` (already exists at line 7753)
- Modify `render_admin_index_html_from_template_source` to accept nonce parameter and inject into template placeholders
- Update `index.html` template to include `{csp_nonce}` placeholder in script/style tags
- Add response headers via Axum's `ResponseBuilder` or custom middleware

### Risk Assessment
- **Medium**: Template injection changes; must ensure nonce uniqueness per request; test CSP compliance across all admin pages
- Styles use `'unsafe-inline'` per design choice (inline `style=""` attributes exist in HTML)

---

## 3. Remove sessionStorage for Carrier Credentials

### Current State (`assets/admin/index.html`)
- **State objects** (lines 5203-5245): `unicomAuthAssistantState`, `telecomAuthAssistantState`, `mobileAuthAssistantState` — plain JS objects in module scope
- **Persist functions** (lines 5909-5976): `persistUnicomAuthAssistantState`, `persistTelecomAuthAssistantState`, `persistMobileAuthAssistantState` — serialize state to `sessionStorage` keys
- **Restore functions** (lines 5977-6046): `restoreUnicomAuthAssistantState`, `restoreTelecomAuthAssistantState`, `restoreMobileAuthAssistantState` — parse from `sessionStorage`
- **Init calls** (lines 6929-6931): Three restore calls during app initialization

### Design
**Delete all persist/restore functions and the init calls.** The state objects already exist in memory (module-level `let` bindings). On page refresh, user simply re-enters phone number/SMS code — no persistent credential storage in browser.

### Fields No Longer Persisted
| Provider | Fields Removed from sessionStorage |
|----------|-----------------------------------|
| Unicom | `session_id`, `phone_number`, `session_expires_at_unix_ms`, `session_timeout_ms`, `window_open` |
| Telecom | `session_id`, `phone_number`, `upload_probe_snapshot_open`, `runtime_snapshot_open`, `window_open` |
| Mobile | `session_id`, `phone_number`, `upload_probe_snapshot_open`, `runtime_snapshot_open`, `window_open` |

Note: `sms_code` was never persisted (good); `catalog` and `session` were also never persisted.

### Risk Assessment
- **Low**: State was already in-memory; persistence was only convenience for page reloads. Re-auth on refresh is acceptable UX for admin console.
- No API changes — only frontend storage removal.

---

## 4. std::sync::Mutex → tokio::sync::Mutex in AppState

### Current State (`main.rs` lines 231-259)
```rust
struct AppState {
    // ... (9 Arc<Mutex<...>> fields)
    backends: Arc<Mutex<Vec<ConfiguredBackend>>>,
    control_plane: Arc<Mutex<ControlPlaneState>>,
    gateway_backup_runtime: Arc<Mutex<GatewayBackupRuntimeState>>,
    gateway_write_ahead_log_runtime: Arc<Mutex<GatewayWriteAheadLogRuntimeState>>,
    notify_state: Arc<Mutex<NotifyState>>,
    credential_lease_probe: Arc<Mutex<CredentialLeaseProbeState>>,
    provider_cdp_keepalive: Arc<Mutex<ProviderCdpKeepaliveState>>,
    admin_client_ip: Arc<Mutex<Option<String>>>,
    admin_sessions: Arc<Mutex<HashMap<String, AdminBrowserSession>>>,
    external_kms_runtime: Arc<Mutex<ExternalKmsRuntimeState>>,
    // ... other fields
}
```
All use `std::sync::Mutex` via implicit import (not shown in imports, but `.lock().expect(...)` pattern confirms it).

### Design
1. **Import**: Add `use tokio::sync::Mutex;`
2. **Change all 9 fields** in `AppState` to `tokio::sync::Mutex`
3. **Update all lock sites** (~80 call sites from grep) from:
   ```rust
   let mut guard = state.field.lock().expect("...");
   ```
   to:
   ```rust
   let mut guard = state.field.lock().await;
   ```
4. **Verify no lock is held across `.await`**: Current code uses block scoping (`{ let guard = ...; ... }`) — must audit each site to ensure the guard drops before any `.await`.
5. **Change return types** of synchronous helper functions that lock to `async` if they need to lock AppState fields.

### Lock Site Categories (from audit grep)
| Category | Example Lines | Count |
|----------|---------------|-------|
| `control_plane` | 7801, 7815, 8759, 13574, 17322, 17333, 17347, 17404, 17625 | ~12 |
| `notify_state` | 12806, 12836, 12856, 12882 | 4 |
| `gateway_backup_runtime` | 27208, 27232 | ~3 |
| `credential_lease_probe` | 12741, 12753, 12763, 12779, 12806 | ~6 |
| `provider_cdp_keepalive` | 12772, 17107 | ~3 |
| `admin_sessions` | 7741, 7759, 7780 | 3 |
| `backends` | 18214, 18233 | 2 |
| `gateway_write_ahead_log_runtime` | ~10 | ~10 |
| `external_kms_runtime` | ~5 | ~5 |
| **Total** | | **~80** |

### Risk Assessment
- **High**: 80+ call sites, must convert synchronous helpers to async where needed
- **Verification required**: No lock held across `.await` — block scoping must be preserved
- **Testing**: Run all integration tests; verify no deadlocks under load
- **Performance**: `tokio::sync::Mutex` is slightly slower but adds fairness/preemption safety

---

## Testing Strategy

1. **Secure cookie**: Verify cookie header includes `Secure` in admin login response
2. **CSP**: 
   - `curl -I /admin` shows CSP header with nonce
   - Load admin console, verify no console CSP violations
   - Verify `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`
3. **sessionStorage**: 
   - Open admin console, go to carrier auth pages
   - Refresh page, verify no `ccbg.*.auth_assistant` keys in DevTools → Application → Session Storage
   - Re-auth flow works after refresh
4. **Mutex migration**:
   - `cargo test` passes
   - `cargo clippy` clean
   - Run `eligible_write_candidates_for_object` tests (stale primary fencing)
   - Load test concurrent writes to verify no deadlocks

---

## Out of Scope (Deferred)

- All other audit findings (dead code, error handling, test gaps, config validation, etc.)
- HSTS header (requires TLS cert strategy)
- CSRF token (requires stateful token store)
- `await_holding_lock` lint (item 4 alternative — not needed if migration completes)

---

## Approval

- [x] Secure cookie flag
- [x] CSP with nonce (scripts), unsafe-inline (styles)
- [x] Remove sessionStorage carrier credentials
- [x] std::sync::Mutex → tokio::sync::Mutex (full migration)