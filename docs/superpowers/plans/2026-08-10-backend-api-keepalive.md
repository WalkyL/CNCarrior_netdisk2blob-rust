# Backend API Keepalive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the CDP-based provider session keepalive with backend API probing by merging keepalive into the existing credential lease probe loop, and add a configurable probe interval.

**Architecture:** Delete `provider_cdp_keepalive_loop` and all CDP keepalive code (state, alert, config, status payload). The existing `credential_lease_loop` (which already calls `backend.health()` for Unicom/Telecom/Mobile and persists lease state) becomes the sole keepalive mechanism. Its per-provider next-probe advance (`5 * 60_000` ms) becomes configurable via a new env var; the reauth short-circuit stays at `60_000` ms. Admin status continues to surface the lease summaries (which already carry `last_verified_at_unix_ms`).

**Tech Stack:** Rust, ccbg gatewayd single-file binary (`crates/gatewayd/src/main.rs`), tokio, LXC deploy template (`deploy/lxc/ccbg.env`).

**Spec:** `docs/superpowers/specs/2026-08-10-backend-api-keepalive-design.md`

## Global Constraints

- All changes confined to `crates/gatewayd/src/main.rs` and `deploy/lxc/ccbg.env`. Do NOT modify `crates/provider-unicom`, `crates/provider-telecom`, `crates/provider-mobile`, or the Admin asset (`crates/gatewayd/assets/admin/index.html`).
- Browser-based login / re-authentication stays unchanged.
- Failure behavior unchanged: `requires_reauth` → primary stale → writes route to write targets → Admin alert.
- New env `CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS`, default `300`, floor `30`. Reauth advance stays hard-coded `60_000` ms.
- Every step compiles: run `cargo check -p gatewayd --bin gatewayd` after each code change.
- Full test gate before commit of each task: `cargo test -p gatewayd --bin gatewayd -- <test-group>`.

---

### Task 1: Add configurable lease probe interval

**Files:**
- Modify: `crates/gatewayd/src/main.rs` (config struct, env parse, test config constructors)

**Interfaces:**
- Produces: `AppConfig.provider_credential_lease_probe_interval_seconds: u64` (used by Task 3)

- [ ] **Step 1: Add the field to `AppConfig`**

In `crates/gatewayd/src/main.rs`, after line 4645 (`provider_lease_poll_interval_seconds: u64,`), add:

```rust
    provider_credential_lease_probe_interval_seconds: u64,
```

- [ ] **Step 2: Parse from env with floor**

After line 5728 (the `.max(5)` closing `provider_lease_poll_interval_seconds`), add:

```rust
            provider_credential_lease_probe_interval_seconds: env_u64(
                "CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS",
                300,
            )
            .max(30),
```

- [ ] **Step 3: Set the field in every test-config constructor**

The struct is constructed at these locations — add the new field to each. Exact context: line 7737 area (`test_app_config`), line 37130 area (`test_config`), and any other `AppConfig {` literal in `mod tests`.

For `test_app_config` (around 7737, adjacent to `provider_lease_poll_interval_seconds: 30,`):

```rust
        provider_credential_lease_probe_interval_seconds: 300,
```

For `test_config` (around 37130, adjacent to `provider_lease_poll_interval_seconds: 30,`):

```rust
        provider_credential_lease_probe_interval_seconds: 300,
```

Locate any remaining `AppConfig {` literals missing the field with:

Run: `cargo check -p gatewayd --bin gatewayd`
Expected: no "missing field `provider_credential_lease_probe_interval_seconds`" errors. Fix any remaining literal by adding the same line.

- [ ] **Step 4: Add a parse test**

In `mod tests`, add:

```rust
    #[test]
    fn credential_lease_probe_interval_env_parses_with_floor() {
        std::env::set_var("CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS", "45");
        let config = test_config();
        std::env::remove_var("CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS");
        assert_eq!(config.provider_credential_lease_probe_interval_seconds, 45);

        std::env::set_var("CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS", "5");
        let config = test_config();
        std::env::remove_var("CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS");
        assert_eq!(config.provider_credential_lease_probe_interval_seconds, 30);
    }
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p gatewayd --bin gatewayd -- credential_lease_probe_interval_env_parses_with_floor`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/gatewayd/src/main.rs
git commit -m "feat(gatewayd): configurable credential lease probe interval (CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS)"
```

---

### Task 2: Remove CDP keepalive config, state, alert, and status fields

**Files:**
- Modify: `crates/gatewayd/src/main.rs`

**Interfaces:**
- Removes: `AppConfig.provider_cdp_keepalive_enabled`, `AppConfig.provider_cdp_keepalive_interval_seconds`, `ProviderCdpKeepaliveState`, `ProviderCdpKeepaliveStatusRecord`, `ProviderCdpKeepaliveSummaryPayload`, `AppState.provider_cdp_keepalive`, `provider_cdp_keepalive_summaries`, `provider_cdp_keepalive_loop`, `maybe_run_provider_cdp_keepalive`, `provider_supports_cdp_keepalive`, `provider_cdp_keepalive_url_matches`, `provider_cdp_keepalive_target_selector`, the `{provider}_cdp_keepalive_failing` alert, and `OperationsOverviewPayload.provider_cdp_keepalive`.
- Consumes: nothing.

Work top-to-bottom through the file.

- [ ] **Step 1: Remove the config fields**

Remove lines 4646-4647:

```rust
    provider_cdp_keepalive_enabled: bool,
    provider_cdp_keepalive_interval_seconds: u64,
