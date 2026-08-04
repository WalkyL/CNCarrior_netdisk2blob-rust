# Code Audit Revalidation — 2026-08-04

**Project:** ccbg (gatewayd)
**Workspace:** `D:\workspaces\ccbg`
**Basis:** Focused revalidation of the original 2026-07-13 audit report against
the current tree, not a re-scan of the full repository. Original numbers that
were not recomputed (e.g. exact `.expect()` / `warn!` counts, total SLOC,
overall test-coverage ratios, "zero integration tests") are intentionally
dropped rather than restated without fresh measurement.

**Commits applied since the 2026-07-13 audit** (short-head → topic):
- `ab85e9e` / `3d060cc` / `7e3bbce` / `7fa269e` / `3c63fcf` — immediate security
  fixes (cookie `Secure`, CSP, security headers, sessionStorage removal)
- `ee1e2df` — test expansion across 4 crates + config/env hardening
- `9104e3e` — HSTS header + `escapeHtml` XSS gaps
- `01a8b5e` — compiler-warning cleanup + WAL path traversal fix
- `ffb3809` — docs (`.49` LXC OOM / `memory.high` lessons)
- `2611c7f` — admin generated-script newline fix + carrier assistant window
  unification

---

## 1. Findings from the 2026-07-13 audit — now RESOLVED

Each of these was reported in the original audit and has since been fixed. They
should not be re-scheduled as outstanding work.

| ID | Original finding | Status | Evidence |
|----|------------------|--------|----------|
| C5 / FC3 | Admin session cookie missing `Secure` | **Resolved** | `CCBG_ADMIN_COOKIE_SECURE` defaults `true` (`main.rs:5775`); cookie header emits `Secure` conditionally. |
| FC1 | No Content-Security-Policy | **Resolved** | `admin_index` sets CSP with nonce (`main.rs:13371-13376`). |
| FH2 | No `X-Frame-Options` | **Resolved** | `x-frame-options: DENY` (`main.rs:13377`). |
| FH3 | No `X-Content-Type-Options` | **Resolved** | `x-content-type-options: nosniff` (`main.rs:13378`). |
| FH4 | No HSTS | **Resolved** | `strict-transport-security` (`main.rs:13381`), commit `9104e3e`. |
| FH1 | Carrier credentials in `sessionStorage` | **Resolved** | No `sessionStorage` usage remains in the admin asset; commits `7fa269e` / `7e3bbce`. |
| C2 | WAL key path allows `.` | **Resolved** | WAL key components sanitized to `[A-Za-z0-9_-]` (`main.rs:26379`, `26400`), commit `01a8b5e`. The original recommendation ("reject any key containing `.`") was not the implemented approach. |
| H1 / 6.1 | Hardcoded default admin credentials | **Resolved** | No default admin password; auth fails when none is configured (`main.rs:7679-7695`, `7849-7875`). The "default `admin`/`password`" claim is stale. |
| H3 / 6.1 | Hardcoded Unicom dispatcher secret in `example.env` | **Resolved** | Example dispatcher secret is empty (`config/example.env`). |
| H4 / 6.1 | SMB sidecar binds `0.0.0.0` by default | **Resolved** | `"" \| "0.0.0.0" \| "*"` normalized to `127.0.0.1` (`smb-sidecar-host/src/main.rs:552`). |
| 7.3 #4 | `escapeHtml` only escapes 4 chars | **Resolved** | Now escapes `& < > ' \`` (`index.html:7429-7436`), commit `9104e3e`. |

---

## 2. Findings that still hold (current evidence)

These remain in the current tree and are worth keeping on the roadmap.

### 2.1 Unauthenticated JSON listing endpoints (was C1, narrowed)
`GET /v1/containers` (`main.rs:31494`) and `GET /v1/objects` (`main.rs:31071-31072`,
route at `main.rs:7126-7127`) perform no `authorize_s3` check — only data-plane
rate limiting / permit acquisition. S3 bucket/object routes do authenticate
(e.g. `main.rs:31544`, `31735`). Scope the remediation to these two JSON routes;
do not treat the S3 data plane as unauthenticated.

### 2.2 No CSRF protection / login rate limiting (was FC2, M4)
State-changing admin endpoints have no CSRF token (only `SameSite=Strict` cookie
mitigation), and the login endpoint has no brute-force rate limiting
(`admin_login`, `main.rs:13181`). Confirm the threat model still accepts
SameSite-only CSRF defense and consider a bounded login throttle.

### 2.3 Unbounded SSE channel (was 4.2)
Provider limit probe stream uses `mpsc::unbounded_channel()` (`main.rs:13980`).
A slow HTTP consumer can grow the channel without bound. Prefer a bounded channel
with backpressure. This is a real OOM/backlog vector but requires a slow-consumer
scenario to trigger.

### 2.4 `std::sync::Mutex` in async context (was 4.1)
`AppState` sharing fields use `std::sync::Mutex` (`main.rs:420-424` etc.). Safe
under current block scoping, but brittle if a lock is ever held across an
`.await`. Low urgency; `tokio::sync::Mutex` would remove the footgun.

### 2.5 Orphaned browser-CDP reader task (was 4.1)
`spawn_reader_task` discards its `JoinHandle` (`browser-cdp/src/lib.rs`). A reader
crash yields a silent CDP disconnect with no recovery path. Only affects the
CDP/browser-flow path.

### 2.6 Misc remaining (from original sections 4.3 / 6.2 / 6.3)
- `Ordering::SeqCst` where `Acquire/Release` suffices (browser-cdp, blob-core) —
  low severity.
- `Notify::notify_waiters()` thundering-herd potential at low concurrency — low.
- Deploy configs bind admin/S3 APIs to `0.0.0.0`; default S3 key `change-me`;
  default KMS `local_mock` — operator-configured defaults, worth keeping explicit
  in deployment docs but not code bugs.
- No log-redaction utility for config values / provider debug headers — medium,
  unchanged from original.
- Hardcoded external URL `carrier-disk-gateway.agi2030.online` in frontend — low.

---

## 3. Items NOT revalidated in this pass

The following original claims require a fresh measurement or deeper pass before
they can be restated as current fact; they are neither confirmed nor refuted
here:

- Exact `.expect()` / silenced-error / `warn!`-without-propagation counts
  (original section 2).
- Full-crate test-coverage ratios and the "zero integration tests" claim
  (original section 5).
- Frontend finding-severity tallies (original section 7.2).

The structural concerns they point at (data-plane `.expect()` on poisoned mutex,
thin test coverage for object-crypto / S3 handlers / replication / auth-broker,
single-file admin app) remain reasonable hardening targets regardless of the
precise numbers.

---

## 4. Priority Roadmap (revalidated)

### Immediate
1. Add `authorize_s3`-equivalent auth (or explicit public intent) to
   `/v1/containers` and `/v1/objects`.
2. Add login rate limiting and decide on explicit CSRF defense.
3. Bound the provider-limit-probe SSE channel.

### Short-term
4. Move `AppState` mutexes to `tokio::sync::Mutex` (or document why not).
5. Add reader-task recovery for browser-CDP.
6. Add focused tests for object-crypto (encryption/decryption/integrity/error)
   and S3 API handlers.

### Longer-term
7. Log-redaction utility for tokens/secrets in provider debug output.
8. Remove dead code (the 24 warnings were cleared in `01a8b5e`; re-run to find
   any remaining dead paths such as `list_objects_v2` / browser-flow helpers).
9. Restate test-coverage and integration-test numbers from a fresh measurement.
