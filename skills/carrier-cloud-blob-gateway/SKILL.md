---
name: carrier-cloud-blob-gateway
description: Use this skill when operating carrier-cloud-blob-gateway for local S3-compatible object access, provider health checks, replication/fallback diagnostics, and alert-aware risk reporting through MCP tools.
---

# Carrier Cloud Blob Gateway

Use this skill for carrier-cloud-blob-gateway operations: local S3-compatible object gateway checks, provider status triage, replication state validation, fallback readiness checks, and alert-aware incident diagnosis.

## MCP Connection Convention

- Prefer local MCP server first.
- Use `stdio` as default transport.
- Optional Streamable HTTP can be enabled with `MCP_SERVER_HTTP_ENABLED=true`.
- The same HTTP transport also accepts `CCBG_MCP_HTTP_*` aliases.
- Default HTTP endpoint is `http://127.0.0.1:61084/mcp`; no public exposure is required.
- Without auth, use discovery only: `tools/list`, `resources/list`, `prompts/list`, `mcp_feature_access_summary`, and `ccbg://public/feature-access-summary`.
- Operator actions over HTTP require the configured bearer token.
- This endpoint is for `ccbg` itself only. If the task also needs the external `.51` coordination hub, use `http://192.168.1.51:8787/mcp` and expect service identity `agent-nats-redmine-hub`.

## MCP Tool Map

### `mcp_feature_access_summary`
- Purpose: return the MCP discovery map, auth boundary, and highlighted operator routes.
- When to call: first contact, unknown auth state, or before choosing tools.
- Key parameters: none.

### `provider_list`
- Purpose: list configured providers and availability surface.
- When to call: initial discovery before any provider-specific diagnosis.
- Key parameters: none.

### `provider_health`
- Purpose: inspect provider health facts (connectivity, auth/readiness signals).
- When to call: provider failures, fallback uncertainty, or before escalation.
- Key parameters: required `provider_id` string.

### `replication_get_status`
- Purpose: read replication status counters and health.
- When to call: before claiming data durability or suggesting fallback reads.
- Key parameters: none.

### `replication_list_failed_jobs`
- Purpose: enumerate failed replication jobs for remediation triage.
- When to call: incident investigation, backlog review, retry planning.
- Key parameters: optional `limit` unsigned integer.

### `s3_list_buckets`
- Purpose: verify accessible bucket inventory from the local S3-compatible gateway.
- When to call: baseline access check and routing sanity verification.
- Key parameters: none.

### `alerts_list_recent`
- Purpose: retrieve recent alert summary for correlated risk output.
- When to call: before high-impact recommendations or when health looks abnormal.
- Key parameters: optional `limit` unsigned integer.

### Operator maintenance tools
- `admin_status_get`
- `applications_get` / `applications_update`
- `content_policies_get` / `content_policies_update`
- `topology_update`
- `provider_credentials_get` / `provider_credentials_update`
- `auth_capture_policy_get` / `auth_capture_policy_update`
- `replication_dlq_list`
- `replication_retry_job`
- `replication_dlq_replay_job`
- `replication_dlq_replay_target`

## Prompts As Optional Helpers

Use prompts only after baseline state is known:
- `safe_object_read`
- `check_replication_before_fallback`
- `diagnose_provider_connection_failure`
- `retry_replication_for_one_object`

## Default Invocation Order

1. Fixed baseline sequence (always first): `provider_list` -> `replication_get_status` -> `alerts_list_recent`.
   If auth state is unknown on HTTP, prepend `mcp_feature_access_summary`.
2. Provider-specific anomaly path: run `provider_health(provider_id)` only after step 1 identifies a target provider.
3. Replication failure/retry path: run `replication_list_failed_jobs(limit=...)` when planning retries or investigating replication failures.
4. S3 baseline path: run `s3_list_buckets` when bucket visibility/routing baseline is required.
5. Then use prompt helpers to refine operator actions.
6. End with risk-scoped output based on observed facts only.

Do not infer hidden internal state. If data is missing, state uncertainty explicitly.

## Operation Risk Levels

- Read-only status query: `provider_list`, `replication_get_status`, `alerts_list_recent`, `s3_list_buckets`.
- Low-risk diagnosis: `provider_health(provider_id)`, targeted replication failure lookup via `replication_list_failed_jobs(limit=...)`.
- High-risk actions: write/overwrite/delete/switch primary provider/bulk retry.

## High-Risk Gates

- Primary write success does not imply replication completed.
- Before primary write/overwrite/delete: must confirm provider health, replication status, and recent alerts.
- Before fallback read: must confirm replication is reliable. If MCP only exposes summary/counter signals, do not claim a specific object is replicated; escalate to human confirmation.
- Replication retry: only single-object retry plan is allowed by default. Bulk retry requires explicit human confirmation and prior `replication_list_failed_jobs` + `alerts_list_recent`.
- Switching primary provider always requires explicit human confirmation; never auto-execute.
- If provider health is abnormal, diagnose first and output risk; do not blind retry writes.
- Never expose token/secret material in outputs.
- Refuse blind delete + blind primary switch requests.
- Keep OneDrive in Parking/default-hidden posture; do not present it as default guidance.

## Scenario Replay Checklist (6)

- Normal read-only health check: execute `provider_list` -> `replication_get_status` -> `alerts_list_recent`; summarize health and uncertainty.
- Pre-primary-write check: baseline sequence + target `provider_health(provider_id)`; block write recommendation when alerts/health/replication are not clean.
- Pre-fallback-read check: baseline sequence; if only aggregate replication view exists, require human confirmation for object-level replication before fallback claim.
- Single-object replication retry plan: baseline sequence + `replication_list_failed_jobs(limit=...)`; output one-object retry plan and stop before bulk actions.
- Provider health anomaly diagnosis: baseline sequence + `provider_health(provider_id)` + optional `replication_list_failed_jobs(limit=...)`; output risk-first diagnosis, no blind write retry.
- High-risk request path (delete/overwrite/switch primary/bulk retry): require explicit human confirmation, include current alerts/replication evidence, otherwise refuse execution.