```

Remove the env parse block (lines 5729-5734):

```rust
            provider_cdp_keepalive_enabled: env_bool("CCBG_PROVIDER_CDP_KEEPALIVE_ENABLED", false),
            provider_cdp_keepalive_interval_seconds: env_u64(
                "CCBG_PROVIDER_CDP_KEEPALIVE_INTERVAL_SECONDS",
                300,
            )
            .max(30),
```

- [ ] **Step 2: Remove `AppState` field and its construction**

Remove the field declaration (line 256):

```rust
    provider_cdp_keepalive: Arc<Mutex<ProviderCdpKeepaliveState>>,
```

Remove all three construction sites (lines 7086, 37379, 52658):

```rust
        provider_cdp_keepalive: Arc::new(Mutex::new(ProviderCdpKeepaliveState::default())),
```

- [ ] **Step 3: Remove the keepalive spawn**

Remove line 7113:

```rust
    tokio::spawn(provider_cdp_keepalive_loop(state.clone()));
```

- [ ] **Step 4: Remove the state structs and summary payload**

Remove `ProviderCdpKeepaliveStatusRecord` (lines 2266-2274) and `ProviderCdpKeepaliveState` (lines 2276-2289), and `ProviderCdpKeepaliveSummaryPayload` (lines 2363-2373).

- [ ] **Step 5: Remove the status payload field and its population**

Remove the struct field (line 2340):

```rust
    provider_cdp_keepalive: Vec<ProviderCdpKeepaliveSummaryPayload>,
```

Remove the local binding (line 4036):

```rust
    let provider_cdp_keepalive = provider_cdp_keepalive_summaries(state);
```

Remove the struct assignment (line 4088):

```rust
        provider_cdp_keepalive,
```

- [ ] **Step 6: Remove the alert block**

Remove lines 12435-12463 (`if state.config.provider_cdp_keepalive_enabled { ... }` block in `build_admin_alerts`).

- [ ] **Step 7: Remove the CDP keepalive functions**

Remove all of these, which are contiguous in the file:
- `provider_supports_cdp_keepalive` (12783-12785)
- `provider_cdp_keepalive_url_matches` (12787-12794)
- `provider_cdp_keepalive_target_selector` (12796-12802)
- `maybe_run_provider_cdp_keepalive` (12804-12920)
- `provider_cdp_keepalive_loop` (12922-12943)

Also remove `provider_cdp_keepalive_summaries` (line 29262 through its closing brace).

- [ ] **Step 8: Remove CDP keepalive tests**

Remove:
- `provider_cdp_keepalive_url_match_is_provider_scoped` (39891-39909)
- `provider_cdp_keepalive_selector_defaults_are_provider_scoped` (39911-39921)
- `admin_status_surfaces_provider_cdp_keepalive_failures` (39924-39962)

- [ ] **Step 9: Compile check**

Run: `cargo check -p gatewayd --bin gatewayd`
Expected: no references to `cdp_keepalive` / `provider_cdp_keepalive` remain. Confirm:

Run: `Select-String -Path crates/gatewayd/src/main.rs -Pattern "cdp_keepalive|CdpKeepalive"`
Expected: no output.

- [ ] **Step 10: Run the existing lease-probe test group**

Run: `cargo test -p gatewayd --bin gatewayd -- credential_lease_probe provider_lease`
Expected: PASS (these tests are untouched).

- [ ] **Step 11: Commit**

```bash
git add crates/gatewayd/src/main.rs
git commit -m "refactor(gatewayd): remove CDP keepalive loop, state, alert, and config"
```

---

### Task 3: Drive the lease probe loop with the configured interval

**Files:**
- Modify: `crates/gatewayd/src/main.rs`

**Interfaces:**
- Consumes: `AppConfig.provider_credential_lease_probe_interval_seconds` (Task 1)
- Produces: keepalive cadence = configured interval on success, `60_000` ms when `requires_reauth`.

- [ ] **Step 1: Extract a cadence helper and use it**

Add a helper function directly above `async fn maybe_probe_provider_credential_lease`:

```rust
fn credential_lease_probe_advance_ms(interval_seconds: u64, requires_reauth: bool) -> u64 {
    if requires_reauth {
        60_000
    } else {
        interval_seconds.saturating_mul(1000)
    }
}
```

In `maybe_probe_provider_credential_lease`, replace lines 12748-12751:

```rust
    guard.next_probe_at_unix_ms_by_provider.insert(
        provider,
        now.saturating_add(if requires_reauth { 60_000 } else { 5 * 60_000 }),
    );
