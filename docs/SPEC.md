# MCP Public Discovery And Operator Ops Spec

## Objective

Expose `ccbg` MCP in two layers:

- Unauthenticated callers can discover available MCP features and see which ones require authentication.
- Authenticated HTTP callers can perform controlled operator workflows through MCP without bypassing `gatewayd` control APIs.

## Interface

Public MCP methods:

- `initialize`
- `notifications/initialized`
- `tools/list`
- `resources/list`
- `resources/read` for explicitly public resources only
- `prompts/list`
- `prompts/get` for explicitly public prompts only
- `tools/call` for explicitly public tools only

Public MCP features:

- Tool: `mcp_feature_access_summary`
- Tool: `mcp_storage_access_model_summary`
- Resource: `ccbg://public/feature-access-summary`
- Resource: `ccbg://public/storage-access-model`
- Prompt: `discover_feature_access_model`
- Prompt: `design_storage_access_mapping`

Protected operator tools:

- Existing read tools remain available but require auth on HTTP transport.
- New operator tools proxy `gatewayd` / Admin API operator endpoints for status, applications, content policies, topology, provider credentials, auth-capture policy, and replication DLQ maintenance.

## Data Flow

1. HTTP request reaches MCP transport.
2. Origin validation runs first.
3. JSON-RPC request is parsed to determine whether the target operation is public discovery or operator-only.
4. Public discovery requests can proceed without bearer auth.
5. Protected requests require the configured bearer token.
6. MCP tool execution calls the existing control-plane client.
7. Control-plane client proxies `gatewayd` / Admin API routes and returns JSON results to MCP.

## Error Handling

- Public discovery requests stay available without auth.
- Protected HTTP requests without valid auth return `401` with a JSON-RPC error payload and discovery guidance.
- Invalid JSON-RPC requests still return `400`.
- Upstream `gatewayd` / Admin API failures continue mapping into MCP machine-readable error payloads.
- Automatic retry is limited to safe GET requests; mutating POST operator calls are not retried automatically.

## Configuration

- Existing `MCP_SERVER_HTTP_*` variables remain supported.
- `CCBG_MCP_*` aliases are accepted for the HTTP transport configuration.
- HTTP bearer token stays externalized in env vars only.
- No secret is written into repo docs, MCP public discovery payloads, or default config.

## Public Storage Access Model

### Objective

Expose a no-auth MCP summary that teaches downstream applications how to map product fields such
as application, container, region, access mode, and affiliation onto CCBG's S3-compatible surface
without overloading S3 protocol fields.

### Interface

- Public MCP tool: `mcp_storage_access_model_summary`
- Public MCP resource: `ccbg://public/storage-access-model`
- Public MCP prompt: `design_storage_access_mapping`

### Field Mapping Contract

- `application` -> `application_id`
- `container` -> S3 bucket
- `region` -> SigV4 signing region only
- `access_mode` -> bucket/prefix layout policy
- `application_root` / `user_root` -> key prefix convention
- `affiliation` -> carrier-affinity write-placement hint, not an S3 signing field

### Control-Plane Surface

- `GET /api/applications` returns `affiliation`, `application_root`, and `user_root_template`
- `POST /api/applications` accepts the same fields
- `GET /api/applications/{application_id}/credentials` includes the same mapping fields plus the plaintext secret for explicit export flows

### Routing Semantics

- `content_policy` remains the highest-priority write-routing control.
- When no matching content policy exists, `affiliation` may bias new object home placement toward the matching primary-capable provider.
- `affiliation` does not rewrite SigV4 region semantics and does not force retroactive migration of existing objects.
- When the chosen home provider already matches the effective target provider, the persisted replication plan omits that self-target.

### Guidance

- Keep `region` stable and neutral, preferably `us-east-1`.
- Do not encode carrier choice in `region` or `location_constraint`.
- Use bucket + prefix for shared-container tenancy.
- Use endpoint selection, scoped application credentials, or bucket/prefix policy for carrier
  routing.

