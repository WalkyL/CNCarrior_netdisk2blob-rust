# Browser Flow LLM Repair

This note defines the first operator-facing repair loop for carrier login browser-flow catalogs.

## Objective

Carrier login pages can change selectors, frames, or intermediate steps without changing the gateway data plane. Operators need a way to capture the current CDP page, ask their configured LLM to propose a browser-flow catalog update, validate it, and keep the update outside the packaged defaults.

## Interfaces

- `POST /api/browser-flows/repair-bundle`
  - Input: provider, surface, flow id, optional last error, and optional CDP endpoint override.
  - Output: current catalog, selected flow, sanitized CDP snapshots, and a strict LLM prompt.
- `POST /api/browser-flows/overrides`
  - Input: provider, surface, a full replacement `BrowserFlowCatalog`, and optional note.
  - Output: override file path and content hash.

## Admin Workflow

Operators do not need to handcraft JSON or edit packaged files directly.

Current Admin behavior:

1. Open the provider login assistant for `unicom`, `telecom`, or `mobile`.
2. When the live page drifts away from the known flow, click `LLM 修复登录插件`.
3. The Admin page calls `POST /api/browser-flows/repair-bundle` with the selected provider/surface/flow.
4. Gateway connects to the current CDP page and captures redacted frame snapshots from:
   - the main frame
   - any frame selectors already referenced by the current catalog
5. The Admin page sends the returned strict repair prompt to the configured front-end LLM endpoint.
6. The LLM must return strict JSON:

```json
{
  "catalog": { "schema_version": 1, "provider": "telecom", "surface": "cloud.189.cn-web" },
  "summary": "one short sentence",
  "risk": "low"
}
```

7. The operator reviews the summary/risk in a browser confirm dialog.
8. Only after confirmation does the Admin page call `POST /api/browser-flows/overrides`.
9. The effective browser-flow catalog is reloaded, and subsequent login runs use the saved override.

## Configuration

Packaged browser-flow catalogs stay under:

- `CCBG_BROWSER_FLOW_CATALOG_DIR`

Operator-written override catalogs live under:

- `CCBG_BROWSER_FLOW_OVERRIDE_DIR`

Default override location:

- `<CCBG_CREDENTIALS_DIR>/browser-flow-overrides`

This keeps emergency operator repairs outside the packaged release files.

## Data Flow

1. Gateway loads packaged catalogs from `CCBG_BROWSER_FLOW_CATALOG_DIR`.
2. Gateway overlays user catalogs from `CCBG_BROWSER_FLOW_OVERRIDE_DIR`.
3. Repair bundle captures sanitized facts from CDP for the selected flow and known catalog frames.
4. LLM proposes a complete replacement catalog JSON.
5. Gateway validates schema and provider/surface match before writing the override.
6. Browser-flow endpoints read the effective packaged-plus-override collection.

## Safety

- Repair bundles redact secrets by construction and do not include cookies, tokens, phone numbers, SMS codes, or passwords.
- The LLM is not allowed to write files directly.
- Overrides are full catalogs, not arbitrary filesystem paths.
- Packaged catalogs remain unchanged and can be restored by deleting the override file.
- Front-end save is confirm-gated. The first version does not auto-save LLM output silently.

## LLM Boundary

- The front-end repair path uses a dedicated strict-JSON LLM call instead of the generic operator explain flow.
- If the selected OpenAI-compatible endpoint rejects `response_format={"type":"json_object"}`, the Admin page retries once without that field but keeps the strict JSON-only prompt.
- If the front-end LLM call fails because of endpoint policy or CORS, Admin copies the generated repair prompt to the clipboard and opens the AI dialog so the operator can continue manually.

## Deployment Notes

- The repair flow depends on both the `gatewayd` binary and the packaged Admin HTML.
- A backend-only deploy is not sufficient; LXC/native packages must carry the updated `assets/admin/index.html`.
- Existing provider runtime credentials, browser profiles, and packaged catalogs are not modified by this feature. Only the override directory changes at runtime.
