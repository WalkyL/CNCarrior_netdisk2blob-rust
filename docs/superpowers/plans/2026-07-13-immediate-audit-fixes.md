# Immediate Audit Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 4 immediate audit findings: secure cookie flag, CSP with nonce, remove sessionStorage carrier credentials, migrate std::sync::Mutex to tokio::sync::Mutex in AppState

**Architecture:** 4 independent fixes targeting security and concurrency safety. Each is self-contained with minimal cross-impact. The Mutex migration is the largest (touching ~80 lock sites) and requires async conversion of sync helpers.

**Tech Stack:** Rust (gatewayd crate, tokio, axum), JavaScript (admin/index.html), existing test infrastructure

## Global Constraints

- All changes in `crates/gatewayd/src/main.rs` and `crates/gatewayd/assets/admin/index.html`
- Maintain backward compatibility for existing deployments (configurable Secure flag)
- Zero CSP violations on admin console after fix
- No lock held across `.await` in Mutex migration
- `cargo test` passes at each commit
- `cargo clippy` clean at each commit

---

### Task 1: Add Secure Flag to Admin Session Cookie (Configurable)

**Files:**
- Modify: `crates/gatewayd/src/main.rs:7695-7703` (cookie header functions)
- Modify: `crates/gatewayd/src/main.rs:5700-5800` (config parsing for new env var)

**Interfaces:**
- Consumes: `CCBG_ADMIN_COOKIE_SECURE` env var (default `true`)
- Produces: `admin_session_cookie_header()` and `expire_cookie_header()` include `; Secure` when config true

- [ ] **Step 1: Add config field and env var parsing**

```rust
// In AppConfig struct (around line 5700-5800), add:
admin_cookie_secure: bool,
```

```rust
// In from_env() config loading, add:
admin_cookie_secure: env_bool("CCBG_ADMIN_COOKIE_SECURE", true),
```

- [ ] **Step 2: Write test for cookie header with Secure flag**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_session_cookie_header_includes_secure_when_enabled() {
        let header = admin_session_cookie_header("test-session", 3600);
        assert!(header.contains("Secure"));
    }

    #[test]
    fn admin_session_cookie_header_excludes_secure_when_disabled() {
        // Need to test with config where admin_cookie_secure = false
        // Will test after config is threaded through
    }
}
```

- [ ] **Step 3: Run test to verify it fails** (function doesn't accept config yet)

```bash
cargo test -p gatewayd admin_session_cookie_header -- --nocapture
# Expected: FAIL - function signature doesn't match
```

- [ ] **Step 4: Update cookie functions to accept config**

```rust
// Modify both functions to take config parameter
fn admin_session_cookie_header(config: &AppConfig, session_id: &str, ttl_seconds: u64) -> String {
    let secure_flag = if config.admin_cookie_secure { "; Secure" } else { "" };
    format!(
        "{ADMIN_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={ttl_seconds}{secure_flag}"
    )
}

fn expire_cookie_header(config: &AppConfig, cookie_name: &str) -> String {
    let secure_flag = if config.admin_cookie_secure { "; Secure" } else { "" };
    format!("{cookie_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure_flag}")
}
```

- [ ] **Step 5: Update all call sites** (search for `admin_session_cookie_header` and `expire_cookie_header`)

```bash
# Find call sites:
# grep -n "admin_session_cookie_header\|expire_cookie_header" crates/gatewayd/src/main.rs
```

- [ ] **Step 6: Run test to verify pass**

```bash
cargo test -p gatewayd admin_session_cookie_header -- --nocapture
```

- [ ] **Step 7: Commit**

```bash
git add crates/gatewayd/src/main.rs
git commit -m "feat: add Secure flag to admin session cookie (configurable via CCBG_ADMIN_COOKIE_SECURE)"
```

---

### Task 2: CSP Header with Nonce for Admin Console

**Files:**
- Modify: `crates/gatewayd/src/main.rs:13145-13215` (admin HTML rendering + handler)
- Modify: `crates/gatewayd/assets/admin/index.html` (add nonce placeholder in script/style tags)

**Interfaces:**
- Consumes: `random_urlsafe_token(32)` (existing, line 7753)
- Produces: CSP header with nonce, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`

- [ ] **Step 1: Add nonce placeholder to admin HTML template**