## Admin S3 Credential Export

### Objective

Let authenticated Admin Web operators reveal and copy one application's S3 credentials without weakening the default `/api/applications` list contract.

### Interface

- New control-plane endpoint: `GET /api/applications/{application_id}/credentials`
- Admin Web adds a per-application `显示 S3 凭据` action.
- Admin Web adds a per-application `复制 S3 凭据` action.
- The advanced secret field can display the current plaintext secret after explicit operator action.
- The advanced panel also shows a paste-ready env snippet with endpoint, bucket, prefix, access key, and secret.

### Data Flow

1. Admin Web loads the normal application list from `/api/applications`, which still hides plaintext secrets.
2. Operator clicks `显示 S3 凭据` or `复制 S3 凭据` on one application.
3. Frontend reuses the current draft if a new secret is already present; otherwise it fetches `/api/applications/{application_id}/credentials`.
4. Frontend fills the secret field with the returned plaintext secret, expands the advanced panel, and renders an env-style snippet using the current endpoint plus the application's current bucket/prefix scope.
5. `复制 S3 凭据` additionally writes that snippet to the clipboard.

### Error Handling

- Empty `application_id` is rejected.
- Unknown applications return `404`.
- Browsers without `navigator.clipboard` still reveal the secret in the form and leave the env snippet visible for manual copy.

### Configuration

- No new environment variables.
- S3 secrets remain stored in the existing control-plane state or default S3 config.
- Plaintext secrets are still excluded from `/api/applications` list responses.

## SMB Sidecar Runtime Isolation

### Objective

Keep `ccbg-smb-sidecar-sync.service` as a short-lived reconcile trigger while moving long-running
`rclone mount` and `smbd` processes out of that oneshot service cgroup.

### Interface

- `deploy/lxc/ccbg-smb-sidecar.py sync` still owns reconcile logic and `status.json` updates.
- Long-running SMB sidecar processes are launched as dedicated transient systemd service units.
- Runtime status continues to expose `pid`, `running`, `mounted`, and log file paths for each
  managed process/share.

### Data Flow

1. `ccbg-smb-sidecar-sync.service` runs `ccbg-smb-sidecar.py sync`.
2. The script computes the desired sidecar runtime spec and compares it with the stored metadata.
3. When reconciliation is needed, the script stops prior managed runtime units, unmounts stale
   shares, and launches fresh transient units for `smbd` and each `rclone mount`.
4. `status.json` and `managed-runtime.json` record the new runtime state without leaving the child
   processes inside the `sync.service` cgroup.

### Error Handling

- If `systemd-run` is unavailable, the script falls back to the previous direct-process launch
  behavior.
- A runtime-spec version bump forces one post-upgrade reconcile so older direct-process deployments
  are migrated to transient units on the next sync pass.

### Configuration

- No new operator-facing environment variables.
- Transient unit names are derived from share ids and do not require manual configuration.

## HTTP 5xx Diagnostic Logging

### Objective

Replace generic `tower_http` 5xx trace lines with higher-signal request logs that include route
context and the concrete application error message.

### Interface

- All `gatewayd` HTTP listeners use a shared custom trace layer.
- `ApiError`, `DataPlaneApiError`, `S3Error`, and `ControlApiError` emit an additional structured
  warning when returning a `5xx` response.

### Data Flow

1. The custom trace layer creates a span with `method`, `route`, and `query`.
2. If a response returns `5xx`, the trace layer logs `status` and `latency_ms` on that span.
3. When the response body is built from an internal error type, the error adapter logs the
   response class plus the concrete error code/message.

### Error Handling

- Non-5xx responses keep the current low-noise behavior.
- Transport failures before a response still log through the custom trace `on_failure` hook.

### Configuration

- No new environment variables.
- Existing `RUST_LOG` filtering remains the control point for surfacing the new warning lines.
