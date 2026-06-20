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
- Resource: `ccbg://public/feature-access-summary`
- Prompt: `discover_feature_access_model`

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