```

with:

```rust
    let advance_ms = credential_lease_probe_advance_ms(
        state.config.provider_credential_lease_probe_interval_seconds,
        requires_reauth,
    );
    guard.next_probe_at_unix_ms_by_provider.insert(provider, now.saturating_add(advance_ms));
```

- [ ] **Step 2: Write the failing cadence test**

Add to `mod tests`:

```rust
    #[test]
    fn credential_lease_probe_advance_ms_uses_interval_and_short_reauth_floor() {
        assert_eq!(credential_lease_probe_advance_ms(300, false), 300_000);
        assert_eq!(credential_lease_probe_advance_ms(45, false), 45_000);
        assert_eq!(credential_lease_probe_advance_ms(300, true), 60_000);
        assert_eq!(credential_lease_probe_advance_ms(45, true), 60_000);
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p gatewayd --bin gatewayd -- credential_lease_probe_advance_ms_uses_interval_and_short_reauth_floor`
Expected: FAIL — `credential_lease_probe_advance_ms` not defined.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p gatewayd --bin gatewayd -- credential_lease_probe_advance_ms_uses_interval_and_short_reauth_floor`
Expected: PASS.

- [ ] **Step 5: Compile + full lease probe suite**

Run: `cargo check -p gatewayd --bin gatewayd`
Expected: clean.

Run: `cargo test -p gatewayd --bin gatewayd -- credential_lease_probe`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/gatewayd/src/main.rs
git commit -m "feat(gatewayd): drive lease probe cadence from configured interval; keep 60s reauth floor"
```

---

### Task 4: Update deploy template

**Files:**
- Modify: `deploy/lxc/ccbg.env`

- [ ] **Step 1: Replace the CDP keepalive vars with the lease probe interval**

In `deploy/lxc/ccbg.env`, remove (if present):

```
CCBG_PROVIDER_CDP_KEEPALIVE_ENABLED=true
CCBG_PROVIDER_CDP_KEEPALIVE_INTERVAL_SECONDS=300
```

Add after `CCBG_PROVIDER_LEASE_POLL_INTERVAL_SECONDS` (or in the provider section):

```
# Backend API keepalive: how often to probe each provider's session via its own
# API (Unicom probe_auth / Telecom getUserInfoForPortal / Mobile list_page).
# Reauth-required providers are re-probed every 60s regardless.
CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS=300
```

- [ ] **Step 2: Verify**

Run: `git diff deploy/lxc/ccbg.env`
Expected: only the keepalive var swap shown above.

- [ ] **Step 3: Commit**

```bash
git add deploy/lxc/ccbg.env
git commit -m "chore(deploy): swap CDP keepalive vars for credential lease probe interval"
```

---

### Task 5: Full verification

**Files:** none (verification only)

- [ ] **Step 1: Full compile**

Run: `cargo check -p gatewayd --bin gatewayd`
Expected: clean (only pre-existing `list_objects_v2` unused warning tolerated).

- [ ] **Step 2: Full gatewayd test suite**

Run: `cargo test -p gatewayd --bin gatewayd`
Expected: all pass (pre-existing flaky `auth_capture_prompts_fill_missing_browser_flow_inputs` "database is locked" tolerated if it fails).

- [ ] **Step 3: Confirm no CDP keepalive remnants**

Run: `Select-String -Path crates/gatewayd/src/main.rs -Pattern "cdp_keepalive|CdpKeepalive|CDP_KEEPALIVE"`
Expected: no output.

- [ ] **Step 4: Update lessons-learned**

Append a short note to `docs/lessons-learned.md` under a `## Session Keepalive` heading:

```markdown
## Session Keepalive

- Keepalive must go through the backend API (probe_auth / getUserInfoForPortal /
  list_page), not a CDP browser reload. A browser is only needed for login and
  re-authentication. The credential lease probe loop is the keepalive loop.
```

Commit:

```bash
git add docs/lessons-learned.md
git commit -m "docs: note backend API keepalive replacing CDP reload"
```

---

## Execution handoff

Push the completed commits to `origin/main`, then on `.49` (when reachable):
1. Edit `/etc/ccbg/ccbg.env`: remove `CCBG_PROVIDER_CDP_KEEPALIVE_ENABLED` and `CCBG_PROVIDER_CDP_KEEPALIVE_INTERVAL_SECONDS`; add `CCBG_PROVIDER_CREDENTIAL_LEASE_PROBE_INTERVAL_SECONDS=300`.
2. Build Linux ELF via `cargo-zigbuild`, package with `build-lxc-package.sh`, install via the LXC install script, restart `ccbg.service`.
3. Verify Admin status shows lease `last_verified_at` for Unicom/Telecom/Mobile and no CDP keepalive alert.