In `crates/gatewayd/assets/admin/index.html`, find the main `<script>` tag (around line where JS starts) and add `nonce="{csp_nonce}"`:
```html
<script nonce="{csp_nonce}">
```
Also find any `<style>` tags and add `nonce="{csp_nonce}"`.

- [ ] **Step 2: Write test for CSP header presence**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[tokio::test]
    async fn admin_index_returns_csp_header_with_nonce() {
        let state = test_app_state().await;
        let response = admin_index(axum::extract::State(state)).await;
        let headers = response.headers();
        
        let csp = headers.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("script-src 'self' 'nonce-"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("base-uri 'none'"));
        
        let xfo = headers.get("x-frame-options").unwrap().to_str().unwrap();
        assert_eq!(xfo, "DENY");
        
        let nosniff = headers.get("x-content-type-options").unwrap().to_str().unwrap();
        assert_eq!(nosniff, "nosniff");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p gatewayd admin_index_returns_csp_header_with_nonce -- --nocapture
# Expected: FAIL - handler doesn't return CSP headers
```

- [ ] **Step 4: Modify render function to accept nonce and inject**

```rust
// Change signature:
fn render_admin_index_html_from_template_source(template_source: &str, nonce: &str) -> String {
    template_source
        .replace("{csp_nonce}", nonce)
        // ... existing replaces
}

// And:
fn render_admin_index_html(nonce: &str) -> String {
    render_admin_index_html_from_template_source(&admin_index_template_source(), nonce)
}
```

- [ ] **Step 5: Modify admin_index handler to generate nonce, add headers**

```rust
async fn admin_index(State(state): State<AppState>) -> Response {
    let nonce = random_urlsafe_token(32);  // existing function line 7753
    let html = render_admin_index_html(&nonce);
    
    let mut response = Html(html).into_response();
    let headers = response.headers_mut();
    
    // CSP with nonce
    let csp = format!(
        "default-src 'self'; script-src 'self' 'nonce-{}'; style-src 'self' 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        nonce
    );
    headers.insert("content-security-policy", csp.parse().unwrap());
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert("referrer-policy", "strict-origin-when-cross-origin".parse().unwrap());
    
    response
}
```

- [ ] **Step 6: Run test to verify pass**

```bash
cargo test -p gatewayd admin_index_returns_csp_header_with_nonce -- --nocapture
```

- [ ] **Step 7: Manual verification** - Start server, visit admin console, check DevTools → Console for CSP violations

- [ ] **Step 8: Commit**

```bash
git add crates/gatewayd/src/main.rs crates/gatewayd/assets/admin/index.html
git commit -m "feat: add CSP with nonce + security headers to admin console"
```

---

### Task 3: Remove sessionStorage for Carrier Auth Credentials

**Files:**
- Modify: `crates/gatewayd/assets/admin/index.html` (lines ~5909-6046, ~6929-6931)

**Interfaces:**
- Produces: Carrier auth state lives only in JS module scope; no `sessionStorage` keys written/read

- [ ] **Step 1: Write test to verify no sessionStorage keys exist after auth**

```javascript
// In admin/index.html test section (if exists) or manual test:
// After completing Unicom/Telecom/Mobile auth flow:
// console.assert(!sessionStorage.getItem('ccbg.unicom.auth_assistant'));
// console.assert(!sessionStorage.getItem('ccbg.telecom.auth_assistant'));
// console.assert(!sessionStorage.getItem('ccbg.mobile.auth_assistant'));
```

- [ ] **Step 2: Delete persist functions** (lines ~5909-5976)

```javascript
// DELETE these 3 functions entirely:
function persistUnicomAuthAssistantState(state) { ... }
function persistTelecomAuthAssistantState(state) { ... }
function persistMobileAuthAssistantState(state) { ... }
```

- [ ] **Step 3: Delete restore functions** (lines ~5977-6046)

```javascript
// DELETE these 3 functions entirely:
function restoreUnicomAuthAssistantState() { ... }
function restoreTelecomAuthAssistantState() { ... }
function restoreMobileAuthAssistantState() { ... }
```

- [ ] **Step 4: Delete init calls** (lines ~6929-6931)

```javascript
// DELETE these lines:
restoreUnicomAuthAssistantState();
restoreTelecomAuthAssistantState();
restoreMobileAuthAssistantState();
```

- [ ] **Step 5: Verify no other references to these functions exist**

```bash
grep -n "persist.*AuthAssistant\|restore.*AuthAssistant" crates/gatewayd/assets/admin/index.html
# Should return nothing
```

- [ ] **Step 6: Verify state variables remain as module-scoped lets**

```javascript
// These should REMAIN (module scope, in-memory only):
let unicomAuthAssistantState = { ... };
let telecomAuthAssistantState = { ... };
let mobileAuthAssistantState = { ... };
```

- [ ] **Step 7: Manual test** - Open admin console, complete carrier auth flow, refresh page, verify re-auth required but no JS errors

- [ ] **Step 8: Commit**

```bash
git add crates/gatewayd/assets/admin/index.html
git commit -m "feat: remove sessionStorage persistence for carrier auth credentials"
```

---

### Task 4: Migrate AppState Mutex to tokio::sync::Mutex

**Files:**
- Modify: `crates/gatewayd/src/main.rs` (AppState struct + ~80 lock sites)

**Interfaces:**
- Consumes: `tokio::sync::Mutex` import
- Produces: All `.lock()` calls become `.lock().await`; no lock held across `.await`

- [ ] **Step 1: Add import and change AppState fields**

```rust
// Add import near line 129:
use tokio::sync::Mutex;

// Change all 9 fields in AppState (lines 234, 238-246, 256):
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
```

- [ ] **Step 2: Write test for one lock site to verify async signature**

```rust
#[tokio::test]
async fn control_plane_lock_is_async() {
    let state = test_app_state().await;
    let guard = state.control_plane.lock().await;  // Should compile
    drop(guard);
}
```

- [ ] **Step 4: Bulk convert lock sites** - Use search/replace pattern

Pattern: `state\.field\.lock\(\)\.expect\("([^"]+)"\)`
Replace: `state.field.lock().await`

