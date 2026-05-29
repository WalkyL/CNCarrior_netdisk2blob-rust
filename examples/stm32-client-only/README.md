# STM32 Client-Only Example

This example shows the intended STM32 integration shape: the MCU is a client of a nearby CCBG host. It does not run `gatewayd`, SQLite, provider adapters, OneDrive auth, replication workers, Admin Web, or MCP stdio.

## Scope

Implemented operations:

- `HeadObject`
- `GetObject`
- streaming `PutObject` with `UNSIGNED-PAYLOAD`

The example uses path-style S3 requests:

```text
http://<gateway-host>:61080/<bucket>/<key>
```

## Integration Points

The C code is intentionally platform-neutral. Your STM32 firmware must provide:

- `sha256_hex`: SHA-256 of a memory buffer, hex encoded
- `hmac_sha256`: HMAC-SHA256
- `utc_now`: UTC timestamp in `YYYYMMDDTHHMMSSZ` and `YYYYMMDD`
- `http_request`: HTTP transport backed by LwIP, a modem stack, or a board SDK

For STM32 projects that already use mbedTLS, map these callbacks to `mbedtls_sha256_*` and `mbedtls_md_hmac`.

## Resource Limits

Default example limits:

- IO chunk: `1024` bytes
- max object: `32 KiB`
- concurrency: `1`
- retry attempts: caller-configured, example uses `2`
- no dynamic allocation

`PutObject` uses `x-amz-content-sha256: UNSIGNED-PAYLOAD`, so the firmware can stream the body through the HTTP transport without hashing the whole object first. The gateway requires a valid `Content-Length` for this path.

## Build Check

Host-side syntax check:

```bash
scripts/check-stm32-client-example.sh
```

The compiled host executable uses fake crypto and fake HTTP only to prove the example is syntactically portable. It is not a real network test.

## Board Acceptance

Use a local gateway with a dedicated S3 key:

```dotenv
CCBG_BIND_ADDR=0.0.0.0:61080
CCBG_S3_ACCESS_KEY_ID=ccbg-stm32
CCBG_S3_SECRET_ACCESS_KEY=<board-specific-secret>
CCBG_PRIMARY_PROVIDER=stub
CCBG_ONEDRIVE_ENABLED=false
CCBG_ONEDRIVE_REPLICATION_ENABLED=false
```

Pass criteria on board:

- `HeadObject` returns `200` for a known object
- `PutObject` uploads a small object, default <= `32 KiB`
- `GetObject` reads the same object through a caller-provided chunk sink
- request timeout and retry are bounded
- peak object buffer remains within the configured chunk size
- firmware does not store provider credentials or control-plane API keys

If MCP is needed from a microcontroller-class device, prefer a host-side bridge or the S3 data plane. Direct MCP stdio is not a STM32 target.