But must do carefully - each site needs manual verification that:
1. The lock is NOT held across an `.await` point
2. The containing function becomes `async` if it wasn't already

Key lock sites to convert (verify each):
- `control_plane`: lines 7801, 7815, 8759, 13574, 17322, 17333, 17347, 17404, 17625, 18245
- `notify_state`: lines 12806, 12836, 12856, 12882
- `gateway_backup_runtime`: lines 27208, 27232
- `credential_lease_probe`: lines 12741, 12753, 12763, 12779, 12806
- `provider_cdp_keepalive`: lines 12772, 17107
- `admin_sessions`: lines 7741, 7759, 7780
- `backends`: lines 18214, 18233
- `gateway_write_ahead_log_runtime`: ~10 sites
- `external_kms_runtime`: ~5 sites

For EACH site:
- [ ] Verify block scoping (guard drops before any `.await`)
- [ ] Change `.lock().expect("...")` to `.lock().await`
- [ ] If parent fn is sync, make it `async fn` and update all callers
- [ ] Run `cargo check` after each batch of 5-10 conversions

- [ ] **Step 5: Full test suite pass**

```bash
cargo test -p gatewayd
# All tests must pass
```

- [ ] **Step 6: Clippy clean**

```bash
cargo clippy -p gatewayd
# No warnings
```

- [ ] **Step 7: Commit**

```bash
git add crates/gatewayd/src/main.rs
git commit -m "refactor: migrate AppState Mutex to tokio::sync::Mutex (deadlock-safe)"
```

---

## Spec Coverage Check

| Spec Section | Task |
|--------------|------|
| Secure cookie flag | Task 1 |
| CSP with nonce + security headers | Task 2 |
| Remove sessionStorage carrier credentials | Task 3 |
| std::sync::Mutex → tokio::sync::Mutex | Task 4 |

All 4 spec items covered.

## Type Consistency Check

- `admin_session_cookie_header` signature change: Task 1 defines new signature, all call sites updated in same task
- `render_admin_index_html` signature change: Task 2 defines new signature with nonce param, handler updated in same task
- `Mutex` type change: Task 4 changes all 9 fields + 80 lock sites consistently
- No cross-task type dependencies (4 tasks are independent)

## Execution Order

Tasks 1, 2, 3 are independent frontend/security fixes — can run in any order or parallel.
Task 4 (Mutex migration) is largest and touches most of main.rs — do last to avoid merge conflicts.

---

**Plan saved to:** `docs/superpowers/plans/2026-07-13-immediate-audit-fixes.md`

**Two execution options:**

1. **Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration
2. **Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**